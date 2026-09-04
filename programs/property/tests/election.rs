//! The letting agent election: candidacy rounds on a finalized property,
//! share-weighted votes locked in the marketplace ledger through the CPI,
//! plurality finalize, and the reopen paths for rounds that elect nobody.

mod common;
use common::*;

use anchor_lang::{InstructionData, ToAccountMetas};

const REGION: u16 = 1;
const ASSET: u64 = 7;

fn setup_prop() -> (LiteSVM, Keypair, Keypair) {
    let (mut svm, admin, authority) = setup();
    seed_location(&mut svm, REGION, POSTCODE);
    seed_location(&mut svm, REGION, POSTCODE_B);
    seed_property_asset(&mut svm, ASSET, REGION, POSTCODE);
    (svm, admin, authority)
}

/// A registered agent covering the property's location.
fn covering_agent(svm: &mut LiteSVM, admin: &Keypair) -> Keypair {
    let agent = new_agent(svm, admin);
    ok(
        svm,
        add_agent_ix(&agent.pubkey(), REGION, POSTCODE, u64::MAX),
        &agent,
        &[&agent],
    );
    agent
}

fn vote(svm: &mut LiteSVM, voter: &Keypair, round: u64, choice: &Pubkey, amount: u32) {
    let sponsor = sponsor();
    ok(
        svm,
        vote_agent_ix(&voter.pubkey(), ASSET, round, choice, None, amount),
        voter,
        &[voter, &sponsor],
    );
}

#[test]
fn claim_opens_the_window_and_creates_the_seat() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);

    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );

    let letting = letting_of(&svm, ASSET);
    assert_eq!(letting.asset_id, ASSET);
    assert_eq!(letting.agent, Pubkey::default());
    assert_eq!(letting.election.round, 1);
    assert_eq!(letting.election.candidate_count, 1);
    assert_eq!(letting.election.quorum_bps, QUORUM_BPS);
    assert!(letting.election.expiry > 0);
    let candidacy = candidacy_of(&svm, ASSET, 1, &agent.pubkey());
    assert_eq!(candidacy.agent, agent.pubkey());
    assert_eq!(candidacy.vote_power, 0);
}

#[test]
fn claim_requires_covering_the_location() {
    let (mut svm, admin, _authority) = setup_prop();
    // Registered agent, but for the other location.
    let agent = new_agent(&mut svm, &admin);
    ok(
        &mut svm,
        add_agent_ix(&agent.pubkey(), REGION, POSTCODE_B, u64::MAX),
        &agent,
        &[&agent],
    );
    fails_with(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
        "NotInLocation",
    );
}

#[test]
fn claim_requires_a_finalized_property() {
    let (mut svm, admin, _authority) = setup_prop();
    set_property_finalized(&mut svm, ASSET, false);
    let agent = covering_agent(&mut svm, &admin);
    fails_with(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
        "PropertyNotFinalized",
    );
}

#[test]
fn claim_pins_the_round_numbers() {
    let (mut svm, admin, _authority) = setup_prop();
    let first = covering_agent(&mut svm, &admin);
    let second = covering_agent(&mut svm, &admin);

    // Opening a fresh window needs the next round number.
    fails_with(
        &mut svm,
        claim_property_ix(&first.pubkey(), ASSET, 2),
        &first,
        &[&first],
        "WrongElectionRound",
    );
    ok(
        &mut svm,
        claim_property_ix(&first.pubkey(), ASSET, 1),
        &first,
        &[&first],
    );
    // Joining the running round needs its exact number.
    fails_with(
        &mut svm,
        claim_property_ix(&second.pubkey(), ASSET, 2),
        &second,
        &[&second],
        "WrongElectionRound",
    );
    ok(
        &mut svm,
        claim_property_ix(&second.pubkey(), ASSET, 1),
        &second,
        &[&second],
    );
    assert_eq!(letting_of(&svm, ASSET).election.candidate_count, 2);
}

#[test]
fn claim_stops_at_the_candidate_cap() {
    let (mut svm, admin, _authority) = setup_prop();
    for _ in 0..5 {
        let agent = covering_agent(&mut svm, &admin);
        ok(
            &mut svm,
            claim_property_ix(&agent.pubkey(), ASSET, 1),
            &agent,
            &[&agent],
        );
    }
    let sixth = covering_agent(&mut svm, &admin);
    fails_with(
        &mut svm,
        claim_property_ix(&sixth.pubkey(), ASSET, 1),
        &sixth,
        &[&sixth],
        "TooManyCandidates",
    );
}

