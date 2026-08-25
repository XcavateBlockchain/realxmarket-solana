//! Document confirmation on a sold-out sale: both lawyers ruling on the
//! papers, the one-time second attempt after a split verdict, the cancelled
//! sale's refunds, and the crank that frees lawyers from a dead case.

mod common;
use common::*;

use marketplace::state::{DocumentStatus, ListingStatus};

const COSTS: u64 = 1_000_000_000;
/// The document set everyone rules on, and the revised set after a split.
const DOCS: [u8; 32] = [7u8; 32];
const DOCS2: [u8; 32] = [8u8; 32];

/// A cancelled sale refunds price + 3% region tax; the 1% investor fee
/// stays behind to pay the review and the treasury.
fn refund_of(amount: u32) -> u64 {
    let price = SHARE_PRICE * amount as u64;
    price + price * 300 / 10_000
}

fn fee_of(amount: u32) -> u64 {
    SHARE_PRICE * amount as u64 / 100
}

/// A sold-out listing with both sides engaged: the developer allocates their
/// lawyer directly, the SPV side elects one unopposed on investor a's vote.
/// The 10_001 warp that ends the voting window leaves most of the legal
/// process to run.
#[allow(clippy::type_complexity)]
fn setup_engaged() -> (
    LiteSVM,
    Keypair,
    Keypair,
    (Keypair, Keypair, Keypair),
    Keypair,
    Keypair,
) {
    setup_sides(true)
}

/// Same, minus the developer's side when `assign_dev` is false: the sale
/// waits on a lawyer the developer never appoints.
#[allow(clippy::type_complexity)]
fn setup_sides(
    assign_dev: bool,
) -> (
    LiteSVM,
    Keypair,
    Keypair,
    (Keypair, Keypair, Keypair),
    Keypair,
    Keypair,
) {
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
    let a = new_investor(&mut svm, &admin);
    let b = new_investor(&mut svm, &admin);
    let c = new_investor(&mut svm, &admin);
    acquire_many(&mut svm, &admin, &[(&a, 34), (&b, 33), (&c, 33)]);

    let dev_lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    if assign_dev {
        ok(
            &mut svm,
            assign_dev_lawyer_ix(&developer.pubkey(), 0, &dev_lawyer.pubkey()),
            &developer,
            &[&developer],
        );
    }
    let spv_lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&spv_lawyer.pubkey(), 0, 1, COSTS),
        &spv_lawyer,
        &[&spv_lawyer, &sponsor()],
    );
    let spn = sponsor();
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &spv_lawyer.pubkey(), None, 34),
        &spn,
        &[&spn, &a],
    );
    warp(&mut svm, 10_001);
    ok(
        &mut svm,
        finalize_spv_ix(
            &operator.pubkey(),
            0,
            1,
            Some(&spv_lawyer.pubkey()),
            &[spv_lawyer.pubkey()],
        ),
        &operator,
        &[&operator],
    );
    (svm, admin, developer, (a, b, c), dev_lawyer, spv_lawyer)
}

/// Both lawyers hand in the given verdicts on `docs`, developer side first.
fn rule_on(
    svm: &mut LiteSVM,
    dev_lawyer: &Keypair,
    spv_lawyer: &Keypair,
    dev: bool,
    spv: bool,
    docs: [u8; 32],
) {
    ok(
        svm,
        confirm_docs_ix(&dev_lawyer.pubkey(), 0, dev, docs),
        dev_lawyer,
        &[dev_lawyer],
    );
    ok(
        svm,
        confirm_docs_ix(&spv_lawyer.pubkey(), 0, spv, docs),
        spv_lawyer,
        &[spv_lawyer],
    );
}

fn rule(svm: &mut LiteSVM, dev_lawyer: &Keypair, spv_lawyer: &Keypair, dev: bool, spv: bool) {
    rule_on(svm, dev_lawyer, spv_lawyer, dev, spv, DOCS);
}

// ========================== the verdicts ==========================

