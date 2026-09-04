//! Lawyer engagement on a sold-out sale: the developer allocating their own
//! lawyer, the SPV-side election (candidacies, share-locked votes, repeated
//! rounds), and the exits (resigning a case, unlocking voting shares,
//! reclaiming candidacy rent).

mod common;
use common::*;

use marketplace::state::ListingStatus;

/// Default costs a test lawyer bids: well inside the 5 GBP fee pot the sale
/// collects (100 shares x 5_000_000_000 quote units x 1% investor fee).
const COSTS: u64 = 1_000_000_000;
const FEE_POT: u64 = 5_000_000_000;

/// Full pipeline to a sold-out listing: three investors at 34/33/33 (the 50%
/// cap tops one investor out at 49).
fn setup_sold_out() -> (LiteSVM, Keypair, Keypair, (Keypair, Keypair, Keypair)) {
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
    (svm, admin, developer, (a, b, c))
}

/// Selling out already runs through the SPV attestation, so the SPV side is
/// always ready; the alias keeps the tests reading by what they need.
fn setup_with_spv() -> (LiteSVM, Keypair, Keypair, (Keypair, Keypair, Keypair)) {
    setup_sold_out()
}

// ========================== developer side ==========================

#[test]
fn assign_developer_lawyer_engages() {
    let (mut svm, admin, developer, _investors) = setup_sold_out();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
    );

    let listing = listing_of(&svm, 0);
    assert_eq!(listing.developer_lawyer.lawyer, lawyer.pubkey());
    // Their pay is a private matter; nothing is committed against the pot.
    assert_eq!(listing.developer_lawyer.costs, 0);
    assert_eq!(lawyer_of(&svm, &lawyer.pubkey()).active_cases, 1);
    // An engaged lawyer can't walk out of the registry with a live case.
    fails_with(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
        "LawyerStillActive",
    );
}

#[test]
fn assign_requires_a_sold_out_sale() {
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
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    fails_with(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
        "ListingNotActive",
    );
}

#[test]
fn assign_requires_the_property_region() {
    let (mut svm, admin, developer, _investors) = setup_sold_out();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 2, &operator.pubkey());
    let lawyer = new_registered_lawyer(&mut svm, &admin, 2);
    fails_with(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
        "WrongRegion",
    );
}

#[test]
fn assign_requires_a_compliant_lawyer() {
    let (mut svm, admin, developer, _investors) = setup_sold_out();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    set_compliance(&mut svm, &admin, &lawyer.pubkey(), false);
    fails_with(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
        "NotCompliant",
    );
}

#[test]
fn assign_after_the_deadline_fails() {
    let (mut svm, admin, developer, _investors) = setup_sold_out();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    warp(&mut svm, 100_001);
    fails_with(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
        "LegalProcessExpired",
    );
}

// The legal side checks `<= deadline` and the exits `> deadline`, so the
// deadline second itself still belongs to the lawyers.
#[test]
fn the_deadline_second_still_belongs_to_the_lawyers() {
    let (mut svm, admin, developer, (a, _b, _c)) = setup_sold_out();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);

    let deadline = listing_of(&svm, 0).legal_deadline;
    warp_to(&mut svm, deadline);
    fails_with(
        &mut svm,
        withdraw_legal_expired_ix(&a.pubkey(), 0),
        &a,
        &[&a],
        "LegalProcessNotExpired",
    );
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
    );
}

#[test]
fn assign_only_once() {
    let (mut svm, admin, developer, _investors) = setup_sold_out();
    let first = new_registered_lawyer(&mut svm, &admin, 1);
    let second = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &first.pubkey()),
        &developer,
        &[&developer],
    );
    fails_with(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &second.pubkey()),
        &developer,
        &[&developer],
        "LawyerJobTaken",
    );
}

// ========================== SPV-side election ==========================

#[test]
fn first_claim_opens_the_window() {
    let (mut svm, admin, _developer, _investors) = setup_with_spv();
    let first = new_registered_lawyer(&mut svm, &admin, 1);
    let second = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&first.pubkey(), 0, 1, COSTS),
        &first,
        &[&first, &sponsor()],
    );
    let election = listing_of(&svm, 0).spv_election;
    assert_eq!(election.round, 1);
    assert_eq!(election.candidate_count, 1);
    assert!(election.expiry > 0);

    // A second candidate joins the same round without restarting the clock.
    ok(
        &mut svm,
        claim_spv_ix(&second.pubkey(), 0, 1, COSTS),
        &second,
        &[&second, &sponsor()],
    );
    let joined = listing_of(&svm, 0).spv_election;
    assert_eq!(joined.candidate_count, 2);
    assert_eq!(joined.expiry, election.expiry);
    assert_eq!(candidacy_of(&svm, 0, 1, &second.pubkey()).vote_power, 0);
}

