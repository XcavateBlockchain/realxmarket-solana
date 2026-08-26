//! The SPV attestation and the dead-listing exits: investors pulling out of
//! an expired sale, and the developer reclaiming the deposit when nothing
//! (or nothing anymore) is sold.

mod common;
use common::*;

use marketplace::state::ListingStatus;

/// Full pipeline up to an open listing, returning the developer too.
fn setup_listed() -> (LiteSVM, Keypair, Keypair, Keypair) {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    (svm, admin, sponsor(), developer)
}

fn buy(svm: &mut LiteSVM, admin: &Keypair, investor: &Keypair, amount: u32) {
    acquire(svm, admin, investor, amount);
}

// ============================ create_spv ============================

#[test]
fn create_spv_records_the_attestation() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    // The attestation needs every share reserved; it locks the sale in and
    // opens the claim window.
    let _investors = fill_reserve(&mut svm, &admin);

    let confirmer = new_confirmer(&mut svm, &admin);
    ok(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
    );
    assert!(property_of(&svm, 0).spv_created);
    assert!(listing_of(&svm, 0).claim_deadline > 0);

    // Attesting twice makes no sense.
    fails_with(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
        "SpvAlreadyCreated",
    );
}

#[test]
fn create_spv_requires_role() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    let _investors = fill_reserve(&mut svm, &admin);

    let stranger = funded(&mut svm);
    fails_with(
        &mut svm,
        create_spv_ix(&stranger.pubkey(), 0),
        &stranger,
        &[&stranger],
        "AccountNotInitialized",
    );
}

#[test]
fn create_spv_requires_full_reservation() {
    let (mut svm, admin, sponsor, _developer) = setup_listed();
    let confirmer = new_confirmer(&mut svm, &admin);
    fails_with(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
        "NotFullyReserved",
    );

    // A nearly full sale is still not locked in.
    let investor = new_investor(&mut svm, &admin);
    ok(
        &mut svm,
        reserve_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 49, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
    );
    fails_with(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
        "NotFullyReserved",
    );
}

// ============================ withdraw_expired ============================

#[test]
fn withdraw_expired_refunds_and_closes() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    let tgbp_before = tgbp_balance(&svm, &investor.pubkey());
    buy(&mut svm, &admin, &investor, 10);

    warp(&mut svm, LISTING_DURATION + 1);
    ok(
        &mut svm,
        withdraw_expired_ix(&investor.pubkey(), 0),
        &investor,
        &[&investor],
    );

    assert_eq!(tgbp_balance(&svm, &investor.pubkey()), tgbp_before);
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Expired);
    assert_eq!(listing_of(&svm, 0).sold_share_amount, 0);
    assert_eq!(property_of(&svm, 0).holder_count, 0);
    // Both per-investor accounts are gone; a dead listing needs neither.
    assert!(svm
        .get_account(&position_pda(0, &investor.pubkey()))
        .is_none_or(|a| a.data.is_empty()));
    assert!(svm
        .get_account(&holding_pda(0, &investor.pubkey()))
        .is_none_or(|a| a.data.is_empty()));
}

#[test]
fn withdraw_expired_before_expiry_fails() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    buy(&mut svm, &admin, &investor, 10);

    fails_with(
        &mut svm,
        withdraw_expired_ix(&investor.pubkey(), 0),
        &investor,
        &[&investor],
        "ListingNotExpired",
    );
}

// Entries check `< expiry` and exits `>= expiry`, so the expiry second itself
// already belongs to the exit and no instant is open to both or neither.
#[test]
fn the_expiry_second_belongs_to_the_exit() {
    let (mut svm, admin, sponsor, _developer) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    buy(&mut svm, &admin, &investor, 10);

    let expiry = listing_of(&svm, 0).listing_expiry;
    warp_to(&mut svm, expiry);
    let late = new_investor(&mut svm, &admin);
    fails_with(
        &mut svm,
        reserve_ix(&late.pubkey(), &sponsor.pubkey(), 0, 1, u64::MAX),
        &sponsor,
        &[&sponsor, &late],
        "ListingExpired",
    );
    ok(
        &mut svm,
        withdraw_expired_ix(&investor.pubkey(), 0),
        &investor,
        &[&investor],
    );
}

#[test]
fn withdraw_expired_after_sellout_fails() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    let (a, _b, _c) = sell_out_three(&mut svm, &admin);

    // A sold-out sale belongs to the legal phase, expiry or not.
    warp(&mut svm, LISTING_DURATION + 1);
    fails_with(
        &mut svm,
        withdraw_expired_ix(&a.pubkey(), 0),
        &a,
        &[&a],
        "ListingNotActive",
    );
}