#[test]
fn both_approvals_move_the_sale_to_settlement() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, spv_lawyer) = setup_engaged();
    ok(
        &mut svm,
        confirm_docs_ix(&dev_lawyer.pubkey(), 0, true, DOCS),
        &dev_lawyer,
        &[&dev_lawyer],
    );
    // One approval alone decides nothing.
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.status, ListingStatus::SoldOut);
    assert_eq!(
        listing.developer_lawyer.doc_status,
        DocumentStatus::Approved
    );

    ok(
        &mut svm,
        confirm_docs_ix(&spv_lawyer.pubkey(), 0, true, DOCS),
        &spv_lawyer,
        &[&spv_lawyer],
    );
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Legal);
}

#[test]
fn both_rejections_cancel_the_sale() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, spv_lawyer) = setup_engaged();
    rule(&mut svm, &dev_lawyer, &spv_lawyer, false, false);
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Cancelled);
}

#[test]
fn split_verdict_reruns_the_review_once() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, spv_lawyer) = setup_engaged();
    rule(&mut svm, &dev_lawyer, &spv_lawyer, true, false);

    // The documents went back for revision: both sides rule again on the
    // new set, the earlier approval included.
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.status, ListingStatus::SoldOut);
    assert!(listing.second_attempt);
    assert_eq!(listing.developer_lawyer.doc_status, DocumentStatus::Pending);
    assert_eq!(listing.spv_lawyer.doc_status, DocumentStatus::Pending);
    // The recorded hashes go with the verdicts: a new set is expected.
    assert_eq!(listing.developer_lawyer.documents_hash, [0u8; 32]);
    assert_eq!(listing.spv_lawyer.documents_hash, [0u8; 32]);

    rule_on(&mut svm, &dev_lawyer, &spv_lawyer, true, true, DOCS2);
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Legal);
}

#[test]
fn second_split_kills_the_sale() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, spv_lawyer) = setup_engaged();
    rule(&mut svm, &dev_lawyer, &spv_lawyer, false, true);
    rule(&mut svm, &dev_lawyer, &spv_lawyer, true, false);
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Cancelled);
}

// ========================== the gates ==========================

#[test]
fn only_the_case_lawyers_have_a_say() {
    let (mut svm, admin, _developer, _investors, _dev_lawyer, _spv_lawyer) = setup_engaged();
    let outsider = new_registered_lawyer(&mut svm, &admin, 1);
    fails_with(
        &mut svm,
        confirm_docs_ix(&outsider.pubkey(), 0, true, DOCS),
        &outsider,
        &[&outsider],
        "NotCaseLawyer",
    );
}

#[test]
fn confirming_twice_fails() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, _spv_lawyer) = setup_engaged();
    ok(
        &mut svm,
        confirm_docs_ix(&dev_lawyer.pubkey(), 0, true, DOCS),
        &dev_lawyer,
        &[&dev_lawyer],
    );
    fails_with(
        &mut svm,
        confirm_docs_ix(&dev_lawyer.pubkey(), 0, false, DOCS),
        &dev_lawyer,
        &[&dev_lawyer],
        "AlreadyConfirmed",
    );
}

#[test]
fn late_verdicts_lose_to_the_timeout() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, _spv_lawyer) = setup_engaged();
    warp(&mut svm, 100_001);
    fails_with(
        &mut svm,
        confirm_docs_ix(&dev_lawyer.pubkey(), 0, true, DOCS),
        &dev_lawyer,
        &[&dev_lawyer],
        "LegalProcessExpired",
    );
}

// ========================== the cancelled sale ==========================

