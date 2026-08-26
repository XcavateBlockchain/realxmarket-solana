//! The reserve-then-claim primary market: money stays in the investor's
//! wallet until the SPV exists, the last reserved share locks the sale in
//! and lets the SPV attest, claims move the money and deliver the shares,
//! and the direct market opens once the claim window runs out.

mod common;
use common::*;

use marketplace::state::ListingStatus;

/// Full pipeline up to an open listing.
fn setup_listed() -> (LiteSVM, Keypair, Keypair) {
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
    (svm, admin, sponsor())
}

fn reserve(svm: &mut LiteSVM, investor: &Keypair, sponsor: &Keypair, amount: u32) {
    ok(
        svm,
        reserve_ix(&investor.pubkey(), &sponsor.pubkey(), 0, amount, u64::MAX),
        sponsor,
        &[sponsor, investor],
    );
}

fn attest_spv(svm: &mut LiteSVM, admin: &Keypair) {
    let confirmer = new_confirmer(svm, admin);
    ok(
        svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
    );
}

/// Cost of `amount` shares at the default settings: price + 1% fee + 3% tax.
fn total_of(amount: u32) -> u64 {
    let price = SHARE_PRICE * amount as u64;
    price + price / 100 + price * 300 / 10_000
}

#[test]
fn reserve_leaves_the_money_in_the_wallet() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    let before = tgbp_balance(&svm, &investor.pubkey());
    reserve(&mut svm, &investor, &sponsor, 10);

    // Nothing moved and nothing was delivered; only the ledger changed.
    assert_eq!(tgbp_balance(&svm, &investor.pubkey()), before);
    assert!(svm.get_account(&listing_payment_ata(0)).is_none());
    let position = position_of(&svm, 0, &investor.pubkey());
    assert_eq!(position.reserved_share_amount, 10);
    assert_eq!(position.share_amount, 0);
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.reserved_share_amount, 10);
    assert_eq!(listing.sold_share_amount, 0);
    assert_eq!(
        reservation_of(&svm, &tgbp_acc(&investor.pubkey())).amount,
        total_of(10)
    );
}

#[test]
fn claim_moves_money_and_delivers() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    let before = tgbp_balance(&svm, &investor.pubkey());
    reserve(&mut svm, &investor, &sponsor, 10);
    let _fillers = fill_reserve(&mut svm, &admin);
    attest_spv(&mut svm, &admin);
    ok(
        &mut svm,
        claim_ix(&investor.pubkey(), &sponsor.pubkey(), 0),
        &sponsor,
        &[&sponsor, &investor],
    );

    assert_eq!(
        tgbp_balance(&svm, &investor.pubkey()),
        before - total_of(10)
    );
    let position = position_of(&svm, 0, &investor.pubkey());
    assert_eq!(position.reserved_share_amount, 0);
    assert_eq!(position.share_amount, 10);
    assert_eq!(holding_of(&svm, 0, &investor.pubkey()).amount, 10);
    let listing = listing_of(&svm, 0);
    // The fillers' 90 shares stay reserved; only the claimed ones convert.
    assert_eq!(listing.reserved_share_amount, 90);
    assert_eq!(listing.sold_share_amount, 10);
    // The reservation wound down; its rent is reclaimable.
    assert_eq!(
        reservation_of(&svm, &tgbp_acc(&investor.pubkey())).amount,
        0
    );
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        close_reservation_ix(&cranker.pubkey(), &tgbp_acc(&investor.pubkey())),
        &cranker,
        &[&cranker],
    );
}

#[test]
fn claim_needs_the_spv() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    reserve(&mut svm, &investor, &sponsor, 10);
    fails_with(
        &mut svm,
        claim_ix(&investor.pubkey(), &sponsor.pubkey(), 0),
        &sponsor,
        &[&sponsor, &investor],
        "ClaimWindowClosed",
    );
}

#[test]
fn claim_stops_at_the_window() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    reserve(&mut svm, &investor, &sponsor, 10);
    let _fillers = fill_reserve(&mut svm, &admin);
    attest_spv(&mut svm, &admin);
    warp(&mut svm, CLAIMING_TIME + 1);
    fails_with(
        &mut svm,
        claim_ix(&investor.pubkey(), &sponsor.pubkey(), 0),
        &sponsor,
        &[&sponsor, &investor],
        "ClaimWindowClosed",
    );
}