fn sell_out_three(svm: &mut LiteSVM, admin: &Keypair) -> (Keypair, Keypair, Keypair) {
    let a = new_investor(svm, admin);
    let b = new_investor(svm, admin);
    let c = new_investor(svm, admin);
    acquire_many(svm, admin, &[(&a, 34), (&b, 33), (&c, 33)]);
    (a, b, c)
}

#[test]
fn second_investor_withdraws_after_status_flip() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    let a = new_investor(&mut svm, &admin);
    let b = new_investor(&mut svm, &admin);
    acquire_many(&mut svm, &admin, &[(&a, 10), (&b, 20)]);

    warp(&mut svm, LISTING_DURATION + 1);
    ok(&mut svm, withdraw_expired_ix(&a.pubkey(), 0), &a, &[&a]);
    ok(&mut svm, withdraw_expired_ix(&b.pubkey(), 0), &b, &[&b]);

    assert_eq!(listing_of(&svm, 0).sold_share_amount, 0);
    assert_eq!(property_of(&svm, 0).holder_count, 0);
}

// Expiry also frees cancelled positions for the crank.
#[test]
fn expiry_flip_opens_the_cancelled_crank() {
    let (mut svm, admin, sponsor, _developer) = setup_listed();
    let cancelled = new_investor(&mut svm, &admin);
    ok(
        &mut svm,
        reserve_ix(&cancelled.pubkey(), &sponsor.pubkey(), 0, 5, u64::MAX),
        &sponsor,
        &[&sponsor, &cancelled],
    );
    ok(
        &mut svm,
        unreserve_ix(&cancelled.pubkey(), 0),
        &cancelled,
        &[&cancelled],
    );
    let holder = new_investor(&mut svm, &admin);
    buy(&mut svm, &admin, &holder, 10);

    warp(&mut svm, LISTING_DURATION + 1);
    ok(
        &mut svm,
        withdraw_expired_ix(&holder.pubkey(), 0),
        &holder,
        &[&holder],
    );

    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        close_position_ix(&cranker.pubkey(), 0, &cancelled.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert!(svm
        .get_account(&position_pda(0, &cancelled.pubkey()))
        .is_none_or(|a| a.data.is_empty()));
}

// ============================ withdraw_deposit_unsold ============================

#[test]
fn deposit_returns_when_nothing_sold() {
    let (mut svm, _admin, _sponsor, developer) = setup_listed();
    warp(&mut svm, LISTING_DURATION + 1);

    let before = xcav_balance(&svm, &developer.pubkey());
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    assert_eq!(
        xcav_balance(&svm, &developer.pubkey()) - before,
        LISTING_DEPOSIT
    );
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Expired);

    fails_with(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
        "DepositAlreadyWithdrawn",
    );
}

// A listing abandoned before its assets exist has taken nobody's money; the
// developer can walk away at once.
#[test]
fn deposit_returns_from_pending_assets() {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let before = xcav_balance(&svm, &developer.pubkey());
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    assert_eq!(
        xcav_balance(&svm, &developer.pubkey()) - before,
        LISTING_DEPOSIT
    );
}

#[test]
fn deposit_requires_the_listing_developer() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    warp(&mut svm, LISTING_DURATION + 1);
    let other = new_developer(&mut svm, &admin);
    fails_with(
        &mut svm,
        withdraw_deposit_ix(&other.pubkey(), 0),
        &other,
        &[&other],
        "NotListingDeveloper",
    );
}

#[test]
fn deposit_blocked_while_shares_outstanding() {
    let (mut svm, admin, _sponsor, developer) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    buy(&mut svm, &admin, &investor, 10);
    warp(&mut svm, LISTING_DURATION + 1);

    fails_with(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
        "SharesOutstanding",
    );

    // Once the last investor withdraws, the deposit follows.
    ok(
        &mut svm,
        withdraw_expired_ix(&investor.pubkey(), 0),
        &investor,
        &[&investor],
    );
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
}

#[test]
fn deposit_blocked_before_expiry() {
    let (mut svm, _admin, _sponsor, developer) = setup_listed();
    fails_with(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
        "ListingNotExpired",
    );
}

// ============================ legal timeout + teardown ============================

