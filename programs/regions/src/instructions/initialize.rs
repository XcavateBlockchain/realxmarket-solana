use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, VAULT_SEED};
use crate::error::RegionsError;
use crate::state::Config;

/// Governance parameters for the regions program.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ConfigParams {
    pub minimum_voting_amount: u64,
    pub voting_period: i64,
    pub owner_change_period: i64,
    pub threshold_bps: u16,
    pub quorum: u64,
    pub notice_period: i64,
    pub min_vote_hold: i64,
    pub max_listing_duration: i64,
    pub max_tax_bps: u16,
    pub max_fee_bps: u16,
    pub location_deposit: u64,
}

impl ConfigParams {
    /// Reject obviously broken parameters up front. A threshold above 100% or a
    /// zero quorum/period would silently make every future proposal unwinnable,
    /// so it's worth catching at the point the authority sets them.
    fn validate(&self) -> Result<()> {
        require!(
            self.threshold_bps > 0 && self.threshold_bps <= 10_000,
            RegionsError::InvalidConfig
        );
        require!(self.quorum > 0, RegionsError::InvalidConfig);
        require!(
            self.voting_period > 0 && self.owner_change_period > 0,
            RegionsError::InvalidConfig
        );
        require!(self.minimum_voting_amount > 0, RegionsError::InvalidConfig);
        require!(self.notice_period > 0, RegionsError::InvalidConfig);
        // Zero would silently switch the anti-borrow protection off, and a
        // hold as long as the window would make every proposal unvotable.
        require!(
            self.min_vote_hold > 0 && self.min_vote_hold < self.voting_period,
            RegionsError::InvalidConfig
        );
        require!(self.max_listing_duration > 0, RegionsError::InvalidConfig);
        require!(self.max_tax_bps <= 10_000, RegionsError::InvalidConfig);
        // A fee at 100% would pay a seller nothing, so cap strictly below.
        require!(self.max_fee_bps < 10_000, RegionsError::InvalidConfig);
        require!(self.location_deposit > 0, RegionsError::InvalidConfig);
        Ok(())
    }

    fn apply(&self, config: &mut Config) {
        config.minimum_voting_amount = self.minimum_voting_amount;
        config.voting_period = self.voting_period;
        config.owner_change_period = self.owner_change_period;
        config.threshold_bps = self.threshold_bps;
        config.quorum = self.quorum;
        config.notice_period = self.notice_period;
        config.min_vote_hold = self.min_vote_hold;
        config.max_listing_duration = self.max_listing_duration;
        config.max_tax_bps = self.max_tax_bps;
        config.max_fee_bps = self.max_fee_bps;
        config.location_deposit = self.location_deposit;
    }
}

/// Creates the singleton config and sets the authority to the signer. Only
/// the program's upgrade authority can call this, so the config can't be
/// claimed by a front-runner between deploy and initialization.
#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    /// This program's executable account, tying `program_data` to it.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ RegionsError::NotUpgradeAuthority)]
    pub program: Program<'info, crate::program::Regions>,

    /// The program's upgrade authority must be the initializing signer.
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ RegionsError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,

    #[account(
        init,
        payer = authority,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Account<'info, Config>,

    /// The XCAV governance mint the protocol stakes.
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The protocol's XCAV vault, owned by the config PDA.
    #[account(
        init,
        payer = authority,
        seeds = [VAULT_SEED],
        bump,
        token::mint = xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitializeConfig>, params: ConfigParams) -> Result<()> {
    params.validate()?;
    crate::mint_guard::require_supported_mint(&ctx.accounts.xcav_mint.to_account_info())?;

    let config = &mut ctx.accounts.config;
    config.authority = ctx.accounts.authority.key();
    config.pending_authority = None;
    config.xcav_mint = ctx.accounts.xcav_mint.key();
    params.apply(config);
    config.proposal_counter = 0;
    config.bump = ctx.bumps.config;

    emit!(ConfigInitialized {
        authority: config.authority,
        xcav_mint: config.xcav_mint,
    });
    Ok(())
}

/// Update the governance parameters. Authority-only. Each proposal snapshots
/// its deposit and expiry, but threshold and quorum are read live at
/// finalization, so a change reaches proposals already in flight. The mint is
/// fixed at initialization.
#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ RegionsError::NotAuthority,
    )]
    pub config: Box<Account<'info, Config>>,
}

pub fn update_config_handler(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
    params.validate()?;
    let config = &mut ctx.accounts.config;
    params.apply(config);

    emit!(ConfigUpdated {
        minimum_voting_amount: config.minimum_voting_amount,
        quorum: config.quorum,
        threshold_bps: config.threshold_bps,
    });
    Ok(())
}

/// Proposes a new authority. Current-authority-only. The handover only
/// completes when the proposed key signs `accept_authority`, so a typo'd
/// address can't brick parameter management. Proposing again overwrites any
/// earlier pending proposal (which also serves as cancellation).
#[derive(Accounts)]
pub struct UpdateAuthority<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ RegionsError::NotAuthority,
    )]
    pub config: Account<'info, Config>,
}

pub fn update_authority_handler(
    ctx: Context<UpdateAuthority>,
    new_authority: Pubkey,
) -> Result<()> {
    require!(
        new_authority != Pubkey::default(),
        RegionsError::InvalidConfig
    );
    let config = &mut ctx.accounts.config;
    config.pending_authority = Some(new_authority);

    emit!(AuthorityUpdateProposed {
        authority: config.authority,
        pending_authority: new_authority,
    });
    Ok(())
}

/// Completes an authority handover. Signed by the pending authority.
#[derive(Accounts)]
pub struct AcceptAuthority<'info> {
    pub new_authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = config.pending_authority == Some(new_authority.key()) @ RegionsError::NotPendingAuthority,
    )]
    pub config: Account<'info, Config>,
}

pub fn accept_authority_handler(ctx: Context<AcceptAuthority>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    let old_authority = config.authority;
    config.authority = ctx.accounts.new_authority.key();
    config.pending_authority = None;

    emit!(AuthorityUpdated {
        old_authority,
        new_authority: config.authority,
    });
    Ok(())
}

#[event]
pub struct ConfigInitialized {
    pub authority: Pubkey,
    pub xcav_mint: Pubkey,
}

#[event]
pub struct ConfigUpdated {
    pub minimum_voting_amount: u64,
    pub quorum: u64,
    pub threshold_bps: u16,
}

#[event]
pub struct AuthorityUpdateProposed {
    pub authority: Pubkey,
    pub pending_authority: Pubkey,
}

#[event]
pub struct AuthorityUpdated {
    pub old_authority: Pubkey,
    pub new_authority: Pubkey,
}