#[test]
fn vote_locks_shares_in_the_marketplace_ledger() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let holder = new_holder(&mut svm, &admin, ASSET, 60);

    vote(&mut svm, &holder, 1, &agent.pubkey(), 40);

    assert_eq!(candidacy_of(&svm, ASSET, 1, &agent.pubkey()).vote_power, 40);
    let holding = holding_of(&svm, ASSET, &holder.pubkey());
    assert_eq!(holding.amount, 60);
    assert_eq!(holding.locked(), 40);
}

#[test]
fn vote_rejects_more_than_the_holding() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let holder = new_holder(&mut svm, &admin, ASSET, 60);
    let sponsor = sponsor();
    fails_with(
        &mut svm,
        vote_agent_ix(&holder.pubkey(), ASSET, 1, &agent.pubkey(), None, 61),
        &holder,
        &[&holder, &sponsor],
        "NotEnoughShares",
    );
}

#[test]
fn revote_moves_the_power() {
    let (mut svm, admin, _authority) = setup_prop();
    let first = covering_agent(&mut svm, &admin);
    let second = covering_agent(&mut svm, &admin);
    for agent in [&first, &second] {
        ok(
            &mut svm,
            claim_property_ix(&agent.pubkey(), ASSET, 1),
            agent,
            &[agent],
        );
    }
    let holder = new_holder(&mut svm, &admin, ASSET, 60);
    let sponsor = sponsor();

    vote(&mut svm, &holder, 1, &first.pubkey(), 40);
    // Moving to another candidate names the old one; the tallies and the
    // lock both follow.
    ok(
        &mut svm,
        vote_agent_ix(
            &holder.pubkey(),
            ASSET,
            1,
            &second.pubkey(),
            Some(&first.pubkey()),
            25,
        ),
        &holder,
        &[&holder, &sponsor],
    );
    assert_eq!(candidacy_of(&svm, ASSET, 1, &first.pubkey()).vote_power, 0);
    assert_eq!(
        candidacy_of(&svm, ASSET, 1, &second.pubkey()).vote_power,
        25
    );
    assert_eq!(holding_of(&svm, ASSET, &holder.pubkey()).locked(), 25);

    // Same candidate again: no previous account rides along.
    vote(&mut svm, &holder, 1, &second.pubkey(), 30);
    assert_eq!(
        candidacy_of(&svm, ASSET, 1, &second.pubkey()).vote_power,
        30
    );
    assert_eq!(holding_of(&svm, ASSET, &holder.pubkey()).locked(), 30);
}

#[test]
fn revote_pins_the_previous_candidacy_exactly() {
    let (mut svm, admin, _authority) = setup_prop();
    let first = covering_agent(&mut svm, &admin);
    let second = covering_agent(&mut svm, &admin);
    for agent in [&first, &second] {
        ok(
            &mut svm,
            claim_property_ix(&agent.pubkey(), ASSET, 1),
            agent,
            &[agent],
        );
    }
    let holder = new_holder(&mut svm, &admin, ASSET, 60);
    let sponsor = sponsor();

    // A fresh vote must not carry a previous candidacy.
    fails_with(
        &mut svm,
        vote_agent_ix(
            &holder.pubkey(),
            ASSET,
            1,
            &first.pubkey(),
            Some(&second.pubkey()),
            10,
        ),
        &holder,
        &[&holder, &sponsor],
        "CandidacyMismatch",
    );
    vote(&mut svm, &holder, 1, &first.pubkey(), 10);
    // Moving candidates without naming the old one must fail too.
    fails_with(
        &mut svm,
        vote_agent_ix(&holder.pubkey(), ASSET, 1, &second.pubkey(), None, 10),
        &holder,
        &[&holder, &sponsor],
        "CandidacyMismatch",
    );
}

