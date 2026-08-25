use anchor_lang::prelude::*;

use crate::constants::{ADMIN_SEED, ROLE_SEED};
use crate::error::WhitelistError;
use crate::state::{Admin, Role, RoleAccount};

/// Grant a role to a user. Admin-only. Says nothing about screening:
/// that is `set_compliance`, and the money gates want both.
#[derive(Accounts)]
#[instruction(role: Role)]
pub struct AssignRole<'info> {
    #[account(mut)]
    pub admin_signer: Signer<'info>,

    /// Proves `admin_signer` is a registered admin (PDA must exist).
    #[account(
        seeds = [ADMIN_SEED, admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,

    /// CHECK: the user receiving the role; used as identity / PDA seed only.
    pub user: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin_signer,
        space = 8 + RoleAccount::INIT_SPACE,
        seeds = [ROLE_SEED, user.key().as_ref(), &[role.seed_byte()]],
        bump,
    )]
    pub role_account: Account<'info, RoleAccount>,

    pub system_program: Program<'info, System>,
}

pub fn assign_role_handler(ctx: Context<AssignRole>, role: Role) -> Result<()> {
    let role_account = &mut ctx.accounts.role_account;
    role_account.user = ctx.accounts.user.key();
    role_account.role = role;
    role_account.rent_payer = ctx.accounts.admin_signer.key();
    role_account.bump = ctx.bumps.role_account;

    emit!(RoleAssigned {
        user: role_account.user,
        role
    });
    Ok(())
}

/// Revoke a role entirely. Admin-only; the account rent goes back to whoever
/// paid it at assignment, not necessarily the removing admin.
#[derive(Accounts)]
#[instruction(role: Role)]
pub struct RemoveRole<'info> {
    pub admin_signer: Signer<'info>,

    #[account(
        seeds = [ADMIN_SEED, admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,

    /// CHECK: the user losing the role; used as PDA seed only.
    pub user: UncheckedAccount<'info>,

    /// CHECK: rent destination, fixed to whoever paid at assignment.
    #[account(
        mut,
        address = role_account.rent_payer @ WhitelistError::WrongRentPayer,
    )]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [ROLE_SEED, user.key().as_ref(), &[role.seed_byte()]],
        bump = role_account.bump,
    )]
    pub role_account: Account<'info, RoleAccount>,
}

pub fn remove_role_handler(ctx: Context<RemoveRole>, role: Role) -> Result<()> {
    emit!(RoleRemoved {
        user: ctx.accounts.user.key(),
        role
    });
    Ok(())
}

/// Give up one's own role. Holder-signed; the account rent goes back to the
/// admin who paid it at assignment.
#[derive(Accounts)]
#[instruction(role: Role)]
pub struct RenounceRole<'info> {
    pub user: Signer<'info>,

    /// CHECK: rent destination, fixed to whoever paid at assignment.
    #[account(
        mut,
        address = role_account.rent_payer @ WhitelistError::WrongRentPayer,
    )]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [ROLE_SEED, user.key().as_ref(), &[role.seed_byte()]],
        bump = role_account.bump,
    )]
    pub role_account: Account<'info, RoleAccount>,
}

pub fn renounce_role_handler(ctx: Context<RenounceRole>, role: Role) -> Result<()> {
    emit!(RoleRemoved {
        user: ctx.accounts.user.key(),
        role
    });
    Ok(())
}

#[event]
pub struct RoleAssigned {
    pub user: Pubkey,
    pub role: Role,
}

#[event]
pub struct RoleRemoved {
    pub user: Pubkey,
    pub role: Role,
}
