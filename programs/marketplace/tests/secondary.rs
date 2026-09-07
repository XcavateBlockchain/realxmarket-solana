//! The secondary share market: relist part of a holding, delist it again,
//! and buy relisted shares with both parties' income settled at their
//! pre-trade balances. Trades exist only on finalized properties; listed
//! shares stay on the seller's ledger but leave their voting and transfer
//! headroom.

mod common;
use common::*;

use anchor_lang::Discriminator;

const ASK: u64 = 6_000_000_000;

fn relist(svm: &mut LiteSVM, seller: &Keypair, id: u64, amount: u32) {
    ok(
        svm,
        relist_ix(&seller.pubkey(), 0, id, amount, ASK),
        seller,
        &[seller],
    );
}

#[test]
fn settle_income_discriminator_matches_the_property_program() {
    assert_eq!(
        marketplace::instructions::secondary::SETTLE_INCOME_DISC,
        <property::instruction::SettleIncome as Discriminator>::DISCRIMINATOR
    );
}

// --- relist / delist ---

#[test]
fn relist_reserves_the_shares() {
    let (mut svm, _admin, investors) = finalized_property();
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 20);

    let listing = share_listing_of(&svm, 0);
    assert_eq!(listing.seller, seller.pubkey());
    assert_eq!(listing.asset_id, 0);
    assert_eq!(listing.amount, 20);
    assert_eq!(listing.share_price, ASK);
    // The region's fees (1% each in the fixture), snapshotted at relist.
    assert_eq!(listing.seller_fee_bps, 100);
    assert_eq!(listing.buyer_fee_bps, 100);

    let holding = holding_of(&svm, 0, &seller.pubkey());
    assert_eq!(holding.amount, 33);
    assert_eq!(holding.listed, 20);
    assert_eq!(holding.transferable(), 13);

    // A second listing works against the reduced headroom, ids are
    // monotonic, and overshooting what's left is rejected.
    relist(&mut svm, seller, 1, 13);
    assert_eq!(holding_of(&svm, 0, &seller.pubkey()).listed, 33);
    fails_with(
        &mut svm,
        relist_ix(&seller.pubkey(), 0, 2, 1, ASK),
        seller,
        &[seller],
        "NotEnoughShares",
    );
}

#[test]
fn relist_rejects_vote_locked_shares() {
    let (mut svm, _admin, investors) = finalized_property();
    // Investor 0 still carries their 34-share election lock.
    let seller = &investors[0];
    fails_with(
        &mut svm,
        relist_ix(&seller.pubkey(), 0, 0, 1, ASK),
        seller,
        &[seller],
        "NotEnoughShares",
    );
    ok(
        &mut svm,
        unlock_votes_ix(&seller.pubkey(), 0, 1),
        seller,
        &[seller],
    );
    relist(&mut svm, seller, 0, 34);
}

#[test]
fn relist_waits_for_the_settlement() {
    let (mut svm, _admin, investors) = build_property(false);
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Legal);
    let seller = &investors[1];
    fails_with(
        &mut svm,
        relist_ix(&seller.pubkey(), 0, 0, 10, ASK),
        seller,
        &[seller],
        "PropertyNotFinalized",
    );
}

#[test]
fn relist_validates_the_ask() {
    let (mut svm, _admin, investors) = finalized_property();
    let seller = &investors[1];
    fails_with(
        &mut svm,
        relist_ix(&seller.pubkey(), 0, 0, 0, ASK),
        seller,
        &[seller],
        "InvalidShareAmount",
    );
    // Below one whole unit of the smallest-decimals mint.
    fails_with(
        &mut svm,
        relist_ix(&seller.pubkey(), 0, 0, 10, 999),
        seller,
        &[seller],
        "InvalidSharePrice",
    );
}

