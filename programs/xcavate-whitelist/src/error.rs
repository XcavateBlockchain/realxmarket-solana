use anchor_lang::prelude::*;

#[error_code]
pub enum WhitelistError {
    /// The signer is not the configured sudo authority.
    #[msg("Signer is not the sudo authority")]
    NotAuthority,
    /// The expiry is negative, or in the past for a record meant to clear.
    #[msg("Invalid compliance expiry")]
    InvalidExpiry,
    /// The new authority cannot be the zero address.
    #[msg("Invalid authority address")]
    InvalidAuthority,
    /// The signer is not the program's upgrade authority.
    #[msg("Signer is not the program upgrade authority")]
    NotUpgradeAuthority,
    /// The signer is not the proposed pending authority.
    #[msg("Signer is not the pending authority")]
    NotPendingAuthority,
    /// The rent destination is not the account's recorded rent payer.
    #[msg("Wrong rent payer")]
    WrongRentPayer,
}