// At the deadline second claims are closed and the sweep is open; once every
// reservation is swept, so is the direct market.
#[test]
fn the_deadline_second_opens_the_direct_market() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    reserve(&mut svm, &investor, &sponsor, 10);
    let fillers = fill_reserve(&mut svm, &admin);
    attest_spv(&mut svm, &admin);

    let deadline = listing_of(&svm, 0).claim_deadline;
    warp_to(&mut svm, deadline);
    fails_with(
        &mut svm,
        claim_ix(&investor.pubkey(), &sponsor.pubkey(), 0),
        &sponsor,
        &[&sponsor, &investor],
        "ClaimWindowClosed",
    );
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        release_reservation_ix(&cranker.pubkey(), 0, &investor.pubkey()),
        &cranker,
        &[&cranker],
    );
    for filler in &fillers {
        ok(
            &mut svm,
            release_reservation_ix(&cranker.pubkey(), 0, &filler.pubkey()),
            &cranker,
            &[&cranker],
        );
    }
    let other = new_investor(&mut svm, &admin);
    ok(
        &mut svm,
        buy_ix(&other.pubkey(), &sponsor.pubkey(), 0, 5, u64::MAX),
        &sponsor,
        &[&sponsor, &other],
    );
}

#[test]
fn claims_can_sell_the_listing_out() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investors = [
        new_investor(&mut svm, &admin),
        new_investor(&mut svm, &admin),
        new_investor(&mut svm, &admin),
    ];
    for (investor, amount) in investors.iter().zip([34, 33, 33]) {
        reserve(&mut svm, investor, &sponsor, amount);
    }
    attest_spv(&mut svm, &admin);
    for investor in &investors {
        ok(
            &mut svm,
            claim_ix(&investor.pubkey(), &sponsor.pubkey(), 0),
            &sponsor,
            &[&sponsor, investor],
        );
    }
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.status, ListingStatus::SoldOut);
    assert!(listing.legal_deadline > 0);
}

#[test]
fn release_sweeps_a_missed_claim() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    reserve(&mut svm, &investor, &sponsor, 10);
    let fillers = fill_reserve(&mut svm, &admin);
    attest_spv(&mut svm, &admin);

    // While the claim can still happen, nobody may sweep it.
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        release_reservation_ix(&cranker.pubkey(), 0, &investor.pubkey()),
        &cranker,
        &[&cranker],
        "ListingStillActive",
    );
    warp(&mut svm, CLAIMING_TIME + 1);
    for missed in std::iter::once(&investor).chain(&fillers) {
        ok(
            &mut svm,
            release_reservation_ix(&cranker.pubkey(), 0, &missed.pubkey()),
            &cranker,
            &[&cranker],
        );
    }

    assert_eq!(
        reservation_of(&svm, &tgbp_acc(&investor.pubkey())).amount,
        0
    );
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.reserved_share_amount, 0);
    assert_eq!(listing.position_count, 0);
    // No fault: the investor may come back and buy directly.
    ok(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 10, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
    );
}

#[test]
fn direct_buy_waits_for_the_window() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    // Before the SPV there is no direct market at all.
    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 10, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "DirectBuyNotOpen",
    );
    reserve(&mut svm, &investor, &sponsor, 1);
    let fillers = fill_reserve(&mut svm, &admin);
    attest_spv(&mut svm, &admin);
    // While claims run, reserved money still has first call.
    let other = new_investor(&mut svm, &admin);
    fails_with(
        &mut svm,
        buy_ix(&other.pubkey(), &sponsor.pubkey(), 0, 10, u64::MAX),
        &sponsor,
        &[&sponsor, &other],
        "DirectBuyNotOpen",
    );
    warp(&mut svm, CLAIMING_TIME + 1);
    let cranker = funded(&mut svm);
    for missed in std::iter::once(&investor).chain(&fillers) {
        ok(
            &mut svm,
            release_reservation_ix(&cranker.pubkey(), 0, &missed.pubkey()),
            &cranker,
            &[&cranker],
        );
    }
    ok(
        &mut svm,
        buy_ix(&other.pubkey(), &sponsor.pubkey(), 0, 10, u64::MAX),
        &sponsor,
        &[&sponsor, &other],
    );
}

