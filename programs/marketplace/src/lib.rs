//! The realXmarket marketplace: developers list fractionalized properties,
//! investors buy shares that are delivered directly at purchase, and once a
//! property sells out the legal process hands the deed to an SPV and settles
//! everyone atomically. Deposits (listing, lawyer) are staked in XCAV; sales
//! are paid in the accepted payment mints.

pub mod compliance_guard;
pub mod constants;
pub mod error;
pub mod instructions;
pub mod mint_guard;
pub mod state;
pub use xcavate_common::vault;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::ConfigParams;

use instructions::*;
use state::LockReason;

declare_id!("dj9Q3CpHvDHwexCbkgJ5APDx4JsTxPssNebkvP15g1T");

#[program]
pub mod marketplace {
    use super::*;

    pub fn initialize_config(ctx: Context<InitializeConfig>, params: ConfigParams) -> Result<()> {
        initialize::handler(ctx, params)
    }

    pub fn update_config(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
        initialize::update_config_handler(ctx, params)
    }

    pub fn update_authority(ctx: Context<UpdateAuthority>, new_authority: Pubkey) -> Result<()> {
        initialize::update_authority_handler(ctx, new_authority)
    }

    pub fn accept_authority(ctx: Context<AcceptAuthority>) -> Result<()> {
        initialize::accept_authority_handler(ctx)
    }

    pub fn register_lawyer(
        ctx: Context<RegisterLawyer>,
        region_id: u16,
        max_deposit: u64,
    ) -> Result<()> {
        lawyers::register_lawyer_handler(ctx, region_id, max_deposit)
    }

    pub fn unregister_lawyer(ctx: Context<UnregisterLawyer>) -> Result<()> {
        lawyers::unregister_lawyer_handler(ctx)
    }

    pub fn list_property(
        ctx: Context<ListProperty>,
        region_id: u16,
        postcode: Vec<u8>,
        share_price: u64,
        share_amount: u32,
        tax_paid_by_developer: bool,
        max_deposit: u64,
    ) -> Result<()> {
        listing::list_property_handler(
            ctx,
            region_id,
            postcode,
            share_price,
            share_amount,
            tax_paid_by_developer,
            max_deposit,
        )
    }

    pub fn upgrade_object(
        ctx: Context<UpgradeObject>,
        listing_id: u64,
        new_price: u64,
    ) -> Result<()> {
        listing::upgrade_object_handler(ctx, listing_id, new_price)
    }

    pub fn init_property_assets(
        ctx: Context<InitPropertyAssets>,
        listing_id: u64,
        name: String,
        uri: String,
    ) -> Result<()> {
        assets::init_property_assets_handler(ctx, listing_id, name, uri)
    }

    pub fn buy_property_shares(
        ctx: Context<BuyPropertyShares>,
        listing_id: u64,
        amount: u32,
        max_total_cost: u64,
    ) -> Result<()> {
        buy::buy_property_shares_handler(ctx, listing_id, amount, max_total_cost)
    }

    pub fn unreserve_shares(ctx: Context<UnreserveShares>, listing_id: u64) -> Result<()> {
        reserve::unreserve_shares_handler(ctx, listing_id)
    }

    pub fn close_cancelled_position(
        ctx: Context<CloseCancelledPosition>,
        listing_id: u64,
        investor: Pubkey,
    ) -> Result<()> {
        reserve::close_cancelled_position_handler(ctx, listing_id, investor)
    }

    pub fn create_spv(ctx: Context<CreateSpv>, listing_id: u64) -> Result<()> {
        spv::create_spv_handler(ctx, listing_id)
    }

    pub fn withdraw_expired(ctx: Context<WithdrawExpired>, listing_id: u64) -> Result<()> {
        withdraw::withdraw_expired_handler(ctx, listing_id)
    }

    pub fn withdraw_deposit_unsold(
        ctx: Context<WithdrawDepositUnsold>,
        listing_id: u64,
    ) -> Result<()> {
        withdraw::withdraw_deposit_unsold_handler(ctx, listing_id)
    }

    pub fn withdraw_legal_process_expired(
        ctx: Context<WithdrawExpired>,
        listing_id: u64,
    ) -> Result<()> {
        withdraw::withdraw_legal_process_expired_handler(ctx, listing_id)
    }

