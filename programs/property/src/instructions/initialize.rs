use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, VAULT_SEED};
use crate::error::PropertyError;
use crate::state::Config;

/// Protocol parameters for the property program.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ConfigParams {
    pub treasury: Pubkey,
    pub rent_sponsor: Pubkey,
    pub agent_deposit: u64,
    pub agent_voting_time: i64,
    pub min_voting_quorum_bps: u16,
    pub agent_notice_period: i64,
    pub proposal_voting_time: i64,
    pub low_proposal: u64,
    pub high_proposal: u64,
    pub high_threshold_bps: u16,
    pub auto_approval_cooldown: i64,
    pub challenge_deposit: u64,
    pub agent_slash_amount: u64,
}

impl ConfigParams {
    fn validate(&self) -> Result<()> {
        require!(
            self.treasury != Pubkey::default() && self.rent_sponsor != Pubkey::default(),
            PropertyError::InvalidConfig
        );
        require!(self.agent_deposit > 0, PropertyError::InvalidConfig);
        require!(
            self.agent_voting_time > 0
                && self.agent_notice_period > 0
                && self.proposal_voting_time > 0
                && self.auto_approval_cooldown > 0,
            PropertyError::InvalidConfig
        );
        require!(
            self.min_voting_quorum_bps <= 10_000 && self.high_threshold_bps <= 10_000,
            PropertyError::InvalidConfig
        );
        require!(
            self.low_proposal <= self.high_proposal,
            PropertyError::InvalidConfig
        );
        require!(
            self.challenge_deposit > 0 && self.agent_slash_amount > 0,
            PropertyError::InvalidConfig
        );
        Ok(())
    }

    fn apply(&self, config: &mut Config) {
        config.treasury = self.treasury;
        config.rent_sponsor = self.rent_sponsor;
        config.agent_deposit = self.agent_deposit;
        config.agent_voting_time = self.agent_voting_time;
        config.min_voting_quorum_bps = self.min_voting_quorum_bps;
        config.agent_notice_period = self.agent_notice_period;
        config.proposal_voting_time = self.proposal_voting_time;
        config.low_proposal = self.low_proposal;
        config.high_proposal = self.high_proposal;
        config.high_threshold_bps = self.high_threshold_bps;
        config.auto_approval_cooldown = self.auto_approval_cooldown;
        config.challenge_deposit = self.challenge_deposit;
        config.agent_slash_amount = self.agent_slash_amount;
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
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ PropertyError::NotUpgradeAuthority)]
    pub program: Program<'info, crate::program::Property>,

    /// The program's upgrade authority must be the initializing signer.
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ PropertyError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,

    #[account(
        init,
        payer = authority,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Box<Account<'info, Config>>,

    /// The XCAV mint agent deposits are paid in.
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The program's XCAV vault, owned by the config PDA. Holds agent
    /// deposits.
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
    config.bump = ctx.bumps.config;

    emit!(ConfigInitialized {
        authority: config.authority,
        xcav_mint: config.xcav_mint,
        treasury: config.treasury,
    });
    Ok(())
}

/// Update the protocol parameters. Authority-only. The mint is fixed at
/// initialization.
#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ PropertyError::NotAuthority,
    )]
    pub config: Box<Account<'info, Config>>,
}

pub fn update_config_handler(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
    params.validate()?;
    let config = &mut ctx.accounts.config;
    params.apply(config);

    emit!(ConfigUpdated {
        treasury: config.treasury,
        agent_deposit: config.agent_deposit,
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
        has_one = authority @ PropertyError::NotAuthority,
    )]
    pub config: Box<Account<'info, Config>>,
}

pub fn update_authority_handler(
    ctx: Context<UpdateAuthority>,
    new_authority: Pubkey,
) -> Result<()> {
    require!(
        new_authority != Pubkey::default(),
        PropertyError::InvalidConfig
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
        constraint = config.pending_authority == Some(new_authority.key()) @ PropertyError::NotPendingAuthority,
    )]
    pub config: Box<Account<'info, Config>>,
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
    pub treasury: Pubkey,
}

#[event]
pub struct ConfigUpdated {
    pub treasury: Pubkey,
    pub agent_deposit: u64,
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
