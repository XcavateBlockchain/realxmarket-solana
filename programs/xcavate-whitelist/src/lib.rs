pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("7TrzjKpdrEhnfhxuw8tWdH1sjxadazscsG5HXCDPLmaY");

/// Roles and compliance registry for the realXmarket protocol.
///
/// Two separate registries. Roles say what an address may do, and are read as
/// the `RoleAccount` PDA (`["role", user, role.seed_byte()]`), whose existence
/// is the grant. Compliance says whether an address is cleared to move money,
/// and is read as the `Compliance` PDA (`["compliance", user]`), which carries
/// a screening verdict and an expiry.
#[program]
pub mod xcavate_whitelist {
    use super::*;

    /// Initialize the singleton config; sets sudo authority to the signer.
    pub fn initialize_config(ctx: Context<InitializeConfig>) -> Result<()> {
        initialize::handler(ctx)
    }

    /// Propose a new sudo authority (two-step handover). Current-authority-only.
    pub fn update_authority(ctx: Context<UpdateAuthority>, new_authority: Pubkey) -> Result<()> {
        initialize::update_authority_handler(ctx, new_authority)
    }

    /// Complete the authority handover. Signed by the pending authority.
    pub fn accept_authority(ctx: Context<AcceptAuthority>) -> Result<()> {
        initialize::accept_authority_handler(ctx)
    }

    /// Register a whitelist admin. Sudo-only.
    pub fn add_admin(ctx: Context<AddAdmin>) -> Result<()> {
        admin::add_admin_handler(ctx)
    }

    /// Remove a whitelist admin. Sudo-only.
    pub fn remove_admin(ctx: Context<RemoveAdmin>, admin_key: Pubkey) -> Result<()> {
        admin::remove_admin_handler(ctx, admin_key)
    }

    /// Assign a role to a user. Admin-only.
    pub fn assign_role(ctx: Context<AssignRole>, role: Role) -> Result<()> {
        role::assign_role_handler(ctx, role)
    }

    /// Remove a role from a user. Admin-only.
    pub fn remove_role(ctx: Context<RemoveRole>, role: Role) -> Result<()> {
        role::remove_role_handler(ctx, role)
    }

    /// Give up one's own role. Signed by the role holder.
    pub fn renounce_role(ctx: Context<RenounceRole>, role: Role) -> Result<()> {
        role::renounce_role_handler(ctx, role)
    }

    /// Record a wallet's screening outcome, creating the record on first use.
    /// Admin-only.
    pub fn set_compliance(
        ctx: Context<SetCompliance>,
        status: ComplianceStatus,
        expires_at: i64,
    ) -> Result<()> {
        compliance::set_compliance_handler(ctx, status, expires_at)
    }

    /// Delete a wallet's compliance record. Admin-only.
    pub fn remove_compliance(ctx: Context<RemoveCompliance>) -> Result<()> {
        compliance::remove_compliance_handler(ctx)
    }
}
