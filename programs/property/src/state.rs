use anchor_lang::prelude::*;

pub use regions::state::POSTCODE_MAX_LEN;

/// One agent covers at most this many locations; each one carries its own
/// deposit, so the bound also caps what a single registry entry can hold.
pub const MAX_AGENT_LOCATIONS: usize = 10;

#[account]
#[derive(InitSpace)]
pub struct Config {
    /// Authority allowed to update parameters.
    pub authority: Pubkey,
    /// Proposed replacement authority; takes over via `accept_authority`.
    /// Two-step so a typo'd address can't brick parameter management.
    pub pending_authority: Option<Pubkey>,
    /// The XCAV mint agent deposits are paid in.
    pub xcav_mint: Pubkey,
    /// Owner of the shared protocol treasury (a multisig on mainnet).
    /// Slashes are paid to this key's token accounts; the program never
    /// holds them.
    pub treasury: Pubkey,
    /// The sponsor wallet that fronts account rent for holders. Sponsored
    /// closes send their lamports here, not to the holder.
    pub rent_sponsor: Pubkey,
    /// XCAV an agent locks per location they register in.
    pub agent_deposit: u64,
    /// Seconds an agent election stays open once the first candidacy claims.
    pub agent_voting_time: i64,
    /// Share of a property's supply that must vote for an agent election to
    /// be valid, in basis points.
    pub min_voting_quorum_bps: u16,
    /// Seconds between an agent's resignation notice and the seat opening.
    pub agent_notice_period: i64,
    /// Seconds a spending proposal or challenge vote stays open.
    pub proposal_voting_time: i64,
    /// Requests at or under this approve without a vote, in quote units.
    pub low_proposal: u64,
    /// Requests at or over this need the high threshold, in quote units.
    pub high_proposal: u64,
    /// Yes share of yes+no a high request must reach, in basis points.
    pub high_threshold_bps: u16,
    /// Seconds between auto-approved requests on one property.
    pub auto_approval_cooldown: i64,
    /// XCAV a challenger stakes against the sitting agent.
    pub challenge_deposit: u64,
    /// XCAV slashed from the agent's location deposit per passed challenge.
    pub agent_slash_amount: u64,
    pub bump: u8,
}

/// Most agents that can stand in one election round. Bounds the account list
/// `finalize_agent_election` walks to find the plurality winner.
pub const MAX_AGENT_CANDIDATES: u32 = 5;

/// The running agent election. Any eligible agent may stand; the first
/// candidacy opens the voting window and shareholders vote among the
/// candidates. A round that elects nobody simply reopens: rounds repeat
/// until an agent is engaged. One round at a time per property.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug, Default,
)]
pub struct AgentElection {
    /// When voting closes; zero while no round is running.
    pub expiry: i64,
    /// Candidates standing in the current round.
    pub candidate_count: u32,
    /// Round number, monotonic per property. Candidacies and vote records
    /// are keyed by it, so nothing stale can count toward a later round.
    pub round: u64,
    /// Quorum at the moment the round opened, so a config change can't move
    /// the goalposts mid-vote.
    pub quorum_bps: u16,
}

/// A property's governance ledger: one live proposal and one live challenge
/// at a time, and strikes against the sitting agent.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug, Default,
)]
pub struct GovState {
    /// Ids already used; the next proposal or challenge takes count + 1.
    pub proposal_count: u64,
    pub challenge_count: u64,
    /// The live proposal / challenge id; zero while none. One of each at a
    /// time per property.
    pub active_proposal: u64,
    pub active_challenge: u64,
    /// Passed challenges against the sitting agent; three remove them.
    /// Reset when a new agent takes the seat.
    pub strikes: u8,
    /// When the last request was approved without a vote.
    pub last_auto_approval_ts: i64,
}

