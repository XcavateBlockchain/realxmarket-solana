use anchor_lang::prelude::*;

#[error_code]
pub enum RegionsError {
    /// The signer is not the configured authority.
    #[msg("Signer is not the authority")]
    NotAuthority,
    /// The supplied governance parameters are invalid.
    #[msg("Invalid governance parameters")]
    InvalidConfig,
    /// The supplied mint is not the configured XCAV mint.
    #[msg("Invalid XCAV mint")]
    InvalidMint,
    /// The region id is zero, which is reserved.
    #[msg("Region id must be nonzero")]
    InvalidRegion,
    /// A region with this id already exists.
    #[msg("Region already created")]
    RegionAlreadyCreated,
    /// The voting amount is below the configured minimum.
    #[msg("Voting amount is below the minimum")]
    BelowMinimumVotingAmount,
    /// The proposal's voting window has closed.
    #[msg("Voting window has closed")]
    ProposalExpired,
    /// The proposal is not in the voting phase.
    #[msg("Region is not in the proposing phase")]
    NotProposing,
    /// The voting window has not yet closed.
    #[msg("Voting is still ongoing")]
    VotingStillOngoing,
    /// The XCAV supply is too small to produce a positive operator bond.
    #[msg("XCAV supply too small to bond")]
    BondTooSmall,
    /// The region proposal has not passed, so it can't be claimed.
    #[msg("Region proposal has not passed")]
    RegionNotPassed,
    /// The window to claim a passed region has closed.
    #[msg("Claim window has closed")]
    ClaimWindowClosed,
    /// The caller is not the proposer of this region.
    #[msg("Caller is not the region proposer")]
    NotProposer,
    /// The region state is not in a clearable (rejected/stale-passed) state.
    #[msg("Region state is not clearable")]
    NotClearable,
    /// The caller is not the region's operator.
    #[msg("Caller is not the region operator")]
    NotRegionOwner,
    /// The region's operator cannot be changed yet.
    #[msg("Region operator cannot be changed yet")]
    RegionOwnerCantBeChanged,
    /// An earlier (or equal) owner change is already scheduled.
    #[msg("An owner change is already scheduled")]
    OwnerChangeAlreadyScheduled,
    /// Arithmetic overflow.
    #[msg("Arithmetic overflow")]
    Overflow,
    /// The mint carries a token extension the vault accounting cannot support.
    #[msg("Unsupported token extension on mint")]
    UnsupportedMintExtension,
    /// The signer is not the program's upgrade authority.
    #[msg("Signer is not the program upgrade authority")]
    NotUpgradeAuthority,
    /// The signer does not match the pending authority proposal.
    #[msg("Signer is not the pending authority")]
    NotPendingAuthority,
    /// The listing duration is zero or above the configured maximum.
    #[msg("Invalid listing duration")]
    InvalidListingDuration,
    /// The tax is above the configured maximum.
    #[msg("Tax is above the maximum")]
    TaxTooHigh,
    /// A seller or buyer fee is above the configured maximum.
    #[msg("Fee is above the maximum")]
    FeeTooHigh,
    /// The postcode is empty, too long, or not uppercase alphanumeric ASCII.
    #[msg("Invalid postcode")]
    InvalidPostcode,
    /// The mint has an authority that could lock vaulted funds.
    #[msg("Unsupported mint authority")]
    UnsupportedMintAuthority,
    /// The computed deposit is above the caller's stated maximum.
    #[msg("Deposit exceeds the caller's maximum")]
    DepositTooHigh,
    /// The operator seat is open; locations are locked until it is filled.
    #[msg("Operator seat is open")]
    SeatOpen,
    /// The vote landed inside the minimum hold window before expiry.
    #[msg("Too close to the proposal expiry to vote")]
    VoteTooLate,
    /// The region name is empty or too long.
    #[msg("Invalid region name")]
    InvalidRegionName,
    /// A refund is due but no token account for the recipient was passed.
    #[msg("Refund token account missing")]
    RefundAccountMissing,
}
