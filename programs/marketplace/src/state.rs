use anchor_lang::prelude::*;

pub use regions::state::POSTCODE_MAX_LEN;

/// Most payment mints the protocol accepts at once.
pub const MAX_PAYMENT_MINTS: usize = 4;

/// Hard ceiling on shares per property. No instruction may iterate holders
/// (teardown is lazy, per holder), but the supply still has to stay bounded so
/// per-share arithmetic can't overflow u64 prices.
pub const MAX_SHARE_SUPPLY: u32 = 100;

/// Singleton config holding protocol parameters and the authority.
#[account]
#[derive(InitSpace)]
pub struct Config {
    /// Authority allowed to update parameters.
    pub authority: Pubkey,
    /// Proposed replacement authority; takes over via `accept_authority`.
    /// Two-step so a typo'd address can't brick parameter management.
    pub pending_authority: Option<Pubkey>,
    /// The XCAV mint listing and lawyer deposits are paid in.
    pub xcav_mint: Pubkey,
    /// Owner of the shared protocol treasury (a multisig on mainnet). Fees are
    /// paid to this key's token accounts; the program never holds them.
    pub treasury: Pubkey,
    /// The sponsor wallet that fronts account rent for investors. Every
    /// position close sends its lamports here, not to the investor.
    pub rent_collector: Pubkey,
    /// Mints a property can be priced and paid in. Every entry must be a
    /// same-value GBP stablecoin: prices convert between them by decimal
    /// count alone, so adding a mint of different value would misprice every
    /// open listing.
    #[max_len(MAX_PAYMENT_MINTS)]
    pub accepted_payment_mints: Vec<Pubkey>,
    /// XCAV a developer locks to list a property.
    pub listing_deposit: u64,
    /// XCAV a lawyer locks to join the registry.
    pub lawyer_deposit: u64,
    /// Fewest shares a property may be split into.
    pub min_property_shares: u32,
    /// Most shares a property may be split into.
    pub max_property_shares: u32,
    /// Protocol fee taken from the developer's proceeds, in basis points.
    pub marketplace_fee_bps: u16,
    /// Fee an investor pays on top of the share price, in basis points.
    pub investor_fee_bps: u16,
    /// Largest slice of a property one investor may hold, in basis points.
    pub max_ownership_bps: u16,
    /// Seconds the claim window stays open once the SPV exists.
    pub claiming_time: i64,
    /// Seconds the legal process may run before it expires.
    pub legal_process_time: i64,
    /// Seconds the SPV lawyer election stays open once the first lawyer claims.
    pub lawyer_voting_time: i64,
    /// Share of a property's supply that must vote for the SPV lawyer election
    /// to be valid, in basis points.
    pub min_voting_quorum_bps: u16,
    /// Monotonic id for the next listing.
    pub next_listing_id: u64,
    /// Monotonic id for the next secondary share listing.
    pub next_share_listing_id: u64,
    pub bump: u8,
}

/// Where a primary listing is in its lifecycle.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListingStatus {
    /// Created, but the share mint doesn't exist yet.
    PendingAssets,
    /// Open for share purchases.
    Listed,
    /// Every share sold; the legal process can start.
    SoldOut,
    /// Lawyers are confirming the sale documents.
    Legal,
    /// Settled; the property is live.
    Finalized,
    /// Expired before selling out; refunds open.
    Expired,
    /// Cancelled by the legal process; refunds open.
    Cancelled,
    /// Being torn down; waiting for the last holder to withdraw.
    Refunding,
}

/// Longest name and metadata URI a property may carry.
pub const MAX_PROPERTY_NAME_LEN: usize = 40;
pub const MAX_PROPERTY_URI_LEN: usize = 200;

