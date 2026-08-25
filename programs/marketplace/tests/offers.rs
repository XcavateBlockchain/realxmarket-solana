//! Offers on secondary share listings (bid held in the offer's vault from
//! make to settle, nonce-bound acceptance, refunds on reject and cancel)
//! and direct share transfers between compliant investors.

mod common;
use common::*;

const ASK: u64 = 6_000_000_000;
const BID: u64 = 5_000_000_000;

/// A finalized property with investor 1's 20 shares listed at 6 GBP.
fn listed_property() -> (LiteSVM, Keypair, Vec<Keypair>) {
    let (mut svm, admin, investors) = finalized_property();
    let seller = &investors[1];
    ok(
        &mut svm,
        relist_ix(&seller.pubkey(), 0, 0, 20, ASK),
        seller,
        &[seller],
    );
    (svm, admin, investors)
}

fn bid(svm: &mut LiteSVM, offeror: &Keypair, amount: u32, price: u64) {
    give_tgbp(svm, &offeror.pubkey(), 200_000_000_000);
    ok(
        svm,
        make_offer_ix(
            &offeror.pubkey(),
            0,
            amount,
            price,
            tgbp_mint(),
            tgbp_acc(&offeror.pubkey()),
        ),
        offeror,
        &[offeror],
    );
}

// --- making offers ---

#[test]
fn make_offer_holds_the_bid() {
    let (mut svm, admin, _investors) = listed_property();
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    // 10 shares at 5 GBP: 50 GBP left the bidder for the offer vault.
    assert_eq!(tgbp_balance(&svm, &offeror.pubkey()), 150_000_000_000);
    assert_eq!(
        token_balance(
            &svm,
            &payment_ata(&offer_vault_pda(0, &offeror.pubkey()), &tgbp_mint())
        ),
        50_000_000_000
    );
    let offer = offer_of(&svm, 0, &offeror.pubkey());
    assert_eq!(offer.amount, 10);
    assert_eq!(offer.share_price, BID);
    assert_eq!(offer.held, 50_000_000_000);
    assert_eq!(offer.nonce, 0);
    assert_eq!(share_listing_of(&svm, 0).next_offer_nonce, 1);

    // One open offer per bidder per listing.
    fails_with(
        &mut svm,
        make_offer_ix(
            &offeror.pubkey(),
            0,
            5,
            BID,
            tgbp_mint(),
            tgbp_acc(&offeror.pubkey()),
        ),
        &offeror,
        &[&offeror],
        "already in use",
    );
}

#[test]
fn make_offer_validates_the_bid() {
    let (mut svm, admin, _investors) = listed_property();
    let offeror = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &offeror.pubkey(), 200_000_000_000);
    give_xcav(&mut svm, &offeror.pubkey(), 1_000_000_000);

    let cases: [(u32, u64, Pubkey, Pubkey, &str); 4] = [
        (
            0,
            BID,
            tgbp_mint(),
            tgbp_acc(&offeror.pubkey()),
            "InvalidShareAmount",
        ),
        (
            10,
            999,
            tgbp_mint(),
            tgbp_acc(&offeror.pubkey()),
            "InvalidSharePrice",
        ),
        (
            21,
            BID,
            tgbp_mint(),
            tgbp_acc(&offeror.pubkey()),
            "NotEnoughSharesListed",
        ),
        (
            10,
            BID,
            xcav_mint(),
            token_acc(&offeror.pubkey()),
            "MintNotAccepted",
        ),
    ];
    for (amount, price, mint, account, expected) in cases {
        fails_with(
            &mut svm,
            make_offer_ix(&offeror.pubkey(), 0, amount, price, mint, account),
            &offeror,
            &[&offeror],
            expected,
        );
    }
}

// --- accepting ---

// Accepting settles both sides in one instruction, so it is the heaviest
// path in the program, and its PDA and ATA derivation costs vary with the
// account keys — unlucky ones ran the 200k default out. Clients therefore
// send an explicit budget (ACCEPT_OFFER_BUDGET, like every test here); the
// pin sits above the observed wobble but well under that budget, so a
// structural regression still fails loudly.
const COMPUTE_BUDGET: u64 = 250_000;

#[test]
fn accept_stays_within_the_compute_budget() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    let used = process_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
    )
    .unwrap()
    .compute_units_consumed;
    assert!(
        used <= COMPUTE_BUDGET,
        "accept_offer used {used} CU, over the {COMPUTE_BUDGET} pin"
    );
}