#[test]
fn cancelled_sale_refunds_and_pays_the_review() {
    let (mut svm, _admin, developer, (a, b, c), dev_lawyer, spv_lawyer) = setup_engaged();
    rule(&mut svm, &dev_lawyer, &spv_lawyer, false, false);
    let cranker = funded(&mut svm);

    // Freeing the lawyers first must not lose them their pay: the
    // cancellation stamped what the SPV side is owed.
    for lawyer in [&dev_lawyer, &spv_lawyer] {
        ok(
            &mut svm,
            close_case_ix(&cranker.pubkey(), 0, &lawyer.pubkey()),
            &cranker,
            &[&cranker],
        );
    }
    // And the fees can't move while investor refunds are still inside.
    give_tgbp(&mut svm, &spv_lawyer.pubkey(), 0);
    fails_with(
        &mut svm,
        settle_cancelled_fees_ix(&cranker.pubkey(), 0, &spv_lawyer.pubkey()),
        &cranker,
        &[&cranker],
        "SharesOutstanding",
    );

    // a's voting shares unlock now that the sale is dead.
    ok(&mut svm, unlock_votes_ix(&a.pubkey(), 0, 1), &a, &[&a]);
    let before = tgbp_balance(&svm, &a.pubkey());
    ok(&mut svm, withdraw_cancelled_ix(&a.pubkey(), 0), &a, &[&a]);
    assert_eq!(tgbp_balance(&svm, &a.pubkey()) - before, refund_of(34));

    ok(&mut svm, withdraw_cancelled_ix(&b.pubkey(), 0), &b, &[&b]);
    ok(&mut svm, withdraw_cancelled_ix(&c.pubkey(), 0), &c, &[&c]);
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.status, ListingStatus::Cancelled);
    assert_eq!(listing.sold_share_amount, 0);
    assert_eq!(property_of(&svm, 0).holder_count, 0);

    // With every share back, the developer's deposit follows.
    ok(
        &mut svm,
        withdraw_deposit_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    // Teardown must wait for the fee settlement: the money still in the
    // vault includes the lawyer's pay. This sale only ever collected tGBP,
    // so that's the one triple the sweep takes.
    fails_with(
        &mut svm,
        close_dead_listing_ix_for(
            &cranker.pubkey(),
            0,
            &developer.pubkey(),
            true,
            &[tgbp_mint()],
        ),
        &cranker,
        &[&cranker],
        "CostsStillDue",
    );

    // The retained fees split: the SPV lawyer collects their costs, the
    // treasury takes the rest.
    ok(
        &mut svm,
        settle_cancelled_fees_ix(&cranker.pubkey(), 0, &spv_lawyer.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(
        token_balance(&svm, &payment_ata(&spv_lawyer.pubkey(), &tgbp_mint())),
        COSTS
    );
    let treasury_acc = svm.get_account(&treasury_payment_ata()).unwrap();
    let treasury_state: anchor_spl::token::spl_token::state::Account =
        anchor_lang::solana_program::program_pack::Pack::unpack(&treasury_acc.data).unwrap();
    assert_eq!(treasury_state.amount, fee_of(100) - COSTS);
    assert_eq!(listing_of(&svm, 0).spv_costs_due, 0);

    // Re-cranking the drained pot is a harmless no-op, so a wind-down
    // script can hit every mint blindly.
    let lawyer_before = token_balance(&svm, &payment_ata(&spv_lawyer.pubkey(), &tgbp_mint()));
    ok(
        &mut svm,
        settle_cancelled_fees_ix(&cranker.pubkey(), 0, &spv_lawyer.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(
        token_balance(&svm, &payment_ata(&spv_lawyer.pubkey(), &tgbp_mint())),
        lawyer_before
    );

    // And with the pot empty, the teardown goes through.
    ok(
        &mut svm,
        close_dead_listing_ix_for(
            &cranker.pubkey(),
            0,
            &developer.pubkey(),
            true,
            &[tgbp_mint()],
        ),
        &cranker,
        &[&cranker],
    );
    assert!(svm
        .get_account(&listing_pda(0))
        .is_none_or(|acc| acc.data.is_empty()));
}

// A mint outside the accepted list must not draw down the lawyer's debt:
// paying it in worthless units would divert the real fees to the treasury.
#[test]
fn settle_rejects_a_stray_mint() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, spv_lawyer) = setup_engaged();
    rule(&mut svm, &dev_lawyer, &spv_lawyer, false, false);
    let cranker = funded(&mut svm);
    give_tgbp(&mut svm, &spv_lawyer.pubkey(), 0);
    let mut ix = settle_cancelled_fees_ix(&cranker.pubkey(), 0, &spv_lawyer.pubkey());
    // XCAV is a real mint the config never accepts as payment.
    ix.accounts[3].pubkey = xcav_mint();
    fails_with(&mut svm, ix, &cranker, &[&cranker], "InvalidMint");
}

// ========================== the silent side ==========================

#[test]
fn silence_defaults_to_rejection() {
    let (mut svm, _admin, _developer, _investors, _dev_lawyer, spv_lawyer) = setup_engaged();
    ok(
        &mut svm,
        confirm_docs_ix(&spv_lawyer.pubkey(), 0, false, DOCS),
        &spv_lawyer,
        &[&spv_lawyer],
    );
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        resolve_silent_ix(&cranker.pubkey(), 0),
        &cranker,
        &[&cranker],
        "VerdictWindowOpen",
    );

    // Into the final fifth of the legal window the developer's side is
    // still quiet; its verdict defaults to a rejection.
    warp(&mut svm, 70_000);
    ok(
        &mut svm,
        resolve_silent_ix(&cranker.pubkey(), 0),
        &cranker,
        &[&cranker],
    );
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.status, ListingStatus::Cancelled);
    assert_eq!(
        listing.developer_lawyer.doc_status,
        DocumentStatus::Rejected
    );
    // The lawyer who did the review keeps their claim on the fees.
    assert_eq!(listing.spv_costs_due, COSTS);
    assert_eq!(listing.spv_costs_payee, spv_lawyer.pubkey());

    // The silence crank never hands out a second attempt.
    assert!(!listing.second_attempt);
}