#[test]
fn claim_must_name_the_current_round() {
    let (mut svm, admin, _developer, _investors) = setup_with_spv();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    fails_with(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 2, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
        "WrongElectionRound",
    );
}

#[test]
fn election_elects_by_plurality() {
    let (mut svm, admin, _developer, (a, b, c)) = setup_with_spv();
    let spn = sponsor();
    let l1 = new_registered_lawyer(&mut svm, &admin, 1);
    let l2 = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&l1.pubkey(), 0, 1, COSTS),
        &l1,
        &[&l1, &sponsor()],
    );
    ok(
        &mut svm,
        claim_spv_ix(&l2.pubkey(), 0, 1, COSTS),
        &l2,
        &[&l2, &sponsor()],
    );
    for (voter, choice, amount) in [(&a, &l1, 34), (&b, &l2, 33), (&c, &l1, 10)] {
        ok(
            &mut svm,
            vote_spv_ix(&voter.pubkey(), 0, 1, &choice.pubkey(), None, amount),
            &spn,
            &[&spn, voter],
        );
    }
    assert_eq!(candidacy_of(&svm, 0, 1, &l1.pubkey()).vote_power, 44);
    assert_eq!(candidacy_of(&svm, 0, 1, &l2.pubkey()).vote_power, 33);

    warp(&mut svm, 10_001);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_spv_ix(
            &cranker.pubkey(),
            0,
            1,
            Some(&l1.pubkey()),
            &[l1.pubkey(), l2.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );

    let listing = listing_of(&svm, 0);
    assert_eq!(listing.spv_lawyer.lawyer, l1.pubkey());
    assert_eq!(listing.spv_lawyer.costs, COSTS);
    assert_eq!(listing.spv_election.expiry, 0);
    assert_eq!(lawyer_of(&svm, &l1.pubkey()).active_cases, 1);
    assert_eq!(lawyer_of(&svm, &l2.pubkey()).active_cases, 0);
}

#[test]
fn vote_locks_shares_and_revote_moves_power() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let l1 = new_registered_lawyer(&mut svm, &admin, 1);
    let l2 = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&l1.pubkey(), 0, 1, COSTS),
        &l1,
        &[&l1, &sponsor()],
    );
    ok(
        &mut svm,
        claim_spv_ix(&l2.pubkey(), 0, 1, COSTS),
        &l2,
        &[&l2, &sponsor()],
    );

    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &l1.pubkey(), None, 30),
        &spn,
        &[&spn, &a],
    );
    assert_eq!(holding_of(&svm, 0, &a.pubkey()).locked(), 30);
    assert_eq!(candidacy_of(&svm, 0, 1, &l1.pubkey()).vote_power, 30);

    // Revote to the other candidate: power moves, the lock follows the new
    // amount.
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &l2.pubkey(), Some(&l1.pubkey()), 10),
        &spn,
        &[&spn, &a],
    );
    assert_eq!(holding_of(&svm, 0, &a.pubkey()).locked(), 10);
    assert_eq!(candidacy_of(&svm, 0, 1, &l1.pubkey()).vote_power, 0);
    assert_eq!(candidacy_of(&svm, 0, 1, &l2.pubkey()).vote_power, 10);

    // And a same-target adjustment moves nothing between candidates.
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &l2.pubkey(), None, 15),
        &spn,
        &[&spn, &a],
    );
    assert_eq!(holding_of(&svm, 0, &a.pubkey()).locked(), 15);
    assert_eq!(candidacy_of(&svm, 0, 1, &l1.pubkey()).vote_power, 0);
    assert_eq!(candidacy_of(&svm, 0, 1, &l2.pubkey()).vote_power, 15);
}

#[test]
fn quorum_miss_reopens_the_election() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    // 20 of 100 shares is under the 25% quorum, however unanimous.
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 20),
        &spn,
        &[&spn, &a],
    );
    warp(&mut svm, 10_001);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_spv_ix(
            &cranker.pubkey(),
            0,
            1,
            Some(&lawyer.pubkey()),
            &[lawyer.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );

    let listing = listing_of(&svm, 0);
    assert_eq!(listing.spv_lawyer.lawyer, Pubkey::default());
    assert_eq!(listing.status, ListingStatus::SoldOut);
    assert_eq!(listing.spv_election.expiry, 0);
    // The next election runs as a fresh round.
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 2, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    assert_eq!(listing_of(&svm, 0).spv_election.round, 2);
}