/// A fractionalized property. Created when it is listed and kept for the
/// asset's whole life; the share mint and metadata are attached by
/// `init_property_assets`.
#[account]
#[derive(InitSpace)]
pub struct PropertyAsset {
    pub asset_id: u64,
    /// Display name, e.g. the street address. Empty until the assets are
    /// initialized.
    #[max_len(MAX_PROPERTY_NAME_LEN)]
    pub name: String,
    /// IPFS URI of the property's documents and images; the canonical
    /// off-chain record the frontend and indexers read.
    #[max_len(MAX_PROPERTY_URI_LEN)]
    pub metadata_uri: String,
    /// The Token-2022 share mint. Default until the assets are initialized.
    pub share_mint: Pubkey,
    pub region_id: u16,
    /// The registered location (postcode) the property sits in.
    #[max_len(POSTCODE_MAX_LEN)]
    pub location: Vec<u8>,
    pub share_amount: u32,
    pub spv_created: bool,
    pub finalized: bool,
    /// Wallets currently holding shares; teardown completes when it reaches
    /// zero again.
    pub holder_count: u32,
    pub bump: u8,
}

/// Where a lawyer's document review stands.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug, Default,
)]
pub enum DocumentStatus {
    #[default]
    Pending,
    Approved,
    Rejected,
}

/// One side's engaged lawyer on a sold-out sale.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug, Default,
)]
pub struct LawyerAssignment {
    /// The lawyer on this side; default until one is engaged.
    pub lawyer: Pubkey,
    /// What they charge, quoted at `PRICE_DECIMALS`. Only the SPV side is
    /// paid from the collected fees; the developer's lawyer is a private
    /// arrangement, so their side stays zero.
    pub costs: u64,
    pub doc_status: DocumentStatus,
    /// Hash of the document set the verdict was passed on; zero while
    /// pending. Both sides must rule on the same hash.
    pub documents_hash: [u8; 32],
}

/// Most lawyers that can stand in one SPV election round. Bounds the account
/// list `finalize_spv_election` walks to find the plurality winner.
pub const MAX_SPV_CANDIDATES: u32 = 5;

/// The running SPV lawyer election. Any eligible lawyer may stand; the first
/// candidacy opens the voting window and shareholders vote among the
/// candidates. A round that elects nobody simply reopens: rounds repeat
/// until a lawyer is engaged or the legal process expires into its timeout
/// exit. One round at a time per listing.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug, Default,
)]
pub struct SpvElection {
    /// When voting closes; zero while no round is running.
    pub expiry: i64,
    /// Candidates standing in the current round.
    pub candidate_count: u32,
    /// Round number, monotonic per listing. Candidacies and vote records are
    /// keyed by it, so nothing stale can count toward a later round.
    pub round: u64,
}