    pub fn close_dead_listing<'info>(
        ctx: Context<'info, CloseDeadListing<'info>>,
        listing_id: u64,
    ) -> Result<()> {
        teardown::close_dead_listing_handler(ctx, listing_id)
    }

    pub fn close_settled_payment_accounts<'info>(
        ctx: Context<'info, CloseSettledPaymentAccounts<'info>>,
        listing_id: u64,
    ) -> Result<()> {
        teardown::close_settled_payment_accounts_handler(ctx, listing_id)
    }

    pub fn assign_developer_lawyer(
        ctx: Context<AssignDeveloperLawyer>,
        listing_id: u64,
        lawyer: Pubkey,
    ) -> Result<()> {
        legal::assign_developer_lawyer_handler(ctx, listing_id, lawyer)
    }

    pub fn claim_spv_case(
        ctx: Context<ClaimSpvCase>,
        listing_id: u64,
        round: u64,
        costs: u64,
    ) -> Result<()> {
        legal::claim_spv_case_handler(ctx, listing_id, round, costs)
    }

    pub fn vote_on_spv_lawyer(
        ctx: Context<VoteOnSpvLawyer>,
        listing_id: u64,
        amount: u32,
    ) -> Result<()> {
        election::vote_on_spv_lawyer_handler(ctx, listing_id, amount)
    }

    pub fn close_candidacy(
        ctx: Context<CloseCandidacy>,
        listing_id: u64,
        round: u64,
        lawyer: Pubkey,
    ) -> Result<()> {
        election::close_candidacy_handler(ctx, listing_id, round, lawyer)
    }

    pub fn finalize_spv_election<'info>(
        ctx: Context<'info, FinalizeSpvElection<'info>>,
        listing_id: u64,
    ) -> Result<()> {
        election::finalize_spv_election_handler(ctx, listing_id)
    }

    pub fn unlock_voting_shares(
        ctx: Context<UnlockVotingShares>,
        listing_id: u64,
        round: u64,
    ) -> Result<()> {
        election::unlock_voting_shares_handler(ctx, listing_id, round)
    }

    pub fn resign_from_case(ctx: Context<ResignFromCase>, listing_id: u64) -> Result<()> {
        legal::resign_from_case_handler(ctx, listing_id)
    }

    pub fn lawyer_confirm_documents(
        ctx: Context<ConfirmDocuments>,
        listing_id: u64,
        approve: bool,
        documents_hash: [u8; 32],
    ) -> Result<()> {
        legal::confirm_documents_handler(ctx, listing_id, approve, documents_hash)
    }

    pub fn close_case(ctx: Context<CloseCase>, listing_id: u64, lawyer: Pubkey) -> Result<()> {
        legal::close_case_handler(ctx, listing_id, lawyer)
    }

    pub fn resolve_silent_verdict(
        ctx: Context<ResolveSilentVerdict>,
        listing_id: u64,
    ) -> Result<()> {
        legal::resolve_silent_verdict_handler(ctx, listing_id)
    }

    pub fn execute_deal<'info>(
        ctx: Context<'info, ExecuteDeal<'info>>,
        listing_id: u64,
    ) -> Result<()> {
        settlement::execute_deal_handler(ctx, listing_id)
    }

    pub fn withdraw_cancelled(ctx: Context<WithdrawExpired>, listing_id: u64) -> Result<()> {
        withdraw::withdraw_cancelled_handler(ctx, listing_id)
    }

    pub fn settle_cancelled_fees(ctx: Context<SettleCancelledFees>, listing_id: u64) -> Result<()> {
        withdraw::settle_cancelled_fees_handler(ctx, listing_id)
    }

    pub fn reserve_shares(
        ctx: Context<ReserveShares>,
        listing_id: u64,
        amount: u32,
        max_total_cost: u64,
    ) -> Result<()> {
        reserve::reserve_shares_handler(ctx, listing_id, amount, max_total_cost)
    }

    pub fn claim_shares(ctx: Context<ClaimShares>, listing_id: u64) -> Result<()> {
        reserve::claim_shares_handler(ctx, listing_id)
    }

    pub fn release_reservation(
        ctx: Context<ReleaseReservation>,
        listing_id: u64,
        investor: Pubkey,
    ) -> Result<()> {
        reserve::release_reservation_handler(ctx, listing_id, investor)
    }

    pub fn close_reservation(ctx: Context<CloseReservation>) -> Result<()> {
        reserve::close_reservation_handler(ctx)
    }

    pub fn relist_shares(
        ctx: Context<RelistShares>,
        asset_id: u64,
        amount: u32,
        share_price: u64,
    ) -> Result<()> {
        secondary::relist_shares_handler(ctx, asset_id, amount, share_price)
    }

    pub fn delist_shares(ctx: Context<DelistShares>) -> Result<()> {
        secondary::delist_shares_handler(ctx)
    }

    pub fn buy_relisted_shares<'info>(
        ctx: Context<'info, BuyRelistedShares<'info>>,
        asset_id: u64,
        id: u64,
        amount: u32,
        max_total_cost: u64,
    ) -> Result<()> {
        secondary::buy_relisted_shares_handler(ctx, asset_id, id, amount, max_total_cost)
    }

    pub fn close_share_holding(ctx: Context<CloseShareHolding>) -> Result<()> {
        secondary::close_share_holding_handler(ctx)
    }

    pub fn make_offer(
        ctx: Context<MakeOffer>,
        id: u64,
        amount: u32,
        share_price: u64,
    ) -> Result<()> {
        offers::make_offer_handler(ctx, id, amount, share_price)
    }

    pub fn accept_offer<'info>(
        ctx: Context<'info, AcceptOffer<'info>>,
        id: u64,
        nonce: u64,
    ) -> Result<()> {
        offers::accept_offer_handler(ctx, id, nonce)
    }

    pub fn reject_offer(ctx: Context<RejectOffer>, id: u64, nonce: u64) -> Result<()> {
        offers::reject_offer_handler(ctx, id, nonce)
    }

    pub fn cancel_offer(ctx: Context<CancelOffer>) -> Result<()> {
        offers::cancel_offer_handler(ctx)
    }

    pub fn send_property_shares<'info>(
        ctx: Context<'info, SendShares<'info>>,
        asset_id: u64,
        amount: u32,
    ) -> Result<()> {
        secondary::send_property_shares_handler(ctx, asset_id, amount)
    }

    pub fn lock_shares(
        ctx: Context<AdjustShareLock>,
        asset_id: u64,
        owner: Pubkey,
        reason: LockReason,
        amount: u32,
    ) -> Result<()> {
        locks::lock_shares_handler(ctx, asset_id, owner, reason, amount)
    }

    pub fn unlock_shares(
        ctx: Context<AdjustShareLock>,
        asset_id: u64,
        owner: Pubkey,
        reason: LockReason,
        amount: u32,
    ) -> Result<()> {
        locks::unlock_shares_handler(ctx, asset_id, owner, reason, amount)
    }
}
