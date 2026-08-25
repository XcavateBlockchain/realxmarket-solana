use anchor_lang::prelude::*;

use crate::constants::{ADMIN_SEED, COMPLIANCE_SEED};
use crate::error::WhitelistError;
use crate::state::{Admin, Compliance, ComplianceStatus};

/// Record a wallet's screening outcome, creating the account on first use.
/// Admin-only. Re-running it is how a record is renewed after re-screening or
/// flipped to `Blocked` on a sanctions hit.
#[derive(Accounts)]
pub struct SetCompliance<'info> {
    #[account(mut)]
    pub admin_signer: Signer<'info>,

    /// Proves `admin_signer` is a registered admin (PDA must exist).
    #[account(
        seeds = [ADMIN_SEED, admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,

    /// CHECK: the wallet being screened; used as identity / PDA seed only.
    pub user: UncheckedAccount<'info>,

    #[account(
        init_if_needed,
        payer = admin_signer,
        space = 8 + Compliance::INIT_SPACE,
        seeds = [COMPLIANCE_SEED, user.key().as_ref()],
        bump,
    )]
    pub compliance: Account<'info, Compliance>,

    pub system_program: Program<'info, System>,
}

pub fn set_compliance_handler(
    ctx: Context<SetCompliance>,
    status: ComplianceStatus,
    expires_at: i64,
) -> Result<()> {
    // A record that is already expired clears nobody, so it is never what the
    // caller meant. `Blocked` carries no expiry: it does not lapse.
    match status {
        ComplianceStatus::Cleared => require!(
            expires_at == 0 || expires_at > Clock::get()?.unix_timestamp,
            WhitelistError::InvalidExpiry
        ),
        ComplianceStatus::Blocked => require!(expires_at == 0, WhitelistError::InvalidExpiry),
    }

    let compliance = &mut ctx.accounts.compliance;
    // Only on creation, so a renewal doesn't move the refund to a second admin.
    if compliance.user == Pubkey::default() {
        compliance.user = ctx.accounts.user.key();
        compliance.rent_payer = ctx.accounts.admin_signer.key();
        compliance.bump = ctx.bumps.compliance;
    }
    compliance.status = status;
    compliance.expires_at = expires_at;

    emit!(ComplianceSet {
        user: compliance.user,
        status,
        expires_at,
    });
    Ok(())
}

/// Delete a wallet's compliance record. Admin-only; the rent goes back to
/// whoever paid it, not necessarily the removing admin.
///
/// This is for erasing a record entirely, say on a data-deletion request. To
/// stop a wallet transacting, set it `Blocked` instead: removing the account
/// only takes the wallet back to having never been screened.
#[derive(Accounts)]
pub struct RemoveCompliance<'info> {
    pub admin_signer: Signer<'info>,

    #[account(
        seeds = [ADMIN_SEED, admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,

    /// CHECK: the wallet losing its record; used as PDA seed only.
    pub user: UncheckedAccount<'info>,

    /// CHECK: rent destination, fixed to whoever paid at creation.
    #[account(
        mut,
        address = compliance.rent_payer @ WhitelistError::WrongRentPayer,
    )]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [COMPLIANCE_SEED, user.key().as_ref()],
        bump = compliance.bump,
    )]
    pub compliance: Account<'info, Compliance>,
}

pub fn remove_compliance_handler(ctx: Context<RemoveCompliance>) -> Result<()> {
    emit!(ComplianceRemoved {
        user: ctx.accounts.user.key(),
    });
    Ok(())
}

#[event]
pub struct ComplianceSet {
    pub user: Pubkey,
    pub status: ComplianceStatus,
    pub expires_at: i64,
}

#[event]
pub struct ComplianceRemoved {
    pub user: Pubkey,
}