/// A property's letting seat: who manages it, the election that fills the
/// seat, and the governance the holders run over the agent. Created by the
/// first candidacy and kept for the property's life.
#[account]
#[derive(InitSpace)]
pub struct PropertyLetting {
    pub asset_id: u64,
    /// The assigned agent; default while the seat is vacant.
    pub agent: Pubkey,
    pub election: AgentElection,
    pub governance: GovState,
    /// The wallet that fronted the account's rent.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// One agent standing in one election round; carries their own tally.
#[account]
#[derive(InitSpace)]
pub struct AgentCandidacy {
    pub asset_id: u64,
    /// The election round the candidacy belongs to.
    pub round: u64,
    pub agent: Pubkey,
    /// Share-weighted votes cast for this candidate.
    pub vote_power: u32,
    /// The wallet that fronted the account's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// One investor's vote in one agent election round. The voting shares stay
/// locked in the marketplace ShareHolding until the record is closed again
/// by `unlock_agent_votes`.
#[account]
#[derive(InitSpace)]
pub struct AgentVote {
    pub asset_id: u64,
    /// The election round the vote belongs to.
    pub round: u64,
    pub voter: Pubkey,
    /// The candidate voted for.
    pub choice: Pubkey,
    /// The shares this vote locked and counts for.
    pub power: u32,
    /// The wallet that fronted the record's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// An assigned agent's notice that they are stepping down. The seat opens
/// once the notice period has run, via the `finalize_resignation` crank.
#[account]
#[derive(InitSpace)]
pub struct ResignationNotice {
    pub asset_id: u64,
    /// The resigning agent, recorded so the crank can release their
    /// assignment even if the seat's state moves on.
    pub agent: Pubkey,
    /// When the resignation takes effect.
    pub due_ts: i64,
    /// The wallet that fronted the account's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// Most payment mints one property can ever receive income in. Twice the
/// marketplace's accepted-mint cap, so the protocol can rotate mints a few
/// times without capping a long-lived property's income.
pub const MAX_INCOME_STREAMS: usize = 8;

/// One payment mint's income stream for a property.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, PartialEq, Eq, Debug)]
pub struct IncomeStream {
    pub mint: Pubkey,
    /// Cumulative income per share since the stream opened, in raw token
    /// units. Wide on purpose: a stream lives as long as the property.
    pub per_share: u128,
    /// Remainder of the last distribution below one unit per share, carried
    /// into the next one so nothing is stranded.
    pub dust: u64,
}

/// A property's rental income ledger, one stream per payment mint. The
/// streams never mix: the mints differ in decimals, so one shared
/// accumulator would misprice payouts across them. The funds sit in the
/// income vault's token accounts, apart from every other pot, so nothing
/// else can spend money already owed to holders.
///
/// Deliberately has no close path: a finalized property lives forever, and
/// the carried dust is still owed to future distributions.
#[account]
#[derive(InitSpace)]
pub struct PropertyIncome {
    pub asset_id: u64,
    /// Append-only: checkpoints refer to streams by index.
    #[max_len(MAX_INCOME_STREAMS)]
    pub streams: Vec<IncomeStream>,
    /// The wallet that fronted the account's rent.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A holder's claim state against the stream at the same index.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug, Default,
)]
pub struct CheckpointEntry {
    /// The stream's `per_share` the holder is paid or banked up to.
    pub per_share: u128,
    /// Income banked for the holder but not yet paid out, written when a
    /// settle runs ahead of a balance change.
    pub pending: u64,
}

/// One holder's income position on one property. `entries[i]` tracks
/// `streams[i]`; missing tail entries read as zero.
#[account]
#[derive(InitSpace)]
pub struct IncomeCheckpoint {
    pub asset_id: u64,
    pub owner: Pubkey,
    #[max_len(MAX_INCOME_STREAMS)]
    pub entries: Vec<CheckpointEntry>,
    /// The wallet that fronted the account's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A vote's direction. Abstain counts toward quorum but neither side.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum VoteChoice {
    Yes,
    No,
    Abstain,
}

/// Share-weighted vote tallies.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug, Default,
)]
pub struct Tally {
    pub yes: u32,
    pub no: u32,
    pub abstain: u32,
}

/// The assigned agent's request for holder sign-off on spending for the
/// property. Small requests approve on the spot (cooldown-gated) and never
/// hit storage; the rest live here for the length of the vote. The money
/// itself moves off chain, authorized by the `ProposalExecuted` event.
#[account]
#[derive(InitSpace)]
pub struct Proposal {
    pub asset_id: u64,
    /// Monotonic per property; vote records are keyed by it.
    pub id: u64,
    pub proposer: Pubkey,
    /// What they ask for, in quote units.
    pub amount: u64,
    /// Hash of the off-chain document describing the work.
    pub details_hash: [u8; 32],
    /// When voting closes.
    pub expiry: i64,
    pub tally: Tally,
    /// Quorum and approval threshold at the moment the vote opened, so a
    /// config change can't move the goalposts mid-vote. The threshold is
    /// zero below the high tier.
    pub quorum_bps: u16,
    pub threshold_bps: u16,
    /// The wallet that fronted the account's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A holder's move to strike the sitting agent, backed by an XCAV stake the
/// challenger loses if the vote goes against them.
#[account]
#[derive(InitSpace)]
pub struct Challenge {
    pub asset_id: u64,
    /// Monotonic per property; vote records are keyed by it.
    pub id: u64,
    pub challenger: Pubkey,
    /// The agent on the seat when the challenge opened; the punishment only
    /// applies while they still hold it.
    pub agent: Pubkey,
    /// The challenger's stake, held in the XCAV vault.
    pub deposit: u64,
    pub expiry: i64,
    pub tally: Tally,
    /// Quorum at the moment the vote opened.
    pub quorum_bps: u16,
    /// The wallet that fronted the account's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// One holder's vote on one proposal or challenge (the seed prefix says
/// which). The voting shares stay locked in the marketplace ShareHolding
/// until the record is closed again by the matching unlock instruction.
#[account]
#[derive(InitSpace)]
pub struct GovVote {
    pub asset_id: u64,
    /// The proposal or challenge the vote belongs to.
    pub id: u64,
    pub voter: Pubkey,
    pub choice: VoteChoice,
    /// The shares this vote locked and counts for.
    pub power: u32,
    /// The wallet that fronted the record's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A letting agent's registry entry, one per wallet. Agents work one region
/// and any number of its locations up to the cap; each location holds its
/// own deposit and counts the properties assigned there.
#[account]
#[derive(InitSpace)]
pub struct LettingAgent {
    pub wallet: Pubkey,
    /// The one region this agent covers.
    pub region_id: u16,
    #[max_len(MAX_AGENT_LOCATIONS)]
    pub locations: Vec<AgentLocation>,
    /// Who funded the account's rent and gets it back at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, PartialEq, Eq, Debug)]
pub struct AgentLocation {
    #[max_len(POSTCODE_MAX_LEN)]
    pub postcode: Vec<u8>,
    /// Properties currently assigned to the agent in this location. Leaving
    /// the location requires zero.
    pub assigned_count: u32,
    /// The deposit locked when this location was registered, recorded so a
    /// later config change can't reprice the refund.
    pub deposit: u64,
}
