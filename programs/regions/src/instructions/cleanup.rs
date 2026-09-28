use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, REGION_STATE_SEED, VAULT_SEED, VOTE_SEED};
use crate::error::RegionsError;
use crate::state::{Config, RegionState, RegionStatus, VoteRecord};
use crate::vault::release_from_vault;

/// Reclaim the XCAV a voter locked, once the proposal's voting window has ended.
/// The record's stored `expiry` means this never needs the proposal account
/// (which finalization may already have closed).
#[derive(Accounts)]
#[instruction(proposal_id: u64)]
pub struct UnlockVotingToken<'info> {
    #[account(mut)]
    pub voter: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ RegionsError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The voter's XCAV account the locked power is returned to.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = voter,
    )]
    pub voter_token: Box<InterfaceAccount<'info, TokenAccount>>,

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
        close = voter,
        seeds = [VOTE_SEED, &proposal_id.to_le_bytes(), voter.key().as_ref()],
        bump = vote_record.bump,
    )]
    pub vote_record: Box<Account<'info, VoteRecord>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn unlock_voting_token_handler(
    ctx: Context<UnlockVotingToken>,
    _proposal_id: u64,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    require!(
        now >= ctx.accounts.vote_record.expiry,
        RegionsError::VotingStillOngoing
    );

    let power = ctx.accounts.vote_record.power;
    if power > 0 {
        release_from_vault(
            &ctx.accounts.token_program.to_account_info(),
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.xcav_mint.to_account_info(),
            &ctx.accounts.voter_token.to_account_info(),
            &ctx.accounts.config.to_account_info(),
            ctx.accounts.config.bump,
            power,
            ctx.accounts.xcav_mint.decimals,
        )?;
    }

    emit!(VotingTokenUnlocked {
        proposal_id: ctx.accounts.vote_record.proposal_id,
        voter: ctx.accounts.voter.key(),
        power,
    });
    // `close = voter` returns the record's rent.
    Ok(())
}

/// Close a region state that can no longer progress so the region can be
/// proposed again: either a rejected proposal, or a passed proposal the proposer
/// never claimed before the deadline (whose bond is refunded here). Permissionless;
/// the caller reclaims the state's rent as a cleanup incentive.
#[derive(Accounts)]
#[instruction(region_id: u16)]
pub struct ClearRegionState<'info> {
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
        close = cranker,
        seeds = [REGION_STATE_SEED, &region_id.to_le_bytes()],
        bump = region_state.bump,
    )]
    pub region_state: Box<Account<'info, RegionState>>,

    /// CHECK: the stale state's proposer; owner of the refund destination.
    #[account(address = region_state.proposer @ RegionsError::NotProposer)]
    pub proposer: UncheckedAccount<'info>,

    /// Any XCAV account the proposer owns; needed only when clearing a stale
    /// passed state, which refunds the bond into it. Not pinned to the ATA,
    /// so a re-owned one can't lock the region.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = proposer,
    )]
    pub proposer_token: Option<Box<InterfaceAccount<'info, TokenAccount>>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn clear_region_state_handler(ctx: Context<ClearRegionState>, _region_id: u16) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let status = ctx.accounts.region_state.status;

    let clearable = status == RegionStatus::Rejected
        || (status == RegionStatus::Passed && now >= ctx.accounts.region_state.claim_deadline);
    require!(clearable, RegionsError::NotClearable);

    // A passed state still holds the proposer's bond in the vault; refund it.
    if status == RegionStatus::Passed {
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
            ctx.accounts.region_state.deposit,
            ctx.accounts.xcav_mint.decimals,
        )?;
    }

    emit!(RegionStateCleared {
        region_id: ctx.accounts.region_state.region_id,
        status,
    });
    // `close = cranker` returns the state's rent.
    Ok(())
}

#[event]
pub struct VotingTokenUnlocked {
    pub proposal_id: u64,
    pub voter: Pubkey,
    pub power: u64,
}

#[event]
pub struct RegionStateCleared {
    pub region_id: u16,
    pub status: RegionStatus,
}