#[test]
fn accept_pays_from_the_vault_and_moves_shares() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    let treasury_before = token_balance(&svm, &treasury_payment_ata());
    ok_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
    );

    // 50 GBP held: 1% fee to the treasury, the rest to the seller. The
    // bidder pays nothing beyond what the vault already held.
    assert_eq!(
        token_balance(&svm, &payment_ata(&seller.pubkey(), &tgbp_mint())),
        49_500_000_000
    );
    assert_eq!(
        token_balance(&svm, &treasury_payment_ata()) - treasury_before,
        500_000_000
    );
    assert_eq!(tgbp_balance(&svm, &offeror.pubkey()), 150_000_000_000);

    // Shares and reservations move like a buy; offer and vault are gone.
    assert_eq!(holding_of(&svm, 0, &seller.pubkey()).amount, 23);
    assert_eq!(holding_of(&svm, 0, &seller.pubkey()).listed, 10);
    assert_eq!(holding_of(&svm, 0, &offeror.pubkey()).amount, 10);
    assert_eq!(
        token_balance(&svm, &investor_share_ata(0, &offeror.pubkey())),
        10
    );
    assert_eq!(share_listing_of(&svm, 0).amount, 10);
    assert_eq!(property_of(&svm, 0).holder_count, 5);
    assert!(svm.get_account(&offer_pda(0, &offeror.pubkey())).is_none());
    assert!(svm
        .get_account(&payment_ata(
            &offer_vault_pda(0, &offeror.pubkey()),
            &tgbp_mint()
        ))
        .map(|a| a.data.is_empty())
        .unwrap_or(true));
}

#[test]
fn accept_binds_to_the_offer_nonce() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    // The bidder swaps their offer for a stingier one right before the
    // seller's acceptance lands; the stale nonce refuses.
    ok(
        &mut svm,
        cancel_offer_ix(&offeror.pubkey(), 0, tgbp_mint()),
        &offeror,
        &[&offeror],
    );
    bid(&mut svm, &offeror, 10, 1_000_000_000);
    fails_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
        "OfferNonceMismatch",
    );
    ok_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 1, tgbp_mint()),
        seller,
        &[seller],
    );
}

#[test]
fn accept_requires_the_seller() {
    let (mut svm, admin, investors) = listed_property();
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);
    let outsider = &investors[2];
    fails_with_budget(
        &mut svm,
        accept_offer_ix(&outsider.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        outsider,
        &[outsider],
        "WrongSeller",
    );
}

#[test]
fn accept_fails_when_the_listing_shrank_below_the_offer() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 15, BID);

    // A direct buy takes most of the listing first.
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 200_000_000_000);
    ok(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 10, u64::MAX),
        &buyer,
        &[&buyer],
    );
    fails_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
        "NotEnoughSharesListed",
    );
    // The bid is not stranded: the bidder cancels out.
    ok(
        &mut svm,
        cancel_offer_ix(&offeror.pubkey(), 0, tgbp_mint()),
        &offeror,
        &[&offeror],
    );
    assert_eq!(tgbp_balance(&svm, &offeror.pubkey()), 125_000_000_000);
}

#[test]
fn accept_that_empties_the_listing_closes_it() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 20, BID);
    ok_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
    );
    assert!(svm.get_account(&share_listing_pda(0)).is_none());
    assert_eq!(holding_of(&svm, 0, &seller.pubkey()).listed, 0);
    assert_eq!(holding_of(&svm, 0, &offeror.pubkey()).amount, 20);
}

#[test]
fn accept_respects_the_ownership_cap() {
    let (mut svm, _admin, investors) = listed_property();
    let seller = &investors[1];
    // Investor 0 holds 34 and may only reach 49; a 16-share offer is over.
    let offeror = &investors[0];
    give_tgbp(&mut svm, &offeror.pubkey(), 200_000_000_000);
    ok(
        &mut svm,
        make_offer_ix(
            &offeror.pubkey(),
            0,
            16,
            BID,
            tgbp_mint(),
            tgbp_acc(&offeror.pubkey()),
        ),
        offeror,
        &[offeror],
    );
    fails_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
        "MaxOwnershipExceeded",
    );
}

#[test]
fn accept_requires_a_still_compliant_bidder() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    // KYC revoked between make and accept: shares must not be delivered.
    set_compliance(&mut svm, &admin, &offeror.pubkey(), false);
    fails_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
        "NotCompliant",
    );
    // The revoked bidder still gets their money back; exits are role-free.
    ok(
        &mut svm,
        cancel_offer_ix(&offeror.pubkey(), 0, tgbp_mint()),
        &offeror,
        &[&offeror],
    );
    assert_eq!(
        token_balance(&svm, &payment_ata(&offeror.pubkey(), &tgbp_mint())),
        50_000_000_000
    );
}