#[test]
fn delist_frees_the_reserve() {
    let (mut svm, _admin, investors) = finalized_property();
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 20);

    // Another holder can't delist someone else's listing, even naming the
    // right rent payer.
    let outsider = &investors[2];
    let mut hijack = delist_ix(&outsider.pubkey(), 0, 0);
    hijack.accounts[1].pubkey = seller.pubkey();
    fails_with(&mut svm, hijack, outsider, &[outsider], "WrongSeller");
    let before = svm.get_balance(&seller.pubkey()).unwrap();
    ok(
        &mut svm,
        delist_ix(&seller.pubkey(), 0, 0),
        seller,
        &[seller],
    );
    assert_eq!(holding_of(&svm, 0, &seller.pubkey()).listed, 0);
    assert!(svm.get_account(&share_listing_pda(0)).is_none());
    assert!(svm.get_balance(&seller.pubkey()).unwrap() > before);
}

// --- buying ---

#[test]
fn buy_pays_out_and_moves_shares() {
    let (mut svm, admin, investors) = finalized_property();
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 20);

    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 150_000_000_000);
    let treasury_before = token_balance(&svm, &treasury_payment_ata());
    ok_with_budget(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 12, u64::MAX),
        &buyer,
        &[&buyer],
    );

    // 12 shares at 6 GBP = 72 GBP; the buyer pays 1% on top, the seller
    // nets 1% less, and the 1.44 GBP of fees splits 67/33 between the
    // region's operator and the treasury.
    assert_eq!(tgbp_balance(&svm, &buyer.pubkey()), 77_280_000_000);
    assert_eq!(
        token_balance(&svm, &payment_ata(&seller.pubkey(), &tgbp_mint())),
        71_280_000_000
    );
    assert_eq!(
        token_balance(
            &svm,
            &payment_ata(&region_operator().pubkey(), &tgbp_mint())
        ),
        964_800_000
    );
    assert_eq!(
        token_balance(&svm, &treasury_payment_ata()) - treasury_before,
        475_200_000
    );

    // Ledger and token side agree; the listing keeps the remainder.
    let seller_holding = holding_of(&svm, 0, &seller.pubkey());
    assert_eq!(seller_holding.amount, 21);
    assert_eq!(seller_holding.listed, 8);
    let buyer_holding = holding_of(&svm, 0, &buyer.pubkey());
    assert_eq!(buyer_holding.amount, 12);
    assert_eq!(
        token_balance(&svm, &investor_share_ata(0, &seller.pubkey())),
        21
    );
    assert_eq!(
        token_balance(&svm, &investor_share_ata(0, &buyer.pubkey())),
        12
    );
    assert_eq!(property_of(&svm, 0).holder_count, 5);
    assert_eq!(share_listing_of(&svm, 0).amount, 8);

    // The rest sells out; the listing closes.
    ok_with_budget(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 8, u64::MAX),
        &buyer,
        &[&buyer],
    );
    assert!(svm.get_account(&share_listing_pda(0)).is_none());
    assert_eq!(holding_of(&svm, 0, &seller.pubkey()).listed, 0);
    assert_eq!(holding_of(&svm, 0, &buyer.pubkey()).amount, 20);
}

#[test]
fn buy_validates_the_request() {
    let (mut svm, admin, investors) = finalized_property();
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 10);
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 100_000_000_000);

    fails_with(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 11, u64::MAX),
        &buyer,
        &[&buyer],
        "NotEnoughSharesListed",
    );
    // 10 shares cost 60 GBP plus the 1% buyer fee; a cap at the bare price
    // must reject, proving the cap covers the full outlay.
    fails_with(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 10, 60_000_000_000),
        &buyer,
        &[&buyer],
        "CostTooHigh",
    );
    give_xcav(&mut svm, &buyer.pubkey(), 1_000_000_000);
    fails_with(
        &mut svm,
        buy_relisted_ix_with_mint(
            &buyer.pubkey(),
            0,
            0,
            &seller.pubkey(),
            10,
            u64::MAX,
            xcav_mint(),
            token_acc(&buyer.pubkey()),
        ),
        &buyer,
        &[&buyer],
        "MintNotAccepted",
    );
}