#[test]
fn stale_reservation_blocks_a_direct_buy() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    reserve(&mut svm, &investor, &sponsor, 10);
    let fillers = fill_reserve(&mut svm, &admin);
    attest_spv(&mut svm, &admin);
    warp(&mut svm, CLAIMING_TIME + 1);
    // Free the other shares so availability is not what stops the buy.
    let cranker = funded(&mut svm);
    for filler in &fillers {
        ok(
            &mut svm,
            release_reservation_ix(&cranker.pubkey(), 0, &filler.pubkey()),
            &cranker,
            &[&cranker],
        );
    }
    // The missed reservation must be swept before this wallet buys directly.
    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 5, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "ReservationOutstanding",
    );
}

#[test]
fn reserved_shares_count_toward_the_cap() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    reserve(&mut svm, &investor, &sponsor, 49);
    let second = new_investor(&mut svm, &admin);
    reserve(&mut svm, &second, &sponsor, 49);
    // 49 reserved plus 2 more crosses the strictly-below-50% cap.
    fails_with(
        &mut svm,
        reserve_ix(&second.pubkey(), &sponsor.pubkey(), 0, 2, u64::MAX),
        &sponsor,
        &[&sponsor, &second],
        "MaxOwnershipExceeded",
    );
}

#[test]
fn reserve_checks_the_wallet_balance() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    // Shrink the wallet below the cost of ten shares.
    give_tgbp(&mut svm, &investor.pubkey(), total_of(10) - 1);
    fails_with(
        &mut svm,
        reserve_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 10, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "BalanceTooLow",
    );
}

#[test]
fn reservations_survive_listing_expiry() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    let before = tgbp_balance(&svm, &investor.pubkey());
    reserve(&mut svm, &investor, &sponsor, 10);
    // The sale dies with the money never moved; the sweep frees it in place.
    warp(&mut svm, LISTING_DURATION + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        release_reservation_ix(&cranker.pubkey(), 0, &investor.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(tgbp_balance(&svm, &investor.pubkey()), before);
    assert_eq!(
        reservation_of(&svm, &tgbp_acc(&investor.pubkey())).amount,
        0
    );
}

#[test]
fn claim_works_with_an_exactly_reserved_balance() {
    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    // The wallet holds not one unit more than the reservation.
    give_tgbp(&mut svm, &investor.pubkey(), total_of(10));
    reserve(&mut svm, &investor, &sponsor, 10);
    let _fillers = fill_reserve(&mut svm, &admin);
    attest_spv(&mut svm, &admin);
    ok(
        &mut svm,
        claim_ix(&investor.pubkey(), &sponsor.pubkey(), 0),
        &sponsor,
        &[&sponsor, &investor],
    );
    assert_eq!(tgbp_balance(&svm, &investor.pubkey()), 0);
}

// Paying in the 6-decimal mint runs the whole reserve/claim math at a real
// rescale factor: every component divides by 1_000.
#[test]
fn claim_pays_in_the_position_mint() {
    use anchor_lang::solana_program::program_pack::Pack;

    let (mut svm, admin, sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    give_gbp6(&mut svm, &investor.pubkey(), 1_000_000_000);

    let vault_ata = Pubkey::find_program_address(
        &[
            listing_vault_pda(0).as_ref(),
            anchor_spl::token::ID.as_ref(),
            gbp6_mint().as_ref(),
        ],
        &anchor_spl::associated_token::ID,
    )
    .0;
    ok(
        &mut svm,
        reserve_ix_with_mint(
            &investor.pubkey(),
            &sponsor.pubkey(),
            0,
            10,
            u64::MAX,
            gbp6_mint(),
            gbp6_acc(&investor.pubkey()),
        ),
        &sponsor,
        &[&sponsor, &investor],
    );
    let _fillers = fill_reserve(&mut svm, &admin);
    attest_spv(&mut svm, &admin);
    ok(
        &mut svm,
        claim_ix_with_mint(
            &investor.pubkey(),
            &sponsor.pubkey(),
            0,
            gbp6_mint(),
            gbp6_acc(&investor.pubkey()),
            vault_ata,
        ),
        &sponsor,
        &[&sponsor, &investor],
    );

    let vault_acc = svm.get_account(&vault_ata).unwrap();
    let vault_state =
        anchor_spl::token::spl_token::state::Account::unpack(&vault_acc.data).unwrap();
    assert_eq!(vault_state.amount, total_of(10) / 1_000);
    let investor_acc = svm.get_account(&gbp6_acc(&investor.pubkey())).unwrap();
    let investor_state =
        anchor_spl::token::spl_token::state::Account::unpack(&investor_acc.data).unwrap();
    assert_eq!(investor_state.amount, 1_000_000_000 - total_of(10) / 1_000);
}
