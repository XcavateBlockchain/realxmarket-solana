use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, CPI_AUTH_SEED, VAULT_SEED};
use crate::error::MarketplaceError;
use crate::state::{
    Config, MAX_PAYMENT_DECIMALS, MAX_PAYMENT_MINTS, MAX_SHARE_SUPPLY, MIN_PAYMENT_DECIMALS,
};

/// Protocol parameters for the marketplace program.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ConfigParams {
    pub treasury: Pubkey,
    pub rent_collector: Pubkey,
    pub accepted_payment_mints: Vec<Pubkey>,
    pub listing_deposit: u64,
    pub lawyer_deposit: u64,
    pub min_property_shares: u32,
    pub max_property_shares: u32,
    pub operator_fee_share_bps: u16,
    pub max_ownership_bps: u16,
    pub claiming_time: i64,
    pub legal_process_time: i64,
    pub lawyer_voting_time: i64,
    pub min_voting_quorum_bps: u16,
}

impl ConfigParams {
    /// Reject obviously broken parameters up front. A fee above 100%, an empty
    /// payment-mint list, or a zero legal window would strand every listing
    /// that hits the affected path, so it's worth catching at the point the
    /// authority sets them.
    fn validate(&self) -> Result<()> {
        require!(
            self.treasury != Pubkey::default() && self.rent_collector != Pubkey::default(),
            MarketplaceError::InvalidConfig
        );
        require!(
            !self.accepted_payment_mints.is_empty()
                && self.accepted_payment_mints.len() <= MAX_PAYMENT_MINTS,
            MarketplaceError::InvalidConfig
        );
        // Few enough mints that the quadratic duplicate scan is fine.
        for (i, mint) in self.accepted_payment_mints.iter().enumerate() {
            require!(
                !self.accepted_payment_mints[..i].contains(mint),
                MarketplaceError::InvalidConfig
            );
        }
        require!(
            self.listing_deposit > 0 && self.lawyer_deposit > 0,
            MarketplaceError::InvalidConfig
        );
        require!(
            self.min_property_shares > 0
                && self.min_property_shares <= self.max_property_shares
                && self.max_property_shares <= MAX_SHARE_SUPPLY,
            MarketplaceError::InvalidConfig
        );
        require!(
            self.operator_fee_share_bps <= 10_000,
            MarketplaceError::InvalidConfig
        );
        require!(
            self.max_ownership_bps > 0 && self.max_ownership_bps <= 10_000,
            MarketplaceError::InvalidConfig
        );
        require!(
            self.claiming_time > 0 && self.legal_process_time > 0 && self.lawyer_voting_time > 0,
            MarketplaceError::InvalidConfig
        );
        require!(
            self.min_voting_quorum_bps > 0 && self.min_voting_quorum_bps <= 10_000,
            MarketplaceError::InvalidConfig
        );
        Ok(())
    }

    fn apply(&self, config: &mut Config) {
        config.treasury = self.treasury;
        config.rent_collector = self.rent_collector;
        config.accepted_payment_mints = self.accepted_payment_mints.clone();
        config.listing_deposit = self.listing_deposit;
        config.lawyer_deposit = self.lawyer_deposit;
        config.min_property_shares = self.min_property_shares;
        config.max_property_shares = self.max_property_shares;
        config.operator_fee_share_bps = self.operator_fee_share_bps;
        config.max_ownership_bps = self.max_ownership_bps;
        config.claiming_time = self.claiming_time;
        config.legal_process_time = self.legal_process_time;
        config.lawyer_voting_time = self.lawyer_voting_time;
        config.min_voting_quorum_bps = self.min_voting_quorum_bps;
    }
}

/// Each accepted payment mint must be passed as a remaining account, proving
/// it is a real mint the vault accounting supports; a typo'd or fee-bearing
/// entry would otherwise only surface once a buyer's funds hit it. Decimals
/// are bounded so price rescaling can neither floor a minimum-priced share to
/// zero nor overflow.
fn validate_payment_mints(mints: &[Pubkey], infos: &[AccountInfo]) -> Result<()> {
    require!(infos.len() == mints.len(), MarketplaceError::InvalidConfig);
    for (expected, info) in mints.iter().zip(infos) {
        require!(info.key == expected, MarketplaceError::InvalidConfig);
        let decimals = crate::mint_guard::require_supported_mint(info)?;
        require!(
            (MIN_PAYMENT_DECIMALS..=MAX_PAYMENT_DECIMALS).contains(&decimals),
            MarketplaceError::InvalidConfig
        );
    }
    Ok(())
}

/// Creates the singleton config and sets the authority to the signer. Only
/// the program's upgrade authority can call this, so the config can't be
/// claimed by a front-runner between deploy and initialization.
#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    /// This program's executable account, tying `program_data` to it.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ MarketplaceError::NotUpgradeAuthority)]
    pub program: Program<'info, crate::program::Marketplace>,

    /// The program's upgrade authority must be the initializing signer.
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ MarketplaceError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,

    #[account(
        init,
        payer = authority,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Box<Account<'info, Config>>,

    /// The XCAV mint deposits are paid in.
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The protocol's XCAV vault, owned by the config PDA. Holds
    /// listing and lawyer deposits.
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
    validate_payment_mints(&params.accepted_payment_mints, ctx.remaining_accounts)?;

    let config = &mut ctx.accounts.config;
    config.authority = ctx.accounts.authority.key();
    config.pending_authority = None;
    config.xcav_mint = ctx.accounts.xcav_mint.key();
    params.apply(config);
    config.next_listing_id = 0;
    config.next_share_listing_id = 0;
    config.cpi_auth_bump = Pubkey::find_program_address(&[CPI_AUTH_SEED], &crate::ID).1;
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
        has_one = authority @ MarketplaceError::NotAuthority,
    )]
    pub config: Box<Account<'info, Config>>,
}

pub fn update_config_handler(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
    params.validate()?;
    validate_payment_mints(&params.accepted_payment_mints, ctx.remaining_accounts)?;
    let config = &mut ctx.accounts.config;
    params.apply(config);

    emit!(ConfigUpdated {
        treasury: config.treasury,
        operator_fee_share_bps: config.operator_fee_share_bps,
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
        has_one = authority @ MarketplaceError::NotAuthority,
    )]
    pub config: Box<Account<'info, Config>>,
}

pub fn update_authority_handler(
    ctx: Context<UpdateAuthority>,
    new_authority: Pubkey,
) -> Result<()> {
    require!(
        new_authority != Pubkey::default(),
        MarketplaceError::InvalidConfig
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
        constraint = config.pending_authority == Some(new_authority.key()) @ MarketplaceError::NotPendingAuthority,
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
    pub operator_fee_share_bps: u16,
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