#[test]
fn seller_cannot_bid_on_own_listing() {
    let (mut svm, _admin, investors) = listed_property();
    let seller = &investors[1];
    give_tgbp(&mut svm, &seller.pubkey(), 200_000_000_000);
    fails_with(
        &mut svm,
        make_offer_ix(
            &seller.pubkey(),
            0,
            5,
            BID,
            tgbp_mint(),
            tgbp_acc(&seller.pubkey()),
        ),
        seller,
        &[seller],
        "SelfOffer",
    );
}

// --- reject / cancel ---

#[test]
fn reject_refunds_the_bidder() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    let outsider = &investors[2];
    fails_with(
        &mut svm,
        reject_offer_ix(&outsider.pubkey(), 0, &offeror.pubkey(), 0, tgbp_mint()),
        outsider,
        &[outsider],
        "WrongSeller",
    );
    ok(
        &mut svm,
        reject_offer_ix(&seller.pubkey(), 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
    );
    // The refund lands at the bidder's ATA; nothing was sold.
    assert_eq!(
        token_balance(&svm, &payment_ata(&offeror.pubkey(), &tgbp_mint())),
        50_000_000_000
    );
    assert!(svm.get_account(&offer_pda(0, &offeror.pubkey())).is_none());
    assert_eq!(share_listing_of(&svm, 0).amount, 20);
}

#[test]
fn cancel_survives_the_listing_dying() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    // The whole listing sells to someone else and closes.
    let buyer = new_investor(&mut svm, &admin);
    give_tgbp(&mut svm, &buyer.pubkey(), 200_000_000_000);
    ok(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), 0, 0, &seller.pubkey(), 20, u64::MAX),
        &buyer,
        &[&buyer],
    );
    assert!(svm.get_account(&share_listing_pda(0)).is_none());

    // The bid still comes back in full.
    ok(
        &mut svm,
        cancel_offer_ix(&offeror.pubkey(), 0, tgbp_mint()),
        &offeror,
        &[&offeror],
    );
    assert_eq!(tgbp_balance(&svm, &offeror.pubkey()), 150_000_000_000);
    assert_eq!(
        token_balance(&svm, &payment_ata(&offeror.pubkey(), &tgbp_mint())),
        50_000_000_000
    );
    assert!(svm.get_account(&offer_pda(0, &offeror.pubkey())).is_none());
}

#[test]
fn dusted_vault_cannot_wedge_the_offer() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    // Anyone can donate tokens into any ATA; a fixed-amount payout would
    // then leave a remainder and the close would refuse. The sweep pays
    // whatever is actually there.
    let vault_ata = payment_ata(&offer_vault_pda(0, &offeror.pubkey()), &tgbp_mint());
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        vault_ata,
        &offer_vault_pda(0, &offeror.pubkey()),
        50_000_000_001,
    );
    ok_with_budget(
        &mut svm,
        accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint()),
        seller,
        &[seller],
    );
    // Seller gets the bid minus the fee, plus the stray unit.
    assert_eq!(
        token_balance(&svm, &payment_ata(&seller.pubkey(), &tgbp_mint())),
        49_500_000_001
    );
    assert!(svm
        .get_account(&vault_ata)
        .map(|a| a.data.is_empty())
        .unwrap_or(true));
}

#[test]
fn dusted_vault_still_refunds_in_full() {
    let (mut svm, admin, _investors) = listed_property();
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);
    let vault_ata = payment_ata(&offer_vault_pda(0, &offeror.pubkey()), &tgbp_mint());
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        vault_ata,
        &offer_vault_pda(0, &offeror.pubkey()),
        50_000_000_001,
    );
    ok(
        &mut svm,
        cancel_offer_ix(&offeror.pubkey(), 0, tgbp_mint()),
        &offeror,
        &[&offeror],
    );
    assert_eq!(
        token_balance(&svm, &payment_ata(&offeror.pubkey(), &tgbp_mint())),
        50_000_000_001
    );
}

#[test]
fn cancel_requires_the_offeror() {
    let (mut svm, admin, investors) = listed_property();
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);
    // Another wallet's cancel derives a different offer PDA and finds
    // nothing there.
    fails_with(
        &mut svm,
        cancel_offer_ix(&investors[2].pubkey(), 0, tgbp_mint()),
        &investors[2],
        &[&investors[2]],
        "AccountNotInitialized",
    );
}