#[test]
fn tied_candidates_reopen_the_election() {
    let (mut svm, admin, _developer, (a, b, _c)) = setup_with_spv();
    let spn = sponsor();
    let l1 = new_registered_lawyer(&mut svm, &admin, 1);
    let l2 = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&l1.pubkey(), 0, 1, COSTS),
        &l1,
        &[&l1, &sponsor()],
    );
    ok(
        &mut svm,
        claim_spv_ix(&l2.pubkey(), 0, 1, COSTS),
        &l2,
        &[&l2, &sponsor()],
    );
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &l1.pubkey(), None, 20),
        &spn,
        &[&spn, &a],
    );
    ok(
        &mut svm,
        vote_spv_ix(&b.pubkey(), 0, 1, &l2.pubkey(), None, 20),
        &spn,
        &[&spn, &b],
    );
    warp(&mut svm, 10_001);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_spv_ix(
            &cranker.pubkey(),
            0,
            1,
            Some(&l1.pubkey()),
            &[l1.pubkey(), l2.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );

    let listing = listing_of(&svm, 0);
    assert_eq!(listing.spv_lawyer.lawyer, Pubkey::default());
    assert_eq!(listing.status, ListingStatus::SoldOut);
}

#[test]
fn vote_requires_an_open_election() {
    let (mut svm, _admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    // With no election there is no candidacy to vote for, and the account
    // check refuses before the handler could.
    fails_with(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 0, &Pubkey::default(), None, 10),
        &spn,
        &[&spn, &a],
        "AccountNotInitialized",
    );
}

// The expiry second is already the finalizer's: too late to vote, not too
// early to count.
#[test]
fn the_expiry_second_belongs_to_the_count() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 34),
        &spn,
        &[&spn, &a],
    );

    let expiry = listing_of(&svm, 0).spv_election.expiry;
    warp_to(&mut svm, expiry);
    fails_with(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 10),
        &spn,
        &[&spn, &a],
        "VotingClosed",
    );
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_spv_ix(
            &cranker.pubkey(),
            0,
            1,
            Some(&lawyer.pubkey()),
            &[lawyer.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );
}

#[test]
fn vote_beyond_the_holding_fails() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    fails_with(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 35),
        &spn,
        &[&spn, &a],
        "NotEnoughShares",
    );
}

#[test]
fn finalize_needs_the_whole_field() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let l1 = new_registered_lawyer(&mut svm, &admin, 1);
    let l2 = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&l1.pubkey(), 0, 1, COSTS),
        &l1,
        &[&l1, &sponsor()],
    );
    ok(
        &mut svm,
        claim_spv_ix(&l2.pubkey(), 0, 1, COSTS),
        &l2,
        &[&l2, &sponsor()],
    );
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &l1.pubkey(), None, 30),
        &spn,
        &[&spn, &a],
    );
    warp(&mut svm, 10_001);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_spv_ix(&cranker.pubkey(), 0, 1, Some(&l1.pubkey()), &[l1.pubkey()]),
        &cranker,
        &[&cranker],
        "CandidacyMismatch",
    );
}

#[test]
fn finalize_waits_for_the_close() {
    let (mut svm, admin, _developer, _investors) = setup_with_spv();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_spv_ix(
            &cranker.pubkey(),
            0,
            1,
            Some(&lawyer.pubkey()),
            &[lawyer.pubkey()],
        ),
        &cranker,
        &[&cranker],
        "VotingStillOngoing",
    );
}

#[test]
fn vanished_winner_fails_gracefully() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 34),
        &spn,
        &[&spn, &a],
    );
    // The candidate leaves the registry mid-vote; the win must not wedge the
    // election, it just doesn't take effect.
    ok(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
    );
    warp(&mut svm, 10_001);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_spv_ix(
            &cranker.pubkey(),
            0,
            1,
            Some(&lawyer.pubkey()),
            &[lawyer.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );

    let listing = listing_of(&svm, 0);
    assert_eq!(listing.spv_lawyer.lawyer, Pubkey::default());
    assert_eq!(listing.spv_election.expiry, 0);
}