#[test]
fn buy_respects_the_ownership_cap() {
    let (mut svm, _admin, investors) = finalized_property();
    // Cap is 50% of 100 shares, strictly below: 49 at most. Investor 0
    // holds 34 and may only add 15 more.
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 20);
    let buyer = &investors[0];
    give_tgbp(&mut svm, &buyer.pubkey(), 200_000_000_000);
    ok(
        &mut svm,
        unlock_votes_ix(&buyer.pubkey(), 0, 1),
        buyer,
        &[buyer],
    );
    fails_with(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 16, u64::MAX),
        buyer,
        &[buyer],
        "MaxOwnershipExceeded",
    );
    ok_with_budget(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 15, u64::MAX),
        buyer,
        &[buyer],
    );
    assert_eq!(holding_of(&svm, 0, &buyer.pubkey()).amount, 49);
}

#[test]
fn buying_own_listing_is_blocked() {
    let (mut svm, _admin, investors) = finalized_property();
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 10);
    give_tgbp(&mut svm, &seller.pubkey(), 100_000_000_000);
    fails_with(
        &mut svm,
        buy_relisted_ix(&seller.pubkey(), 0, 0, &seller.pubkey(), 5, u64::MAX),
        seller,
        &[seller],
        "DuplicateMutableAccount",
    );
}

// Like accept_offer, the settle-heavy buy carries an explicit budget
// (SECONDARY_TRADE_BUDGET); the pin sits above the observed wobble but
// well under that budget, so a structural regression still fails loudly.
#[test]
fn buy_stays_within_the_compute_budget() {
    let (mut svm, admin, investors) = finalized_property();
    svm.add_program(marketplace::PROPERTY_PROGRAM, &program_bytes("property"))
        .unwrap();
    seed_income_stream(&mut svm, 2_000_000_000);
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 20);
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 100_000_000_000);

    let used = process_with_budget(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 12, u64::MAX),
        &buyer,
        &[&buyer],
    )
    .unwrap()
    .compute_units_consumed;
    assert!(
        used <= 250_000,
        "buy_relisted_shares used {used} CU, over the 250000 pin"
    );
}

#[test]
fn buy_settles_income_for_both_sides() {
    let (mut svm, admin, investors) = finalized_property();
    svm.add_program(marketplace::PROPERTY_PROGRAM, &program_bytes("property"))
        .unwrap();
    // 2 GBP per share accrued before the trade.
    seed_income_stream(&mut svm, 2_000_000_000);

    let seller = &investors[1];
    relist(&mut svm, seller, 0, 20);
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 100_000_000_000);
    ok_with_budget(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 12, u64::MAX),
        &buyer,
        &[&buyer],
    );

    // The seller banked 33 shares' worth at the pre-trade balance; the
    // buyer's checkpoint opened at the current accumulator with nothing
    // banked, so they earn only from here on.
    let seller_cp = checkpoint_of(&svm, &seller.pubkey());
    assert_eq!(seller_cp.entries[0].pending, 66_000_000_000);
    assert_eq!(seller_cp.entries[0].per_share, 2_000_000_000);
    let buyer_cp = checkpoint_of(&svm, &buyer.pubkey());
    assert_eq!(buyer_cp.entries[0].pending, 0);
    assert_eq!(buyer_cp.entries[0].per_share, 2_000_000_000);
}

#[test]
fn buy_needs_the_real_income_ledger() {
    let (mut svm, admin, investors) = finalized_property();
    seed_income_stream(&mut svm, 2_000_000_000);
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 10);
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 100_000_000_000);

    // A stand-in income account can't skip the settlements.
    let mut ix = buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 5, u64::MAX);
    let fake = Pubkey::new_unique();
    swap_account(&mut ix, property_income_pda(0), fake);
    fails_with(&mut svm, ix, &buyer, &[&buyer], "WrongIncomeLedger");
}