// The crank only serves the SPV lawyer's claim; with the SPV side silent
// nobody's pay is at stake, and cancelling would retain investor fees the
// timeout exit refunds. So that direction must ride to the deadline.
#[test]
fn a_silent_spv_side_cannot_be_cranked() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, _spv_lawyer) = setup_engaged();
    ok(
        &mut svm,
        confirm_docs_ix(&dev_lawyer.pubkey(), 0, true, DOCS),
        &dev_lawyer,
        &[&dev_lawyer],
    );
    warp(&mut svm, 70_000);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        resolve_silent_ix(&cranker.pubkey(), 0),
        &cranker,
        &[&cranker],
        "NoVerdictPassed",
    );
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::SoldOut);
}

#[test]
fn silence_crank_needs_a_standing_verdict() {
    let (mut svm, _admin, _developer, _investors, _dev_lawyer, _spv_lawyer) = setup_engaged();
    warp(&mut svm, 70_000);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        resolve_silent_ix(&cranker.pubkey(), 0),
        &cranker,
        &[&cranker],
        "NoVerdictPassed",
    );
}

#[test]
fn verdicts_must_name_the_same_documents() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, spv_lawyer) = setup_engaged();
    fails_with(
        &mut svm,
        confirm_docs_ix(&dev_lawyer.pubkey(), 0, true, [0u8; 32]),
        &dev_lawyer,
        &[&dev_lawyer],
        "EmptyDocumentsHash",
    );
    ok(
        &mut svm,
        confirm_docs_ix(&dev_lawyer.pubkey(), 0, true, DOCS),
        &dev_lawyer,
        &[&dev_lawyer],
    );
    // The second verdict must rule on the set the first one recorded.
    fails_with(
        &mut svm,
        confirm_docs_ix(&spv_lawyer.pubkey(), 0, true, DOCS2),
        &spv_lawyer,
        &[&spv_lawyer],
        "DocumentsMismatch",
    );
    ok(
        &mut svm,
        confirm_docs_ix(&spv_lawyer.pubkey(), 0, true, DOCS),
        &spv_lawyer,
        &[&spv_lawyer],
    );
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.status, ListingStatus::Legal);
    assert_eq!(listing.developer_lawyer.documents_hash, DOCS);
    assert_eq!(listing.spv_lawyer.documents_hash, DOCS);
}

