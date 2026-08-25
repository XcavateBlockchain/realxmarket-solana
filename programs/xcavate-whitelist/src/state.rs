use anchor_lang::prelude::*;

/// All roles recognised across the realXmarket protocol.
///
/// A role is app-level authorization, separate from KYC. Screening lives in
/// the per-wallet `Compliance` account, which the consuming programs check
/// alongside the role, so a role account tracks assignment only.
///
/// Do not reorder the variants: the serialized `RoleAccount.role` stores the
/// variant index, so a reorder reinterprets every existing assignment.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Manages a region: claims the operator seat, registers locations, sets
    /// listing duration and tax.
    RegionalOperator,
    /// Buys, claims, relists and votes on fractional property shares.
    RealEstateInvestor,
    /// Lists properties for fractional sale.
    RealEstateDeveloper,
    /// Represents the developer or SPV side of a property sale's legal process.
    Lawyer,
    /// Manages let properties and distributes rental income to share holders.
    LettingAgent,
    /// Confirms that the SPV for a sold-out property has been created.
    SpvConfirmation,
}

impl Role {
    /// Stable one-byte tag for PDA seeds. Explicit on purpose so the derivation
    /// doesn't shift if the enum is ever reordered.
    pub fn seed_byte(&self) -> u8 {
        match self {
            Role::RegionalOperator => 0,
            Role::RealEstateInvestor => 1,
            Role::RealEstateDeveloper => 2,
            Role::Lawyer => 3,
            Role::LettingAgent => 4,
            Role::SpvConfirmation => 5,
        }
    }
}

/// Screening outcome for a wallet. `Blocked` is a positive statement, not the
/// absence of a record: deleting the account only takes a wallet back to
/// never-screened, so a sanctions hit has to stay on file.
///
/// Do not reorder the variants: the serialized `Compliance.status` stores the
/// variant index.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ComplianceStatus {
    /// Passed KYC/AML, so gated actions are allowed until `expires_at`.
    Cleared,
    /// Failed screening or later sanctioned. Never allowed.
    Blocked,
}

/// One wallet's compliance standing, set by an admin after off-chain KYC/AML.
/// Held per wallet rather than per role: screening is a property of the person,
/// and a holder of two roles must not be able to carry two disagreeing
/// verdicts.
#[account]
#[derive(InitSpace)]
pub struct Compliance {
    pub user: Pubkey,
    pub status: ComplianceStatus,
    /// Unix seconds after which this stops clearing anyone, or 0 to never
    /// expire. Screening is continuous, so a date is what forces a re-check.
    pub expires_at: i64,
    /// Who paid the account's rent (the setting admin); teardown refunds them.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

impl Compliance {
    /// Whether this record currently clears the wallet for gated actions.
    pub fn is_live(&self) -> Result<bool> {
        Ok(self.status == ComplianceStatus::Cleared
            && (self.expires_at == 0 || self.expires_at > Clock::get()?.unix_timestamp))
    }
}

/// Singleton config holding the sudo authority that manages admins.
#[account]
#[derive(InitSpace)]
pub struct Config {
    /// Sudo authority allowed to add/remove admins.
    pub authority: Pubkey,
    /// Proposed replacement authority; takes over via `accept_authority`.
    /// Two-step so a typo'd address can't brick admin management.
    pub pending_authority: Option<Pubkey>,
    pub bump: u8,
}

/// Marks an address as a whitelist admin.
#[account]
#[derive(InitSpace)]
pub struct Admin {
    pub admin: Pubkey,
    pub bump: u8,
}

/// One (user, role) assignment. Its existence is the grant.
#[account]
#[derive(InitSpace)]
pub struct RoleAccount {
    pub user: Pubkey,
    pub role: Role,
    /// Who paid the account's rent (the assigning admin); every teardown
    /// refunds them, whoever triggers it.
    pub rent_payer: Pubkey,
    pub bump: u8,
}