// The success path must never be a trap: once the legal window runs out on a
// sold-out sale, every investor can leave with their money.
#[test]
fn legal_timeout_reopens_the_exits() {
    let (mut svm, admin, _sponsor, developer) = setup_listed();
    let (a, b, c) = sell_out_three(&mut svm, &admin);
    assert!(listing_of(&svm, 0).legal_deadline > 0);

    // Before the deadline the sale still belongs to the lawyers.
    fails_with(
        &mut svm,
        withdraw_legal_expired_ix(&a.pubkey(), 0),
        &a,
        &[&a],
        "LegalProcessNotExpired",
    );

    // Just past the default legal_process_time of 100_000.
    warp(&mut svm, 100_001);
    let before = tgbp_balance(&svm, &a.pubkey());
    ok(
        &mut svm,
        withdraw_legal_expired_ix(&a.pubkey(), 0),
        &a,
        &[&a],
    );
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Refunding);
    assert!(tgbp_balance(&svm, &a.pubkey()) > before);

    ok(
        &mut svm,
        withdraw_legal_expired_ix(&b.pubkey(), 0),
        &b,
        &[&b],
    );
    ok(
        &mut svm,
        withdraw_legal_expired_ix(&c.pubkey(), 0),
        &c,
        &[&c],
    );
    assert_eq!(listing_of(&svm, 0).sold_share_amount, 0);
    assert_eq!(property_of(&svm, 0).holder_count, 0);

    // The developer's deposit follows once everyone is out, minus the 1%
    // abandonment slash: no lawyer was ever appointed here.
    let dev_before = xcav_balance(&svm, &developer.pubkey());
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    assert_eq!(
        xcav_balance(&svm, &developer.pubkey()) - dev_before,
        LISTING_DEPOSIT - LISTING_DEPOSIT / 100
    );
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Refunding);
}

#[test]
fn legal_timeout_requires_sold_out() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    buy(&mut svm, &admin, &investor, 10);
    fails_with(
        &mut svm,
        withdraw_legal_expired_ix(&investor.pubkey(), 0),
        &investor,
        &[&investor],
        "ListingNotActive",
    );
}

// Full teardown: after everyone is refunded, the crank sweeps every account
// the listing left behind, rent flowing back to whoever paid it.
#[test]
fn close_dead_listing_sweeps_everything() {
    let (mut svm, admin, sponsor, developer) = setup_listed();
    let (a, b, c) = sell_out_three(&mut svm, &admin);
    warp(&mut svm, 100_001);
    ok(
        &mut svm,
        withdraw_legal_expired_ix(&a.pubkey(), 0),
        &a,
        &[&a],
    );
    ok(
        &mut svm,
        withdraw_legal_expired_ix(&b.pubkey(), 0),
        &b,
        &[&b],
    );
    ok(
        &mut svm,
        withdraw_legal_expired_ix(&c.pubkey(), 0),
        &c,
        &[&c],
    );
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let dev_before = svm.get_account(&developer.pubkey()).unwrap().lamports;
    let sponsor_before = svm.get_account(&sponsor.pubkey()).unwrap().lamports;
    let cranker = funded(&mut svm);
    // The triples must name the mints the listing collected, in order; a
    // different accepted mint doesn't do.
    fails_with(
        &mut svm,
        close_dead_listing_ix_for(
            &cranker.pubkey(),
            0,
            &developer.pubkey(),
            true,
            &[gbp6_mint()],
        ),
        &cranker,
        &[&cranker],
        "InvalidMint",
    );
    ok(
        &mut svm,
        close_dead_listing_ix(&cranker.pubkey(), 0, &developer.pubkey(), true),
        &cranker,
        &[&cranker],
    );

    for addr in [
        listing_pda(0),
        property_pda(0),
        share_mint_pda(0),
        vault_share_account(0),
        listing_payment_ata(0),
    ] {
        assert!(svm.get_account(&addr).is_none_or(|a| a.data.is_empty()));
    }
    assert!(svm.get_account(&developer.pubkey()).unwrap().lamports > dev_before);
    assert!(svm.get_account(&sponsor.pubkey()).unwrap().lamports > sponsor_before);
}

fn treasury_xcav_balance(svm: &LiteSVM) -> u64 {
    let acc = svm
        .get_account(&payment_ata(&treasury(), &xcav_mint()))
        .unwrap();
    let state: anchor_spl::token::spl_token::state::Account =
        anchor_lang::solana_program::program_pack::Pack::unpack(&acc.data).unwrap();
    state.amount
}

