use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, LAWYER_SEED, VAULT_SEED};
use crate::error::MarketplaceError;
use crate::state::{Config, Lawyer};
use crate::vault::{lock_to_vault, release_from_vault};

use xcavate_whitelist::state::{Role, RoleAccount};

/// Join the lawyer registry for a region. Lawyer-role only. The region must
/// exist (its PDA existing is the check), and the caller locks the configured
/// XCAV deposit. The sponsor fronts the registry entry's rent; the deposit
/// stays the lawyer's own stake. One registration per wallet: `init` fails
/// on a second call.
#[derive(Accounts)]
#[instruction(region_id: u16)]
pub struct RegisterLawyer<'info> {
    pub lawyer: Signer<'info>,

    /// The sponsor wallet fronting the registry entry's rent.
    #[account(mut, address = config.rent_sponsor @ MarketplaceError::NotRentSponsor)]
    pub payer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The caller's Lawyer role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            lawyer.key().as_ref(),
            &[Role::Lawyer.seed_byte()],
        ],
        bump = lawyer_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub lawyer_role: Box<Account<'info, RoleAccount>>,

    /// The region the lawyer registers for, owned by the regions program.
    #[account(
        seeds = [regions::REGION_SEED, &region_id.to_le_bytes()],
        bump = region.bump,
        seeds::program = regions::ID,
    )]
    pub region: Box<Account<'info, regions::state::Region>>,

    #[account(
        init,
        payer = payer,
        space = 8 + Lawyer::INIT_SPACE,
        seeds = [LAWYER_SEED, lawyer.key().as_ref()],
        bump,
    )]
    pub lawyer_account: Box<Account<'info, Lawyer>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ MarketplaceError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The lawyer's XCAV account the deposit is pulled from.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = lawyer,
    )]
    pub lawyer_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The protocol's XCAV vault.
    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        token::mint = config.xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn register_lawyer_handler(
    ctx: Context<RegisterLawyer>,
    region_id: u16,
    max_deposit: u64,
) -> Result<()> {
    let deposit = ctx.accounts.config.lawyer_deposit;
    // The deposit is read from live config; the caller caps what they are
    // willing to pay so an update can't reprice their signed transaction.
    require!(deposit <= max_deposit, MarketplaceError::DepositTooHigh);
    lock_to_vault(
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.lawyer_token.to_account_info(),
        &ctx.accounts.xcav_mint.to_account_info(),
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.lawyer.to_account_info(),
        deposit,
        ctx.accounts.xcav_mint.decimals,
    )?;

    let lawyer_account = &mut ctx.accounts.lawyer_account;
    lawyer_account.lawyer = ctx.accounts.lawyer.key();
    lawyer_account.region_id = region_id;
    lawyer_account.deposit = deposit;
    lawyer_account.active_cases = 0;
    lawyer_account.bump = ctx.bumps.lawyer_account;

    emit!(LawyerRegistered {
        lawyer: lawyer_account.lawyer,
        region_id,
        deposit,
    });
    Ok(())
}

/// Leave the lawyer registry. Blocked while the lawyer has active cases; the
/// deposit recorded at registration is returned, and the entry's rent goes
/// back to the sponsor that fronted it. Deliberately not role-gated: this is
/// a pure exit, and a lawyer whose role was revoked must still be able to
/// reclaim their deposit. The registry PDA seeded by the wallet proves who
/// the caller is.
#[derive(Accounts)]
pub struct UnregisterLawyer<'info> {
    pub lawyer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// CHECK: the sponsor wallet that fronted the entry's rent; gets it back
    /// as the entry closes.
    #[account(mut, address = config.rent_sponsor @ MarketplaceError::NotRentSponsor)]
    pub rent_sponsor: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_sponsor,
        seeds = [LAWYER_SEED, lawyer.key().as_ref()],
        bump = lawyer_account.bump,
        constraint = lawyer_account.active_cases == 0 @ MarketplaceError::LawyerStillActive,
    )]
    pub lawyer_account: Box<Account<'info, Lawyer>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ MarketplaceError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The lawyer's XCAV account the deposit is returned to.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = lawyer,
    )]
    pub lawyer_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The protocol's XCAV vault.
    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        token::mint = config.xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn unregister_lawyer_handler(ctx: Context<UnregisterLawyer>) -> Result<()> {
    release_from_vault(
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.xcav_mint.to_account_info(),
        &ctx.accounts.lawyer_token.to_account_info(),
        &ctx.accounts.config.to_account_info(),
        ctx.accounts.config.bump,
        ctx.accounts.lawyer_account.deposit,
        ctx.accounts.xcav_mint.decimals,
    )?;

    emit!(LawyerUnregistered {
        lawyer: ctx.accounts.lawyer.key(),
    });
    Ok(())
}

#[event]
pub struct LawyerRegistered {
    pub lawyer: Pubkey,
    pub region_id: u16,
    pub deposit: u64,
}

#[event]
pub struct LawyerUnregistered {
    pub lawyer: Pubkey,
}