#[test]
fn finalize_assigns_the_plurality_winner() {
    let (mut svm, admin, _authority) = setup_prop();
    let first = covering_agent(&mut svm, &admin);
    let second = covering_agent(&mut svm, &admin);
    for agent in [&first, &second] {
        ok(
            &mut svm,
            claim_property_ix(&agent.pubkey(), ASSET, 1),
            agent,
            &[agent],
        );
    }
    let a = new_holder(&mut svm, &admin, ASSET, 40);
    let b = new_holder(&mut svm, &admin, ASSET, 20);
    vote(&mut svm, &a, 1, &first.pubkey(), 40);
    vote(&mut svm, &b, 1, &second.pubkey(), 20);

    warp(&mut svm, VOTING_TIME + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_election_ix(
            &cranker.pubkey(),
            ASSET,
            1,
            Some(&first.pubkey()),
            &[first.pubkey(), second.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );

    let letting = letting_of(&svm, ASSET);
    assert_eq!(letting.agent, first.pubkey());
    assert_eq!(letting.election.expiry, 0);
    assert_eq!(letting.election.candidate_count, 0);
    assert_eq!(letting.election.round, 1);
    let entry = agent_of(&svm, &first.pubkey());
    assert_eq!(entry.locations[0].assigned_count, 1);
}

#[test]
fn finalize_waits_for_the_window() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_election_ix(&cranker.pubkey(), ASSET, 1, None, &[agent.pubkey()]),
        &cranker,
        &[&cranker],
        "VotingStillOngoing",
    );
}

// The expiry second itself already belongs to the finalizer.
#[test]
fn the_expiry_second_belongs_to_the_finalizer() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let holder = new_holder(&mut svm, &admin, ASSET, 60);
    vote(&mut svm, &holder, 1, &agent.pubkey(), 60);

    let expiry = letting_of(&svm, ASSET).election.expiry;
    warp_to(&mut svm, expiry);
    fails_with(
        &mut svm,
        vote_agent_ix(&holder.pubkey(), ASSET, 1, &agent.pubkey(), None, 10),
        &holder,
        &[&holder, &sponsor()],
        "VotingClosed",
    );
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_election_ix(
            &cranker.pubkey(),
            ASSET,
            1,
            Some(&agent.pubkey()),
            &[agent.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );
    assert_eq!(letting_of(&svm, ASSET).agent, agent.pubkey());
}

#[test]
fn finalize_needs_the_whole_candidate_field() {
    let (mut svm, admin, _authority) = setup_prop();
    let first = covering_agent(&mut svm, &admin);
    let second = covering_agent(&mut svm, &admin);
    for agent in [&first, &second] {
        ok(
            &mut svm,
            claim_property_ix(&agent.pubkey(), ASSET, 1),
            agent,
            &[agent],
        );
    }
    let a = new_holder(&mut svm, &admin, ASSET, 40);
    vote(&mut svm, &a, 1, &first.pubkey(), 40);

    warp(&mut svm, VOTING_TIME + 1);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_election_ix(
            &cranker.pubkey(),
            ASSET,
            1,
            Some(&first.pubkey()),
            &[first.pubkey()],
        ),
        &cranker,
        &[&cranker],
        "CandidacyMismatch",
    );
}

#[test]
fn finalize_reopens_on_quorum_miss() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    // 10 of 100 shares is under the 25% quorum.
    let holder = new_holder(&mut svm, &admin, ASSET, 10);
    vote(&mut svm, &holder, 1, &agent.pubkey(), 10);

    warp(&mut svm, VOTING_TIME + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_election_ix(
            &cranker.pubkey(),
            ASSET,
            1,
            Some(&agent.pubkey()),
            &[agent.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );

    let letting = letting_of(&svm, ASSET);
    assert_eq!(letting.agent, Pubkey::default());
    assert_eq!(letting.election.expiry, 0);
    // The next claim opens round two.
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 2),
        &agent,
        &[&agent],
    );
    assert_eq!(letting_of(&svm, ASSET).election.round, 2);
}