// A developer who never appoints their lawyer and lets the legal window die
// pays 1% of the bond to the treasury.
#[test]
fn abandoned_sale_slashes_the_bond() {
    let (mut svm, admin, _sponsor, developer) = setup_listed();
    let (a, b, c) = sell_out_three(&mut svm, &admin);
    warp(&mut svm, 100_001);
    for investor in [&a, &b, &c] {
        ok(
            &mut svm,
            withdraw_legal_expired_ix(&investor.pubkey(), 0),
            investor,
            &[investor],
        );
    }

    let before = xcav_balance(&svm, &developer.pubkey());
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    let slash = LISTING_DEPOSIT / 100;
    assert_eq!(
        xcav_balance(&svm, &developer.pubkey()) - before,
        LISTING_DEPOSIT - slash
    );
    assert_eq!(treasury_xcav_balance(&svm), slash);

    // The slashed listing still tears down.
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        close_dead_listing_ix(&cranker.pubkey(), 0, &developer.pubkey(), true),
        &cranker,
        &[&cranker],
    );
}

// The slash keys off whether the developer ever engaged counsel, not off the
// slot still being filled: freeing the lawyer from the dead sale first must
// not turn a timeout into an abandonment.
#[test]
fn engaged_developer_keeps_the_full_bond() {
    let (mut svm, admin, _sponsor, developer) = setup_listed();
    let (a, b, c) = sell_out_three(&mut svm, &admin);
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
    );
    warp(&mut svm, 100_001);
    for investor in [&a, &b, &c] {
        ok(
            &mut svm,
            withdraw_legal_expired_ix(&investor.pubkey(), 0),
            investor,
            &[investor],
        );
    }
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        close_case_ix(&cranker.pubkey(), 0, &lawyer.pubkey()),
        &cranker,
        &[&cranker],
    );

    let before = xcav_balance(&svm, &developer.pubkey());
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    assert_eq!(
        xcav_balance(&svm, &developer.pubkey()) - before,
        LISTING_DEPOSIT
    );
    assert_eq!(treasury_xcav_balance(&svm), 0);
}

// One base unit donated to a vault account must not pin the listing open;
// the sweep sends leftovers to the treasury instead of demanding zero.
#[test]
fn close_dead_listing_sweeps_donated_dust() {
    let (mut svm, admin, _sponsor, developer) = setup_listed();
    let (a, b, c) = sell_out_three(&mut svm, &admin);
    warp(&mut svm, 100_001);
    for investor in [&a, &b, &c] {
        ok(
            &mut svm,
            withdraw_legal_expired_ix(&investor.pubkey(), 0),
            investor,
            &[investor],
        );
    }
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        listing_payment_ata(0),
        &listing_vault_pda(0),
        5,
    );
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        treasury_payment_ata(),
        &treasury(),
        0,
    );
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        close_dead_listing_ix(&cranker.pubkey(), 0, &developer.pubkey(), true),
        &cranker,
        &[&cranker],
    );
    assert!(svm
        .get_account(&listing_payment_ata(0))
        .is_none_or(|acc| acc.data.is_empty()));
    let treasury_acc = svm.get_account(&treasury_payment_ata()).unwrap();
    let state: anchor_spl::token::spl_token::state::Account =
        anchor_lang::solana_program::program_pack::Pack::unpack(&treasury_acc.data).unwrap();
    assert_eq!(state.amount, 5);
}

// A mint rotated out of the config must not strand what a live listing's
// vault still holds in it: the sweep follows `listing.collected`, not the
// current accepted list.
#[test]
fn teardown_survives_a_config_mint_rotation() {
    // setup_listed drops the config authority, and this test needs it.
    let (mut svm, admin, authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    let investor = new_investor(&mut svm, &admin);
    buy(&mut svm, &admin, &investor, 10);
    warp(&mut svm, LISTING_DURATION + 1);
    ok(
        &mut svm,
        withdraw_expired_ix(&investor.pubkey(), 0),
        &investor,
        &[&investor],
    );
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    // tGBP, which this sale collected, gets dropped from the accepted list.
    let mut params = default_params();
    params.accepted_payment_mints = vec![gbp6_mint()];
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    // Dust left in the dropped mint still sweeps to the treasury and the
    // teardown completes.
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        listing_payment_ata(0),
        &listing_vault_pda(0),
        7,
    );
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        treasury_payment_ata(),
        &treasury(),
        0,
    );
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        close_dead_listing_ix(&cranker.pubkey(), 0, &developer.pubkey(), true),
        &cranker,
        &[&cranker],
    );
    assert_eq!(token_balance(&svm, &treasury_payment_ata()), 7);
    assert!(svm
        .get_account(&listing_pda(0))
        .is_none_or(|a| a.data.is_empty()));
}