/// A primary property listing. Prices and windows are snapshotted here at
/// listing time, so config or region changes never reprice a sale underway.
#[account]
#[derive(InitSpace)]
pub struct Listing {
    pub listing_id: u64,
    pub developer: Pubkey,
    pub asset_id: u64,
    /// Price per share, in payment-mint base units.
    pub share_price: u64,
    /// Shares put up for sale.
    pub listed_share_amount: u32,
    /// Shares sold (paid for) so far.
    pub sold_share_amount: u32,
    /// Shares reserved but not yet claimed. Reserved money stays in the
    /// investors' own wallets until the claim pays for them.
    pub reserved_share_amount: u32,
    /// Whether the developer covers the region's sale tax themselves.
    pub tax_paid_by_developer: bool,
    /// The region's sale tax at listing time, in basis points.
    pub tax_bps: u16,
    /// Protocol fee at listing time, in basis points.
    pub marketplace_fee_bps: u16,
    /// Investor fee at listing time, in basis points.
    pub investor_fee_bps: u16,
    /// Ownership cap at listing time, in basis points. Holdings must stay
    /// strictly below it, so even 10_000 requires at least two holders.
    pub max_ownership_bps: u16,
    pub listing_expiry: i64,
    /// Seconds the claim window runs once the SPV exists, taken from config
    /// at listing time.
    pub claiming_time: i64,
    /// When the claim window closes and direct purchases open. Zero until
    /// the SPV attestation stamps it.
    pub claim_deadline: i64,
    /// Seconds the legal process may run once the listing sells out, taken
    /// from config at listing time.
    pub legal_process_time: i64,
    /// Seconds the SPV-lawyer election stays open, taken from config at
    /// listing time.
    pub lawyer_voting_time: i64,
    /// Quorum for the SPV-lawyer election, taken from config at listing time.
    pub min_voting_quorum_bps: u16,
    /// Open `InvestorPosition` accounts, cancelled ones included. Teardown
    /// waits for zero, so a position can never outlive the listing it needs
    /// to close against.
    pub position_count: u32,
    /// Set when the last share sells: the moment the legal process runs out
    /// and the timeout exit opens. Zero until then.
    pub legal_deadline: i64,
    /// The XCAV locked by the developer at listing, held in the vault. Zero
    /// doubles as "already withdrawn", which is unambiguous because config
    /// validation never accepts a zero deposit.
    pub deposit: u64,
    /// The developer's lawyer on the sale, allocated by the developer.
    pub developer_lawyer: LawyerAssignment,
    /// The SPV's lawyer on the sale, chosen by investor vote.
    pub spv_lawyer: LawyerAssignment,
    /// Set when a split document verdict sends the papers back for revision.
    /// The revised set is the last chance: a second split cancels the sale.
    pub second_attempt: bool,
    /// Whether the developer ever appointed their lawyer. Sticky: a sale that
    /// times out without this costs the developer 1% of the bond, but a
    /// lawyer resigning later doesn't.
    pub developer_engaged: bool,
    /// Stamped at cancellation: what the SPV lawyer is still owed from the
    /// retained fees, and who collects it. Kept on the listing because
    /// `close_case` clears the assignment before the fees settle.
    pub spv_costs_due: u64,
    pub spv_costs_payee: Pubkey,
    /// Investor fees collected so far, at `PRICE_DECIMALS`; caps the SPV
    /// lawyer's charge without trusting the current share price.
    pub collected_fee_quote: u64,
    /// What each payment mint collected across the sale, split the way
    /// settlement pays it out. Written by every claim and direct buy.
    #[max_len(MAX_PAYMENT_MINTS)]
    pub collected: Vec<CollectedPerMint>,
    pub spv_election: SpvElection,
    pub status: ListingStatus,
    pub bump: u8,
}

impl Listing {
    /// The investor fees a full sale collects, quoted at `PRICE_DECIMALS`.
    /// Lawyer costs are capped by this pot; the per-buy transfers floor when
    /// rescaling to each mint, so settlement pays out with the same floor.
    /// Add one payment's components to its mint's collected totals;
    /// `fee_quote` is the same fee at `PRICE_DECIMALS`.
    pub fn record_collected(
        &mut self,
        mint: Pubkey,
        funds: u64,
        fee: u64,
        fee_quote: u64,
        tax: u64,
    ) -> Result<()> {
        self.collected_fee_quote = self
            .collected_fee_quote
            .checked_add(fee_quote)
            .ok_or(crate::error::MarketplaceError::Overflow)?;
        use crate::error::MarketplaceError;
        let entry = match self.collected.iter_mut().find(|c| c.mint == mint) {
            Some(entry) => entry,
            None => {
                require!(
                    self.collected.len() < MAX_PAYMENT_MINTS,
                    MarketplaceError::TooManyMints
                );
                self.collected.push(CollectedPerMint {
                    mint,
                    ..Default::default()
                });
                self.collected.last_mut().unwrap()
            }
        };
        entry.funds = entry
            .funds
            .checked_add(funds)
            .ok_or(MarketplaceError::Overflow)?;
        entry.fee = entry
            .fee
            .checked_add(fee)
            .ok_or(MarketplaceError::Overflow)?;
        entry.tax = entry
            .tax
            .checked_add(tax)
            .ok_or(MarketplaceError::Overflow)?;
        Ok(())
    }