#[test]
fn withdraw_cancelled_requires_cancellation() {
    let (mut svm, _admin, _developer, (_a, b, _c), _dev_lawyer, _spv_lawyer) = setup_engaged();
    fails_with(
        &mut svm,
        withdraw_cancelled_ix(&b.pubkey(), 0),
        &b,
        &[&b],
        "ListingNotActive",
    );
}

// ========================== freeing the lawyers ==========================

#[test]
fn close_case_frees_the_lawyers() {
    let (mut svm, _admin, _developer, _investors, dev_lawyer, spv_lawyer) = setup_engaged();
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        close_case_ix(&cranker.pubkey(), 0, &dev_lawyer.pubkey()),
        &cranker,
        &[&cranker],
        "CaseStillOpen",
    );

    rule(&mut svm, &dev_lawyer, &spv_lawyer, false, false);
    for lawyer in [&dev_lawyer, &spv_lawyer] {
        ok(
            &mut svm,
            close_case_ix(&cranker.pubkey(), 0, &lawyer.pubkey()),
            &cranker,
            &[&cranker],
        );
        assert_eq!(lawyer_of(&svm, &lawyer.pubkey()).active_cases, 0);
    }
    // Freed lawyers can leave the registry; a freed side can't be freed again.
    ok(
        &mut svm,
        unregister_lawyer_ix(&dev_lawyer.pubkey()),
        &dev_lawyer,
        &[&dev_lawyer],
    );
    fails_with(
        &mut svm,
        close_case_ix(&cranker.pubkey(), 0, &spv_lawyer.pubkey()),
        &cranker,
        &[&cranker],
        "NotCaseLawyer",
    );
}

#[test]
fn approved_documents_still_drain_on_timeout() {
    let (mut svm, _admin, _developer, (a, b, c), dev_lawyer, spv_lawyer) = setup_engaged();
    rule(&mut svm, &dev_lawyer, &spv_lawyer, true, true);
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Legal);

    // Settlement never came; the deadline still opens the refunds.
    warp(&mut svm, 100_001);
    ok(&mut svm, unlock_votes_ix(&a.pubkey(), 0, 1), &a, &[&a]);
    for investor in [&a, &b, &c] {
        ok(
            &mut svm,
            withdraw_legal_expired_ix(&investor.pubkey(), 0),
            investor,
            &[investor],
        );
    }
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Refunding);
    assert_eq!(listing_of(&svm, 0).sold_share_amount, 0);

    // And the crank frees the lawyers from the refunding sale too.
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        close_case_ix(&cranker.pubkey(), 0, &spv_lawyer.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(lawyer_of(&svm, &spv_lawyer.pubkey()).active_cases, 0);
}

// Dying through the silence crank instead of the deadline must not dodge
// the abandonment slash: the developer still never appointed anyone.
#[test]
fn silent_abandonment_still_slashes_the_bond() {
    let (mut svm, _admin, developer, (a, b, c), _dev_lawyer, spv_lawyer) = setup_sides(false);
    ok(
        &mut svm,
        confirm_docs_ix(&spv_lawyer.pubkey(), 0, false, DOCS),
        &spv_lawyer,
        &[&spv_lawyer],
    );
    warp(&mut svm, 70_000);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        resolve_silent_ix(&cranker.pubkey(), 0),
        &cranker,
        &[&cranker],
    );
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Cancelled);

    ok(&mut svm, unlock_votes_ix(&a.pubkey(), 0, 1), &a, &[&a]);
    for investor in [&a, &b, &c] {
        ok(
            &mut svm,
            withdraw_cancelled_ix(&investor.pubkey(), 0),
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
    assert_eq!(
        xcav_balance(&svm, &developer.pubkey()) - before,
        LISTING_DEPOSIT - LISTING_DEPOSIT / 100
    );
}
