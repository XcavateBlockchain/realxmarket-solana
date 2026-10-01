use anchor_lang::prelude::*;

#[error_code]
pub enum HubError {
    #[msg("Signer is not the program upgrade authority")]
    NotUpgradeAuthority,
    #[msg("Invalid hub configuration")]
    InvalidConfig,
    #[msg("Signer is not the hub operator")]
    NotOperator,
    #[msg("Signer is not the current region operator")]
    NotRegionOwner,
    #[msg("The role assignment is not compliant")]
    NotCompliant,
    #[msg("Signer is not the configured verifier")]
    NotVerifier,
    #[msg("An operator cannot verify their own hub")]
    SelfReview,
    #[msg("Hub is not in the required phase")]
    InvalidStatus,
    #[msg("Invalid hub metadata or sale terms")]
    InvalidProposal,
    #[msg("The review does not match the submitted revision and hash")]
    ReviewMismatch,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Mint is not the configured mint")]
    InvalidMint,
    #[msg("XCAV and payment mints must not have freeze authorities")]
    UnsupportedMintAuthority,
    #[msg("Bond exceeds the caller's maximum")]
    BondTooHigh,
    #[msg("Purchase amount must be positive and within the remaining supply")]
    InvalidAmount,
    #[msg("Purchase cost exceeds the caller's maximum")]
    CostTooHigh,
    #[msg("Sale window has closed")]
    SaleExpired,
    #[msg("Sale has not reached its deadline")]
    SaleStillOpen,
    #[msg("Position belongs to another buyer or hub")]
    InvalidPosition,
    #[msg("Position has already been settled")]
    AlreadySettled,
    #[msg("No purchase to settle")]
    EmptyPosition,
    #[msg("Bond has already been refunded")]
    BondAlreadyRefunded,
    #[msg("Paid purchases cannot be converted into unpaid reservations")]
    PurchasesOutstanding,
    #[msg("The wallet balance does not cover its unpaid hub reservations")]
    ReservationBalanceTooLow,
    #[msg("Reservation is bound to another payment account")]
    ReservationAccountMismatch,
    #[msg("Reservation has not reached its cleanup deadline")]
    ReservationStillActive,
}