    /// The strict-below ownership cap, floored to whole shares: at 50% of
    /// 100 shares nobody may reach 50, so the most is 49. `owned_after` is
    /// whatever the call site says the wallet would control.
    pub fn require_below_ownership_cap(&self, owned_after: u64, supply: u32) -> Result<()> {
        use crate::error::MarketplaceError;
        let max_shares = (self.max_ownership_bps as u64)
            .checked_mul(supply as u64)
            .ok_or(MarketplaceError::Overflow)?
            / 10_000;
        require!(
            owned_after < max_shares,
            MarketplaceError::MaxOwnershipExceeded
        );
        Ok(())
    }
}

/// One payment mint's totals across the primary sale, in that mint's units.
#[derive(
    AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug, Default,
)]
pub struct CollectedPerMint {
    pub mint: Pubkey,
    pub funds: u64,
    pub fee: u64,
    pub tax: u64,
}

/// One lawyer standing in one SPV election round; carries their own tally.
/// Whoever fronts the rent (the sponsor by default) takes it back with
/// `close_candidacy` once the round is over.
#[account]
#[derive(InitSpace)]
pub struct LawyerCandidacy {
    pub listing_id: u64,
    /// The election round the candidacy belongs to.
    pub round: u64,
    pub lawyer: Pubkey,
    /// What the candidate would charge, quoted at `PRICE_DECIMALS`.
    pub costs: u64,
    /// Share-weighted votes cast for this candidate.
    pub vote_power: u32,
    /// The wallet that fronted the account's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// One investor's vote in one SPV lawyer election round. Locks the voting
/// shares until the record is closed again by `unlock_voting_shares`.
#[account]
#[derive(InitSpace)]
pub struct LawyerVote {
    pub listing_id: u64,
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

/// Prices are quoted at this scale (tGBP's 9 decimals); transfers rescale to
/// each payment mint's own decimals, flooring in the investor's favour. The
/// rescale is by decimal count alone, which is only sound because every
/// accepted payment mint must be a same-value GBP stablecoin.
pub const PRICE_DECIMALS: u8 = 9;

/// Accepted payment mints must sit in this decimals range: the lower bound
/// keeps a minimum-priced share from flooring to zero, the upper bound keeps
/// the rescale arithmetic comfortably inside u128.
pub const MIN_PAYMENT_DECIMALS: u8 = 6;
pub const MAX_PAYMENT_DECIMALS: u8 = 12;

/// Why shares are locked. Each reason keeps its own counter on the holding
/// and the effective lock is the largest of them, so backing one vote never
/// spends weight another kind of vote could still use.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, PartialEq, Eq, Debug)]
pub enum LockReason {
    LawyerElection,
    AgentElection,
    Proposal,
    Challenge,
}

/// One lock slot per `LockReason` variant; the two must grow together.
pub const LOCK_REASONS: usize = 4;

/// The canonical share ledger for one holder of one property. The Token-2022
/// accounts mirror this; they never lead it.
#[account]
#[derive(InitSpace)]
pub struct ShareHolding {
    pub asset_id: u64,
    pub owner: Pubkey,
    pub amount: u32,
    /// Shares locked by votes, one counter per `LockReason`. Counters are
    /// additive within a reason but overlap across reasons.
    pub locks: [u32; LOCK_REASONS],
    /// Shares committed to open secondary listings. Still counted in
    /// `amount` — the seller keeps the ledger, and the income that comes
    /// with it, until the moment of sale — but out of voting and transfers.
    pub listed: u32,
    pub bump: u8,
}

impl ShareHolding {
    /// The effective lock: shares blocked from transfer until unlocked.
    pub fn locked(&self) -> u32 {
        self.locks.into_iter().max().unwrap_or(0)
    }

    /// Shares free to vote with: everything not committed to a listing.
    pub fn votable(&self) -> u32 {
        self.amount.saturating_sub(self.listed)
    }