#[test]
fn candidacy_rent_comes_back_after_settlement() {
    let (mut svm, admin, _developer, _investors) = setup_with_spv();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        close_candidacy_ix(&cranker.pubkey(), 0, 1, &lawyer.pubkey()),
        &cranker,
        &[&cranker],
        "VotingStillOngoing",
    );

    warp(&mut svm, 10_001);
    ok(
        &mut svm,
        finalize_spv_ix(&cranker.pubkey(), 0, 1, None, &[lawyer.pubkey()]),
        &cranker,
        &[&cranker],
    );
    // The sponsor fronted the candidacy's rent, so the close pays it back
    // there, not to the lawyer.
    let before = svm.get_account(&sponsor().pubkey()).unwrap().lamports;
    ok(
        &mut svm,
        close_candidacy_ix(&cranker.pubkey(), 0, 1, &lawyer.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert!(svm.get_account(&sponsor().pubkey()).unwrap().lamports > before);
    assert!(svm
        .get_account(&candidacy_pda(0, 1, &lawyer.pubkey()))
        .is_none_or(|acc| acc.data.is_empty()));
}

// ===================== conflicts and the shared pot =====================

#[test]
fn one_lawyer_cannot_take_both_sides() {
    let (mut svm, admin, developer, _investors) = setup_with_spv();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
    );
    fails_with(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
        "ConflictOfInterest",
    );
}

#[test]
fn spv_costs_are_capped_by_the_fee_pot() {
    let (mut svm, admin, developer, _investors) = setup_with_spv();
    let first = new_registered_lawyer(&mut svm, &admin, 1);
    let second = new_registered_lawyer(&mut svm, &admin, 1);
    // The developer's side never touches the pot, so their engagement can't
    // squeeze the SPV lawyer's budget.
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &first.pubkey()),
        &developer,
        &[&developer],
    );
    fails_with(
        &mut svm,
        claim_spv_ix(&second.pubkey(), 0, 1, FEE_POT + 1),
        &second,
        &[&second, &sponsor()],
        "CostsExceedFees",
    );
    ok(
        &mut svm,
        claim_spv_ix(&second.pubkey(), 0, 1, FEE_POT),
        &second,
        &[&second, &sponsor()],
    );
}

// ========================== unlocking and exits ==========================

#[test]
fn unlock_waits_for_the_election() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 30),
        &spn,
        &[&spn, &a],
    );
    fails_with(
        &mut svm,
        unlock_votes_ix(&a.pubkey(), 0, 1),
        &a,
        &[&a],
        "VotingStillOngoing",
    );

    warp(&mut svm, 10_001);
    ok(&mut svm, unlock_votes_ix(&a.pubkey(), 0, 1), &a, &[&a]);
    assert_eq!(holding_of(&svm, 0, &a.pubkey()).locked(), 0);
    assert!(svm
        .get_account(&lawyer_vote_pda(0, 1, &a.pubkey()))
        .is_none_or(|acc| acc.data.is_empty()));
}

#[test]
fn voter_can_still_exit_after_legal_timeout() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 34),
        &spn,
        &[&spn, &a],
    );

    // Nobody finalizes; the legal window runs out with the vote still on the
    // books. Unlock, then the timeout exit, must both go through.
    warp(&mut svm, 100_001);
    let before = tgbp_balance(&svm, &a.pubkey());
    ok(&mut svm, unlock_votes_ix(&a.pubkey(), 0, 1), &a, &[&a]);
    ok(
        &mut svm,
        withdraw_legal_expired_ix(&a.pubkey(), 0),
        &a,
        &[&a],
    );
    assert!(tgbp_balance(&svm, &a.pubkey()) > before);
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Refunding);
}

#[test]
fn teardown_waits_for_the_finalizer() {
    let (mut svm, admin, developer, (a, b, c)) = setup_with_spv();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    // The legal window dies with the election never settled; everyone leaves.
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

    let cranker = funded(&mut svm);
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
        "VotingStillOngoing",
    );
    ok(
        &mut svm,
        finalize_spv_ix(&cranker.pubkey(), 0, 1, None, &[lawyer.pubkey()]),
        &cranker,
        &[&cranker],
    );
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
    // The candidacy stays reclaimable even though the listing is gone.
    ok(
        &mut svm,
        close_candidacy_ix(&cranker.pubkey(), 0, 1, &lawyer.pubkey()),
        &cranker,
        &[&cranker],
    );
}

#[test]
fn resign_reopens_the_side() {
    let (mut svm, admin, developer, _investors) = setup_sold_out();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        resign_case_ix(&lawyer.pubkey(), 0),
        &lawyer,
        &[&lawyer],
    );

    let listing = listing_of(&svm, 0);
    assert_eq!(listing.developer_lawyer.lawyer, Pubkey::default());
    assert_eq!(listing.developer_lawyer.costs, 0);
    assert_eq!(lawyer_of(&svm, &lawyer.pubkey()).active_cases, 0);
    // The side is open again for the next lawyer.
    let next = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &next.pubkey()),
        &developer,
        &[&developer],
    );
}

