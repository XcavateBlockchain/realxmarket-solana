use anchor_lang::prelude::*;

/// The recognised regions and their on-chain ids.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionIdentifier {
    England,
    France,
    Japan,
    India,
}

impl RegionIdentifier {
    /// Returns the identifier for a raw id, or `None` if it isn't recognised.
    pub fn from_code(code: u16) -> Option<Self> {
        match code {
            1 => Some(RegionIdentifier::England),
            2 => Some(RegionIdentifier::France),
            3 => Some(RegionIdentifier::Japan),
            4 => Some(RegionIdentifier::India),
            _ => None,
        }
    }
}

/// How a voter can vote on a proposal.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vote {
    Yes,
    No,
    Abstain,
}

/// Singleton config holding governance parameters and the authority.
#[account]
#[derive(InitSpace)]
pub struct Config {
    /// Authority allowed to update parameters.
    pub authority: Pubkey,
    /// Proposed replacement authority; takes over via `accept_authority`.
    /// Two-step so a typo'd address can't brick parameter management.
    pub pending_authority: Option<Pubkey>,
    /// The XCAV governance mint staked for proposals, votes, and operator bonds.
    pub xcav_mint: Pubkey,
    /// Minimum XCAV a voter must lock to vote.
    pub minimum_voting_amount: u64,
    /// Seconds a proposal stays open for voting.
    pub voting_period: i64,
    /// The operator's term: minimum time before a region's seat can be claimed
    /// by someone else, and how long a passed proposal has to be claimed.
    pub owner_change_period: i64,
    /// Approval threshold in basis points (yes / (yes + no)).
    pub threshold_bps: u16,
    /// Minimum total voting power for a proposal to be valid.
    pub quorum: u64,
    /// Notice an operator must give before resigning (seconds).
    pub notice_period: i64,
    /// Seconds before a proposal's expiry after which new votes are rejected,
    /// so every vote's lock lasts at least this long.
    pub min_vote_hold: i64,
    /// Longest listing duration an operator may set for their region (seconds).
    pub max_listing_duration: i64,
    /// Highest property tax an operator may set, in basis points.
    pub max_tax_bps: u16,
    /// Highest seller or buyer fee an operator may set, in basis points.
    pub max_fee_bps: u16,
    /// XCAV an operator locks per registered location, on top of their bond.
    pub location_deposit: u64,
    /// Monotonic id for the next proposal.
    pub proposal_counter: u64,
    pub bump: u8,
}

/// A created region and its operator.
#[account]
#[derive(InitSpace)]
pub struct Region {
    pub region_id: u16,
    pub owner: Pubkey,
    /// The operator's bonded XCAV plus the location deposits, held in the
    /// vault. Returned when the seat changes hands.
    pub collateral: u64,
    /// The part of `collateral` backing the registered locations: the sum of
    /// every `Location.deposit`. Takeovers and removals move these recorded
    /// amounts, so the seat's bond stays exactly what was locked no matter
    /// how the configured deposit changes in between.
    pub location_collateral: u64,
    pub next_owner_change: i64,
    /// Seconds a property listing in this region stays active.
    pub listing_duration: i64,
    /// Property sale tax for this region, in basis points.
    pub tax_bps: u16,
    /// Fee on the developer's sale proceeds, in basis points.
    pub seller_fee_bps: u16,
    /// Fee a buyer pays on top of the share price, in basis points.
    pub buyer_fee_bps: u16,
    /// Number of registered locations (each backed by a location deposit).
    pub location_count: u32,
    pub bump: u8,
}

/// An open proposal to create a region, with its running vote tally. The
/// proposer's bond is held in the vault and tracked on the region's
/// `RegionState`; this account only carries the vote.
#[account]
#[derive(InitSpace)]
pub struct RegionProposal {
    pub proposal_id: u64,
    pub proposer: Pubkey,
    pub region_id: u16,
    pub created_at: i64,
    pub expiry: i64,
    /// Votes are rejected from this moment on, so every vote's lock lasts at
    /// least the configured minimum hold. Snapshotted at propose time; a
    /// config change never shrinks an open proposal's window.
    pub vote_cutoff: i64,
    pub yes_power: u64,
    pub no_power: u64,
    pub abstain_power: u64,
    pub bump: u8,
}

/// Where a region is in its lifecycle before it is created.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionStatus {
    /// A proposal is open for voting.
    Proposing,
    /// The proposal passed; the proposer can now claim the region (create it).
    Passed,
    /// The proposal was rejected; the state lingers until cleared so the
    /// region can be proposed again.
    Rejected,
}

/// One account per region tracking its pre-creation lifecycle. Created when a
/// region is proposed and kept (transitioning Proposing -> Passed/Rejected)
/// until the region is created or the state is cleared. Having exactly one of
/// these per region is what enforces a single in-flight proposal at a time.
#[account]
#[derive(InitSpace)]
pub struct RegionState {
    pub region_id: u16,
    pub status: RegionStatus,
    /// The proposal this region is voting on.
    pub proposal_id: u64,
    /// The proposer; on a pass they claim the region, on a stale pass they are
    /// refunded the bond.
    pub proposer: Pubkey,
    /// The proposer's bonded XCAV, held in the vault. Becomes the region's
    /// collateral on claim, or is refunded on reject / stale pass.
    pub deposit: u64,
    /// Once `Passed`, the deadline for the proposer to claim; after it anyone
    /// can clear the state and the bond is returned.
    pub claim_deadline: i64,
    pub bump: u8,
}

/// One voter's vote on a proposal. The locked voting power (XCAV) is held in the
/// vault and returned when the voter unlocks. `expiry` is copied from the
/// proposal so unlocking never needs the proposal account (which may already be
/// closed by finalization).
#[account]
#[derive(InitSpace)]
pub struct VoteRecord {
    pub proposal_id: u64,
    pub voter: Pubkey,
    pub region_id: u16,
    pub vote: Vote,
    pub power: u64,
    pub expiry: i64,
    pub bump: u8,
}

/// A registered postcode within a region. Existence is the point: the
/// marketplace requires a listing's location PDA to exist for its region.
#[account]
#[derive(InitSpace)]
pub struct Location {
    pub region_id: u16,
    /// The postcode, as seeded (uppercase ASCII, no spaces).
    #[max_len(POSTCODE_MAX_LEN)]
    pub postcode: Vec<u8>,
    /// The XCAV locked when the location was registered. Released on removal,
    /// even if the configured deposit has changed since.
    pub deposit: u64,
    pub bump: u8,
}

/// Longest postcode accepted for a location.
pub const POSTCODE_MAX_LEN: usize = 10;

/// The XCAV bond an operator must lock to run a region: 0.1% of the current
/// supply. Returns `None` if the supply is too small to produce a positive bond.
pub fn operator_bond(xcav_supply: u64) -> Option<u64> {
    let bond = xcav_supply / 1_000;
    (bond > 0).then_some(bond)
}