// --- direct transfers ---

#[test]
fn send_moves_shares_between_compliant_holders() {
    let (mut svm, admin, investors) = finalized_property();
    let sender = &investors[1];
    let receiver = new_investor(&mut svm, &admin);
    ok(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 12),
        sender,
        &[sender],
    );

    assert_eq!(holding_of(&svm, 0, &sender.pubkey()).amount, 21);
    assert_eq!(holding_of(&svm, 0, &receiver.pubkey()).amount, 12);
    assert_eq!(
        token_balance(&svm, &investor_share_ata(0, &receiver.pubkey())),
        12
    );
    assert_eq!(property_of(&svm, 0).holder_count, 5);
}

#[test]
fn send_rejects_listed_or_locked_shares() {
    let (mut svm, admin, investors) = listed_property();
    // Investor 1 has 33 shares with 20 listed; only 13 can move.
    let sender = &investors[1];
    let receiver = new_investor(&mut svm, &admin);
    fails_with(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 14),
        sender,
        &[sender],
        "NotEnoughShares",
    );
    ok(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 13),
        sender,
        &[sender],
    );
}

#[test]
fn send_to_self_is_blocked() {
    let (mut svm, _admin, investors) = finalized_property();
    let sender = &investors[1];
    fails_with(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &sender.pubkey(), 0, 5),
        sender,
        &[sender],
        "DuplicateMutableAccount",
    );
}

#[test]
fn send_needs_a_compliant_receiver() {
    let (mut svm, _admin, investors) = finalized_property();
    let sender = &investors[1];
    // A wallet with no investor role has no role account to show.
    let stranger = funded(&mut svm);
    fails_with(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &stranger.pubkey(), 0, 5),
        sender,
        &[sender],
        "AccountNotInitialized",
    );
}

#[test]
fn send_respects_the_ownership_cap() {
    let (mut svm, _admin, investors) = finalized_property();
    // Investor 0 holds 34; 16 more would break the strict-below-49 cap.
    let sender = &investors[1];
    let receiver = &investors[0];
    fails_with(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 16),
        sender,
        &[sender],
        "MaxOwnershipExceeded",
    );
    ok(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 15),
        sender,
        &[sender],
    );
    assert_eq!(holding_of(&svm, 0, &receiver.pubkey()).amount, 49);
}

#[test]
fn send_waits_for_the_settlement() {
    let (mut svm, admin, investors) = build_property(false);
    let sender = &investors[1];
    let receiver = new_investor(&mut svm, &admin);
    fails_with(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 5),
        sender,
        &[sender],
        "PropertyNotFinalized",
    );
}

#[test]
fn send_settles_income_for_both_sides() {
    let (mut svm, admin, investors) = finalized_property();
    svm.add_program(marketplace::PROPERTY_PROGRAM, &program_bytes("property"))
        .unwrap();
    seed_income_stream(&mut svm, 2_000_000_000);

    let sender = &investors[1];
    let receiver = new_investor(&mut svm, &admin);
    ok(
        &mut svm,
        send_shares_ix(&sender.pubkey(), &receiver.pubkey(), 0, 12),
        sender,
        &[sender],
    );

    // The sender banked all 33 shares' worth at the pre-transfer balance;
    // the receiver starts earning only from here.
    let sender_cp = checkpoint_of(&svm, &sender.pubkey());
    assert_eq!(sender_cp.entries[0].pending, 66_000_000_000);
    let receiver_cp = checkpoint_of(&svm, &receiver.pubkey());
    assert_eq!(receiver_cp.entries[0].pending, 0);
    assert_eq!(receiver_cp.entries[0].per_share, 2_000_000_000);
}

// The handler-checked registry accounts (the struct is frame-tight) must
// still pin the right wallet: a cleared bystander's record can't stand in
// for the offeror's.
#[test]
fn anothers_registry_accounts_are_rejected_at_accept() {
    let (mut svm, admin, investors) = listed_property();
    let seller = &investors[1];
    let offeror = new_investor(&mut svm, &admin);
    bid(&mut svm, &offeror, 10, BID);

    let mut ix = accept_offer_ix(&seller.pubkey(), 0, 0, &offeror.pubkey(), 0, tgbp_mint());
    for account in ix.accounts.iter_mut() {
        if account.pubkey == compliance_pda(&offeror.pubkey()) {
            account.pubkey = compliance_pda(&seller.pubkey());
        }
    }
    fails_with_budget(&mut svm, ix, seller, &[seller], "WrongRegistryAccount");
}