#[test]
fn close_dead_listing_requires_terminal_status() {
    let (mut svm, _admin, _sponsor, developer) = setup_listed();
    // Expiry alone changes nothing; until a withdraw flips the status the
    // listing still reads Listed and the crank must refuse it.
    warp(&mut svm, LISTING_DURATION + 1);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        close_dead_listing_ix(&cranker.pubkey(), 0, &developer.pubkey(), true),
        &cranker,
        &[&cranker],
        "ListingNotActive",
    );
}

#[test]
fn close_dead_listing_requires_deposit_withdrawn() {
    let (mut svm, admin, _sponsor, developer) = setup_listed();
    // The investor's withdraw flips the listing to Expired and drains the
    // shares, leaving the deposit as the only thing still held.
    let investor = new_investor(&mut svm, &admin);
    buy(&mut svm, &admin, &investor, 10);
    warp(&mut svm, LISTING_DURATION + 1);
    ok(
        &mut svm,
        withdraw_expired_ix(&investor.pubkey(), 0),
        &investor,
        &[&investor],
    );

    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        close_dead_listing_ix(&cranker.pubkey(), 0, &developer.pubkey(), true),
        &cranker,
        &[&cranker],
        "DepositStillHeld",
    );
}

// A listing abandoned before its assets existed tears down without the mint
// half.
#[test]
fn close_dead_listing_from_pending_assets() {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let cranker = funded(&mut svm);
    // Nothing was ever collected, so the sweep takes no payment triples.
    ok(
        &mut svm,
        close_dead_listing_ix_for(&cranker.pubkey(), 0, &developer.pubkey(), false, &[]),
        &cranker,
        &[&cranker],
    );
    assert!(svm
        .get_account(&listing_pda(0))
        .is_none_or(|a| a.data.is_empty()));
    assert!(svm
        .get_account(&property_pda(0))
        .is_none_or(|a| a.data.is_empty()));
}

// A cancelled position needs the listing alive to close against, so teardown
// must wait for the crank even though the position carries no shares.
#[test]
fn teardown_waits_for_cancelled_positions() {
    let (mut svm, admin, sponsor, developer) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    ok(
        &mut svm,
        reserve_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 5, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
    );
    ok(
        &mut svm,
        unreserve_ix(&investor.pubkey(), 0),
        &investor,
        &[&investor],
    );

    warp(&mut svm, LISTING_DURATION + 1);
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        close_dead_listing_ix_for(&cranker.pubkey(), 0, &developer.pubkey(), true, &[]),
        &cranker,
        &[&cranker],
        "PositionsOutstanding",
    );

    // Sweep the position first; then the teardown goes through.
    ok(
        &mut svm,
        close_position_ix(&cranker.pubkey(), 0, &investor.pubkey()),
        &cranker,
        &[&cranker],
    );
    ok(
        &mut svm,
        close_dead_listing_ix_for(&cranker.pubkey(), 0, &developer.pubkey(), true, &[]),
        &cranker,
        &[&cranker],
    );
    assert!(svm
        .get_account(&listing_pda(0))
        .is_none_or(|a| a.data.is_empty()));
}

// ===================== the attestation meets expiry =====================

// An expired listing is the withdraw paths' territory: attesting it would
// promise a claim window the claims themselves refuse.
#[test]
fn attesting_an_expired_listing_fails() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    let _investors = fill_reserve(&mut svm, &admin);
    warp(&mut svm, LISTING_DURATION + 1);

    let confirmer = new_confirmer(&mut svm, &admin);
    fails_with(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
        "ListingExpired",
    );
}

// A late attestation still opens a window, but never one running past the
// expiry that claims are gated on.
#[test]
fn claim_window_never_outruns_the_listing() {
    let (mut svm, admin, _sponsor, _developer) = setup_listed();
    let _investors = fill_reserve(&mut svm, &admin);
    // 30k left on the listing, against a 50k claiming time.
    warp(&mut svm, LISTING_DURATION - 30_000);

    let confirmer = new_confirmer(&mut svm, &admin);
    ok(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
    );
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.claim_deadline, listing.listing_expiry);
}