// The operator fee follows the region record; none of its accounts can be
// swapped to redirect it.
#[test]
fn buy_pins_the_region_and_its_owner() {
    let (mut svm, admin, investors) = finalized_property();
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 10);
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 100_000_000_000);

    // A forged region account isn't owned by the regions program.
    let mut ix = buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 5, u64::MAX);
    swap_account(&mut ix, region_pda(1), buyer.pubkey());
    fails_with(
        &mut svm,
        ix,
        &buyer,
        &[&buyer],
        "AccountOwnedByWrongProgram",
    );

    // A genuine region under someone else fails the id match.
    let intruder = funded(&mut svm);
    seed_region(&mut svm, 2, &intruder.pubkey());
    let mut ix = buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 5, u64::MAX);
    swap_account(&mut ix, region_pda(1), region_pda(2));
    fails_with(&mut svm, ix, &buyer, &[&buyer], "WrongRegionAccount");

    // A wrong payee behind the real region fails the owner check.
    let mut ix = buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 5, u64::MAX);
    swap_account(&mut ix, region_operator().pubkey(), intruder.pubkey());
    fails_with(&mut svm, ix, &buyer, &[&buyer], "WrongPayee");
}

// --- emptied holdings ---

#[test]
fn emptied_holding_closes_and_leaves_the_count() {
    let (mut svm, admin, investors) = finalized_property();
    let seller = &investors[3];
    relist(&mut svm, seller, 0, 9);
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 100_000_000_000);
    ok_with_budget(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 9, u64::MAX),
        &buyer,
        &[&buyer],
    );
    assert_eq!(holding_of(&svm, 0, &seller.pubkey()).amount, 0);
    assert_eq!(property_of(&svm, 0).holder_count, 5);

    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        close_holding_ix(&cranker.pubkey(), 0, &seller.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert!(svm.get_account(&holding_pda(0, &seller.pubkey())).is_none());
    assert_eq!(property_of(&svm, 0).holder_count, 4);
}

#[test]
fn close_rejects_a_holding_still_in_use() {
    let (mut svm, _admin, investors) = finalized_property();
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        close_holding_ix(&cranker.pubkey(), 0, &investors[1].pubkey()),
        &cranker,
        &[&cranker],
        "HoldingNotEmpty",
    );
}

#[test]
fn anothers_record_cannot_clear_the_buyer() {
    let (mut svm, admin, investors) = finalized_property();
    let seller = &investors[1];
    relist(&mut svm, seller, 0, 10);
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 100_000_000_000);

    // The seller's (cleared) compliance record standing in for the buyer's.
    let mut ix = buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 5, u64::MAX);
    swap_account(
        &mut ix,
        compliance_pda(&buyer.pubkey()),
        compliance_pda(&seller.pubkey()),
    );
    fails_with(&mut svm, ix, &buyer, &[&buyer], "WrongRegistryAccount");
}

#[test]
fn send_needs_the_real_income_ledger() {
    let (mut svm, _admin, investors) = finalized_property();
    let (sender, receiver) = (&investors[1], &investors[2]);

    let mut ix = send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 5);
    let fake = Pubkey::new_unique();
    swap_account(&mut ix, property_income_pda(0), fake);
    fails_with(&mut svm, ix, sender, &[sender], "WrongIncomeLedger");
}

#[test]
fn send_rejects_a_decoy_share_account() {
    let (mut svm, _admin, investors) = finalized_property();
    let (sender, receiver) = (&investors[1], &investors[2]);

    // The receiver's account standing where the sender's must be.
    let mut ix = send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 5);
    swap_account(
        &mut ix,
        investor_share_ata(0, &sender.pubkey()),
        investor_share_ata(0, &receiver.pubkey()),
    );
    fails_with(&mut svm, ix, sender, &[sender], "WrongTokenAccount");
}