#[test]
fn finalize_reopens_on_a_tie() {
    let (mut svm, admin, _authority) = setup_prop();
    let first = covering_agent(&mut svm, &admin);
    let second = covering_agent(&mut svm, &admin);
    for agent in [&first, &second] {
        ok(
            &mut svm,
            claim_property_ix(&agent.pubkey(), ASSET, 1),
            agent,
            &[agent],
        );
    }
    let a = new_holder(&mut svm, &admin, ASSET, 30);
    let b = new_holder(&mut svm, &admin, ASSET, 30);
    vote(&mut svm, &a, 1, &first.pubkey(), 30);
    vote(&mut svm, &b, 1, &second.pubkey(), 30);

    warp(&mut svm, VOTING_TIME + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_election_ix(
            &cranker.pubkey(),
            ASSET,
            1,
            Some(&first.pubkey()),
            &[first.pubkey(), second.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );
    assert_eq!(letting_of(&svm, ASSET).agent, Pubkey::default());
}

#[test]
fn finalize_reopens_when_nobody_voted() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    warp(&mut svm, VOTING_TIME + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_election_ix(&cranker.pubkey(), ASSET, 1, None, &[agent.pubkey()]),
        &cranker,
        &[&cranker],
    );
    let letting = letting_of(&svm, ASSET);
    assert_eq!(letting.agent, Pubkey::default());
    assert_eq!(letting.election.expiry, 0);
}

#[test]
fn finalize_fails_the_round_when_the_winner_left() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let holder = new_holder(&mut svm, &admin, ASSET, 40);
    vote(&mut svm, &holder, 1, &agent.pubkey(), 40);
    // The would-be winner deregisters mid-election; nothing assigned them
    // yet, so the registry lets them go.
    ok(
        &mut svm,
        remove_agent_ix(&agent.pubkey(), POSTCODE),
        &agent,
        &[&agent],
    );

    warp(&mut svm, VOTING_TIME + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_election_ix(
            &cranker.pubkey(),
            ASSET,
            1,
            Some(&agent.pubkey()),
            &[agent.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );
    // The election failed gracefully instead of wedging.
    assert_eq!(letting_of(&svm, ASSET).agent, Pubkey::default());
}

#[test]
fn finalize_demands_the_winner_entry() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let holder = new_holder(&mut svm, &admin, ASSET, 40);
    vote(&mut svm, &holder, 1, &agent.pubkey(), 40);

    warp(&mut svm, VOTING_TIME + 1);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_election_ix(&cranker.pubkey(), ASSET, 1, None, &[agent.pubkey()]),
        &cranker,
        &[&cranker],
        "WrongAgent",
    );
}

#[test]
fn unlock_waits_for_the_round_to_settle() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let holder = new_holder(&mut svm, &admin, ASSET, 60);
    vote(&mut svm, &holder, 1, &agent.pubkey(), 40);

    fails_with(
        &mut svm,
        unlock_votes_ix(&holder.pubkey(), ASSET, 1),
        &holder,
        &[&holder],
        "VotingStillOngoing",
    );

    warp(&mut svm, VOTING_TIME + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_election_ix(
            &cranker.pubkey(),
            ASSET,
            1,
            Some(&agent.pubkey()),
            &[agent.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );
    ok(
        &mut svm,
        unlock_votes_ix(&holder.pubkey(), ASSET, 1),
        &holder,
        &[&holder],
    );
    assert_eq!(holding_of(&svm, ASSET, &holder.pubkey()).locked(), 0);
    assert!(account_gone(
        &svm,
        &agent_vote_pda(ASSET, 1, &holder.pubkey())
    ));
}

#[test]
fn close_candidacy_waits_for_the_round_to_settle() {
    let (mut svm, admin, _authority) = setup_prop();
    let agent = covering_agent(&mut svm, &admin);
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        close_candidacy_ix(
            &cranker.pubkey(),
            &agent.pubkey(),
            ASSET,
            1,
            &agent.pubkey(),
        ),
        &cranker,
        &[&cranker],
        "VotingStillOngoing",
    );

    warp(&mut svm, VOTING_TIME + 1);
    ok(
        &mut svm,
        finalize_election_ix(&cranker.pubkey(), ASSET, 1, None, &[agent.pubkey()]),
        &cranker,
        &[&cranker],
    );
    ok(
        &mut svm,
        close_candidacy_ix(
            &cranker.pubkey(),
            &agent.pubkey(),
            ASSET,
            1,
            &agent.pubkey(),
        ),
        &cranker,
        &[&cranker],
    );
    assert!(account_gone(
        &svm,
        &candidacy_pda(ASSET, 1, &agent.pubkey())
    ));
}

#[test]
fn lock_surface_rejects_wallet_callers() {
    // The marketplace lock instruction only accepts the property program's
    // signer PDA; a wallet passing it unsigned must bounce off the check.
    let (mut svm, admin, _authority) = setup_prop();
    let holder = new_holder(&mut svm, &admin, ASSET, 60);
    let attacker = funded(&mut svm);

    let mut accounts = marketplace::accounts::AdjustShareLock {
        property_signer: cpi_auth(),
        holding: holding_pda(ASSET, &holder.pubkey()),
    }
    .to_account_metas(None);
    // Nobody can sign for the PDA, so the attacker sends it unsigned.
    accounts[0].is_signer = false;
    let ix = anchor_lang::solana_program::instruction::Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::LockShares {
            asset_id: ASSET,
            owner: holder.pubkey(),
            reason: LockReason::AgentElection,
            amount: 60,
        }
        .data(),
        accounts,
    );
    fails_with(&mut svm, ix, &attacker, &[&attacker], "AccountNotSigner");
    assert_eq!(holding_of(&svm, ASSET, &holder.pubkey()).locked(), 0);
}
