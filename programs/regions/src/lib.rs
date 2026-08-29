pub mod constants;
pub mod error;
pub mod instructions;
pub mod mint_guard;
pub mod state;
pub use xcavate_common::vault;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("5iupkzVtWxee48UXh3s615V9sXXuYjsSr61VPuduXdPc");

/// Region governance for the realXmarket protocol: a regional operator proposes
/// a region by bonding XCAV, holders vote, and on a pass the proposer claims it
/// and becomes the operator. A seat turns over at the end of the operator's term
/// (or earlier through resignation), after which any operator can claim the open
/// region by bonding and the incumbent can renew. On top of the seat governance
/// the region carries the marketplace's static data: registered locations
/// (postcodes), the property sale tax, and the listing duration.
#[program]
pub mod regions {
    use super::*;

    /// Initialize the singleton config with governance parameters.
    pub fn initialize_config(ctx: Context<InitializeConfig>, params: ConfigParams) -> Result<()> {
        initialize::handler(ctx, params)
    }

    /// Update the governance parameters. Authority-only.
    pub fn update_config(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
        initialize::update_config_handler(ctx, params)
    }

    /// Propose a new config authority (two-step handover). Current-authority-only.
    pub fn update_authority(ctx: Context<UpdateAuthority>, new_authority: Pubkey) -> Result<()> {
        initialize::update_authority_handler(ctx, new_authority)
    }

    /// Complete the authority handover. Signed by the pending authority.
    pub fn accept_authority(ctx: Context<AcceptAuthority>) -> Result<()> {
        initialize::accept_authority_handler(ctx)
    }

    /// Propose a new region. RegionalOperator-only; bonds 0.1% of XCAV supply,
    /// capped by the caller's `max_deposit`.
    pub fn propose_new_region(
        ctx: Context<ProposeNewRegion>,
        region_id: u16,
        max_deposit: u64,
    ) -> Result<()> {
        propose::propose_new_region_handler(ctx, region_id, max_deposit)
    }

    /// Vote on an open proposal. Anyone may vote; the amount is locked.
    pub fn vote_on_region_proposal(
        ctx: Context<VoteOnRegionProposal>,
        region_id: u16,
        vote: Vote,
        amount: u64,
    ) -> Result<()> {
        vote::vote_on_region_proposal_handler(ctx, region_id, vote, amount)
    }

    /// Finalize an expired proposal (permissionless crank).
    pub fn finalize_region_proposal(
        ctx: Context<FinalizeRegionProposal>,
        region_id: u16,
    ) -> Result<()> {
        finalize::finalize_region_proposal_handler(ctx, region_id)
    }

    /// Claim a region whose proposal passed, creating it with its initial
    /// listing duration, sale tax and fees. Proposer-only.
    pub fn create_region(
        ctx: Context<CreateRegion>,
        region_id: u16,
        listing_duration: i64,
        tax_bps: u16,
        seller_fee_bps: u16,
        buyer_fee_bps: u16,
    ) -> Result<()> {
        create::create_region_handler(
            ctx,
            region_id,
            listing_duration,
            tax_bps,
            seller_fee_bps,
            buyer_fee_bps,
        )
    }

    /// Claim an open region seat, bonding 0.1% of XCAV supply plus the region's
    /// location deposits, capped by the caller's `max_deposit`. First-come and
    /// RegionalOperator-only; the incumbent may also call this to renew, paying
    /// only the difference if the bond has moved since they last bonded.
    pub fn claim_open_region(
        ctx: Context<ClaimOpenRegion>,
        region_id: u16,
        max_deposit: u64,
    ) -> Result<()> {
        create::claim_open_region_handler(ctx, region_id, max_deposit)
    }

    /// Reclaim locked voting XCAV after a proposal's voting window ends.
    pub fn unlock_voting_token(ctx: Context<UnlockVotingToken>, proposal_id: u64) -> Result<()> {
        cleanup::unlock_voting_token_handler(ctx, proposal_id)
    }

    /// Close a rejected/empty region state so the region can be proposed again.
    pub fn clear_region_state(ctx: Context<ClearRegionState>, region_id: u16) -> Result<()> {
        cleanup::clear_region_state_handler(ctx, region_id)
    }

    /// Schedule the caller's own departure as a region's operator.
    pub fn initiate_resignation(ctx: Context<InitiateResignation>, region_id: u16) -> Result<()> {
        create::initiate_resignation_handler(ctx, region_id)
    }

    /// Register a postcode as a listable location. Region-operator-only;
    /// locks a location deposit (capped by `max_deposit`) that joins the
    /// region's collateral.
    pub fn create_new_location(
        ctx: Context<CreateNewLocation>,
        region_id: u16,
        postcode: Vec<u8>,
        max_deposit: u64,
    ) -> Result<()> {
        manage::create_new_location_handler(ctx, region_id, postcode, max_deposit)
    }

    /// Deregister a postcode and release its recorded deposit. Region-operator-only.
    pub fn remove_location(
        ctx: Context<RemoveLocation>,
        region_id: u16,
        postcode: Vec<u8>,
    ) -> Result<()> {
        manage::remove_location_handler(ctx, region_id, postcode)
    }

    /// Change how long new listings in the region stay active. Region-operator-only.
    pub fn adjust_listing_duration(
        ctx: Context<AdjustRegion>,
        region_id: u16,
        listing_duration: i64,
    ) -> Result<()> {
        manage::adjust_listing_duration_handler(ctx, region_id, listing_duration)
    }

    /// Change the region's property sale tax. Region-operator-only.
    pub fn adjust_region_tax(
        ctx: Context<AdjustRegion>,
        region_id: u16,
        tax_bps: u16,
    ) -> Result<()> {
        manage::adjust_region_tax_handler(ctx, region_id, tax_bps)
    }

    /// Change the region's seller and buyer fees. Region-operator-only.
    pub fn adjust_region_fees(
        ctx: Context<AdjustRegion>,
        region_id: u16,
        seller_fee_bps: u16,
        buyer_fee_bps: u16,
    ) -> Result<()> {
        manage::adjust_region_fees_handler(ctx, region_id, seller_fee_bps, buyer_fee_bps)
    }
}