    /// Shares free to sell or send: everything neither vote-locked nor
    /// already listed.
    pub fn transferable(&self) -> u32 {
        self.amount
            .saturating_sub(self.locked())
            .saturating_sub(self.listed)
    }
}

/// A holder's open offer to sell part of their shares on the secondary
/// market. The shares stay on the seller's holding (reserved via `listed`)
/// until someone buys.
#[account]
#[derive(InitSpace)]
pub struct ShareListing {
    pub id: u64,
    pub asset_id: u64,
    pub seller: Pubkey,
    /// Asking price per share, in quote units.
    pub share_price: u64,
    /// Shares still for sale; partial buys draw it down.
    pub amount: u32,
    /// Marketplace fee at the moment of listing, so a config change can't
    /// reprice the seller's proceeds under them.
    pub fee_bps: u16,
    /// Stamped on each offer against this listing; the seller accepts by
    /// nonce, so an offer can't be swapped under their signature.
    pub next_offer_nonce: u64,
    /// The wallet that fronted the account's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A bid below (or above) a share listing's asking price, one per bidder
/// per listing. The bid money sits in the offer's own vault from make to
/// settle, so an accepted offer can always pay.
#[account]
#[derive(InitSpace)]
pub struct Offer {
    /// The share listing this bids on.
    pub listing_id: u64,
    pub asset_id: u64,
    pub offeror: Pubkey,
    /// Offered price per share, in quote units.
    pub share_price: u64,
    pub amount: u32,
    pub payment_mint: Pubkey,
    /// What the vault holds, in the mint's units; paid out or refunded in
    /// full, exactly once.
    pub held: u64,
    pub nonce: u64,
    /// The wallet that fronted the offer's rent; refunded at close.
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// One investor's stake in a primary listing: what they paid, per component,
/// so refunds and settlement can be exact. The shares themselves are already
/// delivered; this is the accounting.
#[account]
#[derive(InitSpace)]
pub struct InvestorPosition {
    pub listing_id: u64,
    pub investor: Pubkey,
    /// The mint the investor paid in. One mint per position: later buys must
    /// use the same one, so every refund is a single transfer.
    pub payment_mint: Pubkey,
    /// The investor's token account the reservation is recorded against and
    /// the claim later pays from.
    pub payment_account: Pubkey,
    /// Shares paid for and delivered.
    pub share_amount: u32,
    /// Shares reserved and not yet claimed; a claim converts all of them at
    /// once, so a position is only ever reserved or paid, not both.
    pub reserved_share_amount: u32,
    /// Paid toward the property price, in payment-mint base units.
    pub paid_funds: u64,
    /// Paid as sale tax on top, in payment-mint base units.
    pub paid_tax: u64,
    /// Paid as the investor fee on top, in payment-mint base units.
    pub paid_fee: u64,
    /// Reserved toward the property price, still in the investor's wallet.
    pub reserved_funds: u64,
    /// Reserved for the sale tax, still in the investor's wallet.
    pub reserved_tax: u64,
    /// Reserved for the investor fee, still in the investor's wallet.
    pub reserved_fee: u64,
    /// Set when the investor unreserved. The position stays open as the
    /// one-way re-buy bar: `buy` rejects a cancelled position forever.
    pub cancelled: bool,
    pub bump: u8,
}

/// Payment money promised to unclaimed reservations, one record per token
/// account, summed across every listing the owner reserved into. Reserving
/// checks the balance covers it; only the claim actually collects.
#[account]
#[derive(InitSpace)]
pub struct Reservation {
    pub token_account: Pubkey,
    pub amount: u64,
    pub bump: u8,
}

/// A lawyer registered to take cases in a region. One registration per wallet;
/// only registered lawyers can be assigned to a sale.
#[account]
#[derive(InitSpace)]
pub struct Lawyer {
    pub lawyer: Pubkey,
    /// The region the lawyer serves; cases must match it.
    pub region_id: u16,
    /// The XCAV locked at registration, held in the vault. Returned on
    /// unregister, even if the configured deposit has changed since.
    pub deposit: u64,
    /// Cases currently assigned; must be zero to unregister.
    pub active_cases: u32,
    pub bump: u8,
}
