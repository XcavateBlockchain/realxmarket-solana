use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, PROPOSAL_SEED, REGION_STATE_SEED, VAULT_SEED};
use crate::error::RegionsError;
use crate::state::{Config, RegionProposal, RegionState, RegionStatus};
use crate::vault::release_from_vault;

/// Finalize an expired proposal. Permissionless: anyone can crank it once the
/// voting window closes. If it passed (threshold + quorum) the region becomes
/// claimable by the proposer, with their bond kept as the region's collateral.
/// Otherwise the bond is returned in full and the region is marked rejected.
#[derive(Accounts)]
#[instruction(region_id: u16)]
pub struct FinalizeRegionProposal<'info> {
    /// Pays the transaction fee; no authority required.
    #[account(mut)]
    pub cranker: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ RegionsError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The protocol's XCAV vault.
    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        token::mint = config.xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [REGION_STATE_SEED, &region_id.to_le_bytes()],
        bump = region_state.bump,
    )]
    pub region_state: Box<Account<'info, RegionState>>,

    #[account(
        mut,
        close = proposer,
        has_one = proposer,
        seeds = [PROPOSAL_SEED, &region_state.proposal_id.to_le_bytes()],
        bump = proposal.bump,
    )]
    pub proposal: Box<Account<'info, RegionProposal>>,

    /// CHECK: the proposal's proposer (enforced by `has_one`); receives the
    /// closed proposal's rent.
    #[account(mut)]
    pub proposer: UncheckedAccount<'info>,

    /// Any XCAV account the proposer owns; needed only on a rejection, which
    /// refunds the bond into it. Not pinned to the ATA, so a re-owned one
    /// can't wedge the crank.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = proposer,
    )]
    pub proposer_token: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn finalize_region_proposal_handler(
    ctx: Context<FinalizeRegionProposal>,
    region_id: u16,
) -> Result<()> {
    require!(
        ctx.accounts.region_state.status == RegionStatus::Proposing,
        RegionsError::NotProposing
    );

    let now = Clock::get()?.unix_timestamp;
    require!(
        now >= ctx.accounts.proposal.expiry,
        RegionsError::VotingStillOngoing
    );

    let proposal = &ctx.accounts.proposal;
    let yes = proposal.yes_power;
    let no = proposal.no_power;
    let approval_base = yes.checked_add(no).ok_or(RegionsError::Overflow)?;

    let threshold_bps = ctx.accounts.config.threshold_bps as u128;
    // Governor Bravo quorum: only Yes counts, and Yes must clear the ratio over No
    // (abstains signal only). Set `quorum` high vs any single holder.
    let meets_threshold = yes > 0
        && (yes as u128).saturating_mul(10_000)
            >= (approval_base as u128).saturating_mul(threshold_bps);
    let meets_quorum = yes >= ctx.accounts.config.quorum;
    let proposal_id = proposal.proposal_id;
    let deposit = ctx.accounts.region_state.deposit;

    if meets_threshold && meets_quorum {
        // Passed: the region is now claimable by the proposer. The bond stays in
        // the vault and becomes the region's collateral once claimed; if the
        // proposer never claims, anyone can clear the state and refund them
        // after the claim deadline. (The proposal account closes here, returning
        // its rent to the proposer.)
        let region_state = &mut ctx.accounts.region_state;
        region_state.status = RegionStatus::Passed;
        region_state.claim_deadline = now
            .checked_add(ctx.accounts.config.owner_change_period)
            .ok_or(RegionsError::Overflow)?;

        emit!(RegionProposalPassed {
            region_id,
            proposal_id,
        });
    } else {
        // Rejected: return the bond in full and mark the state rejected so the
        // region can be proposed again once cleared.
        let proposer_token = ctx
            .accounts
            .proposer_token
            .as_ref()
            .ok_or(RegionsError::RefundAccountMissing)?;
        release_from_vault(
            &ctx.accounts.token_program.to_account_info(),
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.xcav_mint.to_account_info(),
            &proposer_token.to_account_info(),
            &ctx.accounts.config.to_account_info(),
            ctx.accounts.config.bump,
            deposit,
            ctx.accounts.xcav_mint.decimals,
        )?;

        ctx.accounts.region_state.status = RegionStatus::Rejected;
        emit!(RegionProposalRejected {
            region_id,
            proposal_id,
            refunded: deposit,
        });
    }
    Ok(())
}

#[event]
pub struct RegionProposalPassed {
    pub region_id: u16,
    pub proposal_id: u64,
}

#[event]
pub struct RegionProposalRejected {
    pub region_id: u16,
    pub proposal_id: u64,
    pub refunded: u64,
}
