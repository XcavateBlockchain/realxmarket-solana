use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, PROPOSAL_SEED, REGION_SEED, REGION_STATE_SEED, VAULT_SEED};
use crate::error::RegionsError;
use crate::state::{
    operator_bond, Config, RegionProposal, RegionState, RegionStatus, MAX_REGION_NAME_LEN,
};
use crate::vault::lock_to_vault;

use xcavate_whitelist::state::{Role, RoleAccount};

/// Propose a new region. The caller must be a RegionalOperator, the region must
/// not already exist, and there must be no other open proposal for it. The
/// proposer picks the id and the name, and the vote decides whether the region
/// is legitimate. The proposer bonds 0.1% of the XCAV supply into the vault; it
/// is returned if the proposal is rejected and becomes the region's collateral
/// once claimed.
#[derive(Accounts)]
#[instruction(region_id: u16)]
pub struct ProposeNewRegion<'info> {
    #[account(mut)]
    pub proposer: Signer<'info>,

    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ RegionsError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The proposer's XCAV account the deposit is pulled from.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = proposer,
    )]
    pub proposer_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The protocol's XCAV vault.
    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        token::mint = config.xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The caller's RegionalOperator role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            proposer.key().as_ref(),
            &[Role::RegionalOperator.seed_byte()],
        ],
        bump = operator_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub operator_role: Box<Account<'info, RoleAccount>>,

    /// CHECK: canonical region PDA; must be empty, i.e. the region isn't created yet.
    #[account(
        seeds = [REGION_SEED, &region_id.to_le_bytes()],
        bump,
        constraint = region.data_is_empty() @ RegionsError::RegionAlreadyCreated,
    )]
    pub region: UncheckedAccount<'info>,

    #[account(
        init,
        payer = proposer,
        space = 8 + RegionState::INIT_SPACE,
        seeds = [REGION_STATE_SEED, &region_id.to_le_bytes()],
        bump,
    )]
    pub region_state: Box<Account<'info, RegionState>>,

    #[account(
        init,
        payer = proposer,
        space = 8 + RegionProposal::INIT_SPACE,
        seeds = [PROPOSAL_SEED, &config.proposal_counter.to_le_bytes()],
        bump,
    )]
    pub proposal: Box<Account<'info, RegionProposal>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn propose_new_region_handler(
    ctx: Context<ProposeNewRegion>,
    region_id: u16,
    name: String,
    max_deposit: u64,
) -> Result<()> {
    // Zero is reserved so an unset region id can never match a real one.
    require!(region_id != 0, RegionsError::InvalidRegion);
    require!(
        !name.is_empty() && name.len() <= MAX_REGION_NAME_LEN,
        RegionsError::InvalidRegionName
    );

    let clock = Clock::get()?;
    let proposal_id = ctx.accounts.config.proposal_counter;
    let deposit = operator_bond(ctx.accounts.xcav_mint.supply).ok_or(RegionsError::BondTooSmall)?;
    // The bond is computed from live supply; the caller caps what they are
    // willing to pay so state changes can't reprice their signed transaction.
    require!(deposit <= max_deposit, RegionsError::DepositTooHigh);
    let voting_period = ctx.accounts.config.voting_period;

    // Lock the proposer's XCAV bond in the vault.
    lock_to_vault(
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.proposer_token.to_account_info(),
        &ctx.accounts.xcav_mint.to_account_info(),
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.proposer.to_account_info(),
        deposit,
        ctx.accounts.xcav_mint.decimals,
    )?;

    let proposal = &mut ctx.accounts.proposal;
    proposal.proposal_id = proposal_id;
    proposal.proposer = ctx.accounts.proposer.key();
    proposal.region_id = region_id;
    proposal.created_at = clock.unix_timestamp;
    proposal.expiry = clock.unix_timestamp.saturating_add(voting_period);
    proposal.vote_cutoff = proposal
        .expiry
        .checked_sub(ctx.accounts.config.min_vote_hold)
        .ok_or(RegionsError::Overflow)?;
    proposal.yes_power = 0;
    proposal.no_power = 0;
    proposal.abstain_power = 0;
    proposal.bump = ctx.bumps.proposal;

    let region_state = &mut ctx.accounts.region_state;
    region_state.region_id = region_id;
    region_state.name = name.clone();
    region_state.status = RegionStatus::Proposing;
    region_state.proposal_id = proposal_id;
    region_state.proposer = ctx.accounts.proposer.key();
    region_state.deposit = deposit;
    region_state.claim_deadline = 0;
    region_state.bump = ctx.bumps.region_state;

    ctx.accounts.config.proposal_counter =
        proposal_id.checked_add(1).ok_or(RegionsError::Overflow)?;

    emit!(RegionProposed {
        region_id,
        name,
        proposal_id,
        proposer: ctx.accounts.proposer.key(),
        expiry: proposal.expiry,
    });
    Ok(())
}

#[event]
pub struct RegionProposed {
    pub region_id: u16,
    pub name: String,
    pub proposal_id: u64,
    pub proposer: Pubkey,
    pub expiry: i64,
}