#[test]
fn teardown_waits_for_engaged_lawyers() {
    let (mut svm, admin, developer, (a, b, c)) = setup_sold_out();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &lawyer.pubkey()),
        &developer,
        &[&developer],
    );
    // The legal window dies; everyone drains the listing.
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

    // Closing now would strand the lawyer's registry deposit: their case
    // count only falls through resign, and resign needs the listing.
    let cranker = funded(&mut svm);
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
        "LawyerStillEngaged",
    );
    ok(
        &mut svm,
        resign_case_ix(&lawyer.pubkey(), 0),
        &lawyer,
        &[&lawyer],
    );
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
    // With the case gone, the lawyer can leave with their deposit.
    ok(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
    );
}

#[test]
fn revote_rejects_a_surplus_candidacy() {
    let (mut svm, admin, _developer, (a, _b, _c)) = setup_with_spv();
    let spn = sponsor();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 30),
        &spn,
        &[&spn, &a],
    );
    // A same-target revote must not carry the candidacy a second time; the
    // runtime hands back one shared buffer and the write-back would be
    // stale. Anchor's duplicate-mutable check refuses it outright.
    fails_with(
        &mut svm,
        vote_spv_ix(
            &a.pubkey(),
            0,
            1,
            &lawyer.pubkey(),
            Some(&lawyer.pubkey()),
            10,
        ),
        &spn,
        &[&spn, &a],
        "DuplicateMutableAccount",
    );
    // A different candidacy smuggled into the unused slot is refused too:
    // every account that rides along gets written back at exit, so nothing
    // unvalidated may ride.
    let other = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&other.pubkey(), 0, 1, COSTS),
        &other,
        &[&other, &sponsor()],
    );
    fails_with(
        &mut svm,
        vote_spv_ix(
            &a.pubkey(),
            0,
            1,
            &lawyer.pubkey(),
            Some(&other.pubkey()),
            10,
        ),
        &spn,
        &[&spn, &a],
        "CandidacyMismatch",
    );
    // The proper same-target revote adjusts the one tally.
    ok(
        &mut svm,
        vote_spv_ix(&a.pubkey(), 0, 1, &lawyer.pubkey(), None, 10),
        &spn,
        &[&spn, &a],
    );
    assert_eq!(candidacy_of(&svm, 0, 1, &lawyer.pubkey()).vote_power, 10);
    assert_eq!(holding_of(&svm, 0, &a.pubkey()).locked(), 10);
}

#[test]
fn voting_window_stops_at_the_deadline() {
    let (mut svm, admin, _developer, _investors) = setup_with_spv();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    // Standing this close to the legal deadline clamps the window to it; a
    // longer one could only ever produce an unassignable winner.
    warp(&mut svm, 95_000);
    ok(
        &mut svm,
        claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.spv_election.expiry, listing.legal_deadline);
}

#[test]
fn resign_needs_a_case() {
    let (mut svm, admin, _developer, _investors) = setup_sold_out();
    let lawyer = new_registered_lawyer(&mut svm, &admin, 1);
    fails_with(
        &mut svm,
        resign_case_ix(&lawyer.pubkey(), 0),
        &lawyer,
        &[&lawyer],
        "NotCaseLawyer",
    );
}

// The finalizer must bring the actual winner's registry for the engagement
// checks; the runner-up's can't stand in.
#[test]
fn finalize_needs_the_winners_registry() {
    let (mut svm, admin, _developer, (a, b, c)) = setup_with_spv();
    let spn = sponsor();
    let l1 = new_registered_lawyer(&mut svm, &admin, 1);
    let l2 = new_registered_lawyer(&mut svm, &admin, 1);
    for lawyer in [&l1, &l2] {
        ok(
            &mut svm,
            claim_spv_ix(&lawyer.pubkey(), 0, 1, COSTS),
            lawyer,
            &[lawyer, &sponsor()],
        );
    }
    for (voter, choice, amount) in [(&a, &l1, 34), (&b, &l2, 33), (&c, &l1, 10)] {
        ok(
            &mut svm,
            vote_spv_ix(&voter.pubkey(), 0, 1, &choice.pubkey(), None, amount),
            &spn,
            &[&spn, voter],
        );
    }
    warp(&mut svm, 10_001);

    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_spv_ix(
            &cranker.pubkey(),
            0,
            1,
            Some(&l2.pubkey()),
            &[l1.pubkey(), l2.pubkey()],
        ),
        &cranker,
        &[&cranker],
        "WrongLawyer",
    );
}
