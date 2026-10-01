use anchor_lang::prelude::*;
use anchor_spl::token::Mint;

use crate::constants::CONFIG_SEED;
use crate::error::HubError;
use crate::state::Config;

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ConfigParams {
    pub verifier: Pubkey,
    pub bond_amount: u64,
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ HubError::NotUpgradeAuthority)]
    pub program: Program<'info, crate::program::Realxhub>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ HubError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,
    #[account(init, payer = authority, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Account<'info, Config>,
    pub xcav_mint: Account<'info, Mint>,
    pub payment_mint: Account<'info, Mint>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitializeConfig>, params: ConfigParams) -> Result<()> {
    require!(
        params.verifier != Pubkey::default() && params.bond_amount > 0,
        HubError::InvalidConfig
    );
    require!(
        ctx.accounts.xcav_mint.key() != ctx.accounts.payment_mint.key(),
        HubError::InvalidConfig
    );
    // Classic SPL mints keep escrow accounting exact. A freeze authority could
    // otherwise prevent buyers from recovering their deposits.
    require!(
        ctx.accounts.xcav_mint.freeze_authority.is_none()
            && ctx.accounts.payment_mint.freeze_authority.is_none(),
        HubError::UnsupportedMintAuthority
    );
    let config = &mut ctx.accounts.config;
    config.authority = ctx.accounts.authority.key();
    config.verifier = params.verifier;
    config.xcav_mint = ctx.accounts.xcav_mint.key();
    config.payment_mint = ctx.accounts.payment_mint.key();
    config.bond_amount = params.bond_amount;
    config.next_hub_id = 0;
    config.bump = ctx.bumps.config;
    emit!(ConfigInitialized {
        authority: config.authority,
        verifier: config.verifier
    });
    Ok(())
}

#[event]
pub struct ConfigInitialized {
    pub authority: Pubkey,
    pub verifier: Pubkey,
}
