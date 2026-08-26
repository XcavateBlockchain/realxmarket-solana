//! Property governance: agent spending proposals (auto-approval tier,
//! share-weighted votes, quorum and high threshold) whose approval event
//! authorizes the SPV's off-chain payment, and challenges that slash and
//! strike the sitting agent.

mod common;
use common::*;

use anchor_lang::Discriminator;
use property::instructions::governance::ProposalExecuted;

const ASSET: u64 = 7;
const REGION: u16 = 1;
const HASH: [u8; 32] = [1u8; 32];
// Between the low and high tiers, so it goes to a plain vote.
const MID_AMOUNT: u64 = 500_000_000_000;

/// Whether the transaction emitted the event with this discriminator. Event
/// logs are base64; the first ten characters encode exactly the eight
/// discriminator bytes.
fn emitted(logs: &[String], discriminator: &[u8]) -> bool {
    const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u128;
    for &b in discriminator {
        bits = (bits << 8) | b as u128;
    }
    bits <<= 8;
    let prefix: String = (0..10)
        .map(|i| B64[((bits >> (66 - 6 * i)) & 63) as usize] as char)
        .collect();
    logs.iter().any(|l| {
        l.strip_prefix("Program data: ")
            .is_some_and(|d| d.starts_with(&prefix))
    })
}

fn executed(logs: &[String]) -> bool {
    emitted(logs, ProposalExecuted::DISCRIMINATOR)
}

/// Base world: a finalized property with an assigned, registered letting
/// agent.
fn gov_setup() -> (LiteSVM, Keypair, Keypair) {
    let (mut svm, admin, _authority) = setup();
    // LiteSVM's clock starts at zero, which the program reads as "never";
    // start at a real time so the cooldown sentinel behaves like mainnet.
    warp(&mut svm, 1_755_000_000);
    seed_location(&mut svm, REGION, POSTCODE);
    seed_property_asset(&mut svm, ASSET, REGION, POSTCODE);
    let agent = new_agent(&mut svm, &admin);
    ok(
        &mut svm,
        add_agent_ix(&agent.pubkey(), REGION, POSTCODE, AGENT_DEPOSIT),
        &agent,
        &[&agent],
    );
    seed_letting(&mut svm, ASSET, &agent.pubkey());
    seed_assignment(&mut svm, &agent.pubkey(), POSTCODE);
    (svm, admin, agent)
}

fn propose(svm: &mut LiteSVM, agent: &Keypair, id: u64, amount: u64) -> Vec<String> {
    match process(
        svm,
        propose_ix(&agent.pubkey(), ASSET, id, amount, HASH),
        agent,
        &[agent],
    ) {
        Ok(meta) => meta.logs,
        Err(failed) => panic!("expected success, failed with: {:?}", failed.err),
    }
}

fn vote(svm: &mut LiteSVM, voter: &Keypair, id: u64, choice: VoteChoice, amount: u32) {
    ok(
        svm,
        vote_proposal_ix(&voter.pubkey(), ASSET, id, choice, amount),
        voter,
        &[voter, &sponsor()],
    );
}

fn finalize(svm: &mut LiteSVM, agent: &Keypair, id: u64) -> Vec<String> {
    let cranker = funded(svm);
    match process(
        svm,
        finalize_proposal_ix(&cranker.pubkey(), &agent.pubkey(), ASSET, id),
        &cranker,
        &[&cranker],
    ) {
        Ok(meta) => meta.logs,
        Err(failed) => panic!("expected success, failed with: {:?}", failed.err),
    }
}

fn challenge(svm: &mut LiteSVM, challenger: &Keypair, id: u64) {
    ok(
        svm,
        challenge_ix(&challenger.pubkey(), ASSET, id, CHALLENGE_DEPOSIT),
        challenger,
        &[challenger],
    );
}

fn vote_challenge(svm: &mut LiteSVM, voter: &Keypair, id: u64, choice: VoteChoice, amount: u32) {
    ok(
        svm,
        vote_challenge_ix(&voter.pubkey(), ASSET, id, choice, amount),
        voter,
        &[voter, &sponsor()],
    );
}

fn finalize_challenge(svm: &mut LiteSVM, challenger: &Keypair, id: u64, agent: Option<&Pubkey>) {
    let cranker = funded(svm);
    ok(
        svm,
        finalize_challenge_ix(
            &cranker.pubkey(),
            &challenger.pubkey(),
            ASSET,
            id,
            agent,
            &challenger.pubkey(),
        ),
        &cranker,
        &[&cranker],
    );
}

// --- proposals ---

#[test]
fn high_proposal_opens_vote() {
    let (mut svm, _admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, HIGH_PROPOSAL);

    let proposal = proposal_of(&svm, ASSET, 1);
    assert_eq!(proposal.proposer, agent.pubkey());
    assert_eq!(proposal.amount, HIGH_PROPOSAL);
    assert!(proposal.expiry > 0);
    assert_eq!(proposal.quorum_bps, QUORUM_BPS);
    assert_eq!(proposal.threshold_bps, HIGH_THRESHOLD_BPS);

    let gov = letting_of(&svm, ASSET).governance;
    assert_eq!(gov.proposal_count, 1);
    assert_eq!(gov.active_proposal, 1);
}

#[test]
fn mid_proposal_carries_no_threshold() {
    let (mut svm, _admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    assert_eq!(proposal_of(&svm, ASSET, 1).threshold_bps, 0);
}

#[test]
fn low_proposal_executes_immediately() {
    let (mut svm, _admin, agent) = gov_setup();
    let logs = propose(&mut svm, &agent, 1, LOW_PROPOSAL);

    // The authorization is the event; nothing stays behind on chain.
    assert!(executed(&logs));
    assert!(account_gone(&svm, &proposal_pda(ASSET, 1)));
    let gov = letting_of(&svm, ASSET).governance;
    assert_eq!(gov.proposal_count, 0);
    assert_eq!(gov.active_proposal, 0);
    assert!(gov.last_auto_approval_ts > 0);
}

#[test]
fn auto_approval_respects_cooldown() {
    let (mut svm, _admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, LOW_PROPOSAL);
    // Auto-approvals never consume an id, so the next request is 1 again.
    fails_with(
        &mut svm,
        propose_ix(&agent.pubkey(), ASSET, 1, LOW_PROPOSAL, HASH),
        &agent,
        &[&agent],
        "AutoApprovalTooSoon",
    );
    warp(&mut svm, AUTO_COOLDOWN + 1);
    propose(&mut svm, &agent, 1, LOW_PROPOSAL);
    // The cooldown only gates the auto tier; a vote can open right away.
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    assert_eq!(letting_of(&svm, ASSET).governance.active_proposal, 1);
}

#[test]
fn propose_requires_assigned_agent() {
    let (mut svm, admin, _agent) = gov_setup();
    let outsider = new_agent(&mut svm, &admin);
    fails_with(
        &mut svm,
        propose_ix(&outsider.pubkey(), ASSET, 1, MID_AMOUNT, HASH),
        &outsider,
        &[&outsider],
        "NotAssignedAgent",
    );
}

#[test]
fn propose_validates_the_request() {
    let (mut svm, _admin, agent) = gov_setup();
    fails_with(
        &mut svm,
        propose_ix(&agent.pubkey(), ASSET, 1, 0, HASH),
        &agent,
        &[&agent],
        "ZeroAmount",
    );
    fails_with(
        &mut svm,
        propose_ix(&agent.pubkey(), ASSET, 1, MID_AMOUNT, [0u8; 32]),
        &agent,
        &[&agent],
        "InvalidDetailsHash",
    );
    fails_with(
        &mut svm,
        propose_ix(&agent.pubkey(), ASSET, 2, MID_AMOUNT, HASH),
        &agent,
        &[&agent],
        "WrongGovernanceId",
    );
}

#[test]
fn one_proposal_at_a_time() {
    let (mut svm, _admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    fails_with(
        &mut svm,
        propose_ix(&agent.pubkey(), ASSET, 2, MID_AMOUNT, HASH),
        &agent,
        &[&agent],
        "ProposalOngoing",
    );
    // The running vote blocks the auto tier too.
    fails_with(
        &mut svm,
        propose_ix(&agent.pubkey(), ASSET, 2, LOW_PROPOSAL, HASH),
        &agent,
        &[&agent],
        "ProposalOngoing",
    );
}

#[test]
fn vote_moves_tally_and_locks_shares() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 50);

    vote(&mut svm, &voter, 1, VoteChoice::Yes, 30);
    let proposal = proposal_of(&svm, ASSET, 1);
    assert_eq!(proposal.tally.yes, 30);
    assert_eq!(holding_of(&svm, ASSET, &voter.pubkey()).locked(), 30);

    // A revote replaces the old vote and moves the lock by the difference.
    vote(&mut svm, &voter, 1, VoteChoice::No, 20);
    let proposal = proposal_of(&svm, ASSET, 1);
    assert_eq!(proposal.tally.yes, 0);
    assert_eq!(proposal.tally.no, 20);
    assert_eq!(holding_of(&svm, ASSET, &voter.pubkey()).locked(), 20);
}

#[test]
fn vote_rejects_bad_casts() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    fails_with(
        &mut svm,
        vote_proposal_ix(&voter.pubkey(), ASSET, 1, VoteChoice::Yes, 0),
        &voter,
        &[&voter, &sponsor()],
        "InvalidVoteAmount",
    );
    // More than the holding carries; the marketplace lock rejects it.
    fails_with(
        &mut svm,
        vote_proposal_ix(&voter.pubkey(), ASSET, 1, VoteChoice::Yes, 51),
        &voter,
        &[&voter, &sponsor()],
        "NotEnoughShares",
    );
    warp(&mut svm, VOTING_TIME + 1);
    fails_with(
        &mut svm,
        vote_proposal_ix(&voter.pubkey(), ASSET, 1, VoteChoice::Yes, 10),
        &voter,
        &[&voter, &sponsor()],
        "VotingClosed",
    );
}

#[test]
fn finalize_executes_a_carried_vote() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 60);
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 60);
    warp(&mut svm, VOTING_TIME + 1);
    let logs = finalize(&mut svm, &agent, 1);

    assert!(executed(&logs));
    assert!(account_gone(&svm, &proposal_pda(ASSET, 1)));
    assert_eq!(letting_of(&svm, ASSET).governance.active_proposal, 0);
    // The freed slot admits the next proposal under the next id.
    propose(&mut svm, &agent, 2, MID_AMOUNT);
}

#[test]
fn finalize_rejects_no_majority() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let yes = new_holder(&mut svm, &admin, ASSET, 30);
    let no = new_holder(&mut svm, &admin, ASSET, 30);
    vote(&mut svm, &yes, 1, VoteChoice::Yes, 30);
    vote(&mut svm, &no, 1, VoteChoice::No, 30);
    warp(&mut svm, VOTING_TIME + 1);
    let logs = finalize(&mut svm, &agent, 1);

    assert!(!executed(&logs));
    assert!(account_gone(&svm, &proposal_pda(ASSET, 1)));
    assert_eq!(letting_of(&svm, ASSET).governance.active_proposal, 0);
}

#[test]
fn finalize_rejects_quorum_miss() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    // Exactly the quorum share of supply; the check needs strictly more.
    let voter = new_holder(&mut svm, &admin, ASSET, 25);
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 25);
    warp(&mut svm, VOTING_TIME + 1);
    let logs = finalize(&mut svm, &agent, 1);
    assert!(!executed(&logs));
    assert!(account_gone(&svm, &proposal_pda(ASSET, 1)));
}

#[test]
fn abstentions_count_toward_quorum_only() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let yes = new_holder(&mut svm, &admin, ASSET, 10);
    let neutral = new_holder(&mut svm, &admin, ASSET, 40);
    vote(&mut svm, &yes, 1, VoteChoice::Yes, 10);
    vote(&mut svm, &neutral, 1, VoteChoice::Abstain, 40);
    warp(&mut svm, VOTING_TIME + 1);
    let logs = finalize(&mut svm, &agent, 1);
    assert!(executed(&logs));
}

#[test]
fn high_tier_needs_the_threshold() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, HIGH_PROPOSAL);
    let yes = new_holder(&mut svm, &admin, ASSET, 60);
    let no = new_holder(&mut svm, &admin, ASSET, 40);
    // 60% of the decided votes is a majority but under the 67% threshold.
    vote(&mut svm, &yes, 1, VoteChoice::Yes, 60);
    vote(&mut svm, &no, 1, VoteChoice::No, 40);
    warp(&mut svm, VOTING_TIME + 1);
    let logs = finalize(&mut svm, &agent, 1);
    assert!(!executed(&logs));

    // Fresh holders: the first round's votes are still locked.
    propose(&mut svm, &agent, 2, HIGH_PROPOSAL);
    let yes = new_holder(&mut svm, &admin, ASSET, 60);
    let no = new_holder(&mut svm, &admin, ASSET, 20);
    vote(&mut svm, &yes, 2, VoteChoice::Yes, 60);
    vote(&mut svm, &no, 2, VoteChoice::No, 20);
    warp(&mut svm, VOTING_TIME + 1);
    let logs = finalize(&mut svm, &agent, 2);
    assert!(executed(&logs));
}

#[test]
fn finalize_waits_for_expiry() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 60);
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 60);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_proposal_ix(&cranker.pubkey(), &agent.pubkey(), ASSET, 1),
        &cranker,
        &[&cranker],
        "VotingStillOngoing",
    );
}

// Votes run `< expiry` and the finalizer `>= expiry`: the boundary second
// flips straight from voting to counting.
#[test]
fn the_expiry_second_belongs_to_the_finalizer() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 60);
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 60);

    let expiry = proposal_of(&svm, ASSET, 1).expiry;
    warp_to(&mut svm, expiry);
    fails_with(
        &mut svm,
        vote_proposal_ix(&voter.pubkey(), ASSET, 1, VoteChoice::Yes, 10),
        &voter,
        &[&voter, &sponsor()],
        "VotingClosed",
    );
    finalize(&mut svm, &agent, 1);
    assert!(account_gone(&svm, &proposal_pda(ASSET, 1)));
}

#[test]
fn finalize_settles_after_the_agent_departs() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 60);
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 60);
    // The seat empties mid-vote; the proposal still settles and frees the
    // slot for a successor.
    set_letting_agent(&mut svm, ASSET, &Pubkey::default());
    warp(&mut svm, VOTING_TIME + 1);
    finalize(&mut svm, &agent, 1);
    assert!(account_gone(&svm, &proposal_pda(ASSET, 1)));
    assert_eq!(letting_of(&svm, ASSET).governance.active_proposal, 0);
}

#[test]
fn unlock_returns_shares_and_rent() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 30);
    fails_with(
        &mut svm,
        unlock_proposal_votes_ix(&voter.pubkey(), ASSET, 1),
        &voter,
        &[&voter],
        "VotingStillOngoing",
    );
    warp(&mut svm, VOTING_TIME + 1);
    ok(
        &mut svm,
        unlock_proposal_votes_ix(&voter.pubkey(), ASSET, 1),
        &voter,
        &[&voter],
    );
    assert_eq!(holding_of(&svm, ASSET, &voter.pubkey()).locked(), 0);
    assert!(account_gone(
        &svm,
        &proposal_vote_pda(ASSET, 1, &voter.pubkey())
    ));
}

#[test]
fn unlock_works_after_the_proposal_closed() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    vote(&mut svm, &voter, 1, VoteChoice::No, 30);
    warp(&mut svm, VOTING_TIME + 1);
    finalize(&mut svm, &agent, 1);
    assert!(account_gone(&svm, &proposal_pda(ASSET, 1)));
    ok(
        &mut svm,
        unlock_proposal_votes_ix(&voter.pubkey(), ASSET, 1),
        &voter,
        &[&voter],
    );
    assert_eq!(holding_of(&svm, ASSET, &voter.pubkey()).locked(), 0);
}

// --- challenges ---

#[test]
fn challenge_stakes_the_deposit() {
    let (mut svm, admin, agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    let vault_before = vault_balance(&svm);
    challenge(&mut svm, &challenger, 1);

    assert_eq!(vault_balance(&svm), vault_before + CHALLENGE_DEPOSIT);
    let record = challenge_of(&svm, ASSET, 1);
    assert_eq!(record.challenger, challenger.pubkey());
    assert_eq!(record.agent, agent.pubkey());
    assert_eq!(record.deposit, CHALLENGE_DEPOSIT);
    let gov = letting_of(&svm, ASSET).governance;
    assert_eq!(gov.challenge_count, 1);
    assert_eq!(gov.active_challenge, 1);
}

#[test]
fn challenge_requires_a_holder_and_a_seat() {
    let (mut svm, admin, _agent) = gov_setup();
    let empty = new_holder(&mut svm, &admin, ASSET, 0);
    give_xcav(&mut svm, &empty.pubkey(), FUND_XCAV);
    fails_with(
        &mut svm,
        challenge_ix(&empty.pubkey(), ASSET, 1, CHALLENGE_DEPOSIT),
        &empty,
        &[&empty],
        "NotAHolder",
    );

    seed_letting(&mut svm, ASSET, &Pubkey::default());
    let holder = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &holder.pubkey(), FUND_XCAV);
    fails_with(
        &mut svm,
        challenge_ix(&holder.pubkey(), ASSET, 1, CHALLENGE_DEPOSIT),
        &holder,
        &[&holder],
        "SeatVacant",
    );
}

#[test]
fn one_challenge_at_a_time() {
    let (mut svm, admin, _agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    fails_with(
        &mut svm,
        challenge_ix(&challenger.pubkey(), ASSET, 2, CHALLENGE_DEPOSIT),
        &challenger,
        &[&challenger],
        "ChallengeOngoing",
    );
}

#[test]
fn passed_challenge_slashes_and_strikes() {
    let (mut svm, admin, agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    let voter = new_holder(&mut svm, &admin, ASSET, 60);
    vote_challenge(&mut svm, &voter, 1, VoteChoice::Yes, 60);
    warp(&mut svm, VOTING_TIME + 1);
    let vault_before = vault_balance(&svm);
    finalize_challenge(&mut svm, &challenger, 1, Some(&agent.pubkey()));

    // The slash lands in the treasury, the stake goes back, the agent's
    // recorded deposit shrinks by exactly the slash.
    assert_eq!(balance_at(&svm, &xcav_ata(&treasury())), SLASH_AMOUNT);
    assert_eq!(
        balance_at(&svm, &xcav_ata(&challenger.pubkey())),
        CHALLENGE_DEPOSIT
    );
    assert_eq!(
        vault_balance(&svm),
        vault_before - SLASH_AMOUNT - CHALLENGE_DEPOSIT
    );
    let entry = agent_of(&svm, &agent.pubkey());
    assert_eq!(entry.locations[0].deposit, AGENT_DEPOSIT - SLASH_AMOUNT);
    let letting = letting_of(&svm, ASSET);
    assert_eq!(letting.governance.strikes, 1);
    assert_eq!(letting.agent, agent.pubkey());
    assert_eq!(letting.governance.active_challenge, 0);
    assert!(account_gone(&svm, &challenge_pda(ASSET, 1)));
}

#[test]
fn failed_challenge_forfeits_the_stake() {
    let (mut svm, admin, agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    let voter = new_holder(&mut svm, &admin, ASSET, 60);
    vote_challenge(&mut svm, &voter, 1, VoteChoice::No, 60);
    warp(&mut svm, VOTING_TIME + 1);
    finalize_challenge(&mut svm, &challenger, 1, Some(&agent.pubkey()));

    assert_eq!(balance_at(&svm, &xcav_ata(&treasury())), CHALLENGE_DEPOSIT);
    assert_eq!(balance_at(&svm, &xcav_ata(&challenger.pubkey())), 0);
    let entry = agent_of(&svm, &agent.pubkey());
    assert_eq!(entry.locations[0].deposit, AGENT_DEPOSIT);
    assert_eq!(letting_of(&svm, ASSET).governance.strikes, 0);
}

#[test]
fn quorum_miss_fails_the_challenge() {
    let (mut svm, admin, agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    let voter = new_holder(&mut svm, &admin, ASSET, 25);
    vote_challenge(&mut svm, &voter, 1, VoteChoice::Yes, 25);
    warp(&mut svm, VOTING_TIME + 1);
    finalize_challenge(&mut svm, &challenger, 1, Some(&agent.pubkey()));
    assert_eq!(balance_at(&svm, &xcav_ata(&treasury())), CHALLENGE_DEPOSIT);
    assert_eq!(letting_of(&svm, ASSET).governance.strikes, 0);
}

#[test]
fn three_strikes_remove_the_agent() {
    let (mut svm, admin, agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    let voter = new_holder(&mut svm, &admin, ASSET, 90);
    for id in 1..=3u64 {
        challenge(&mut svm, &challenger, id);
        vote_challenge(&mut svm, &voter, id, VoteChoice::Yes, 30);
        warp(&mut svm, VOTING_TIME + 1);
        finalize_challenge(&mut svm, &challenger, id, Some(&agent.pubkey()));
    }

    let letting = letting_of(&svm, ASSET);
    assert_eq!(letting.agent, Pubkey::default());
    assert_eq!(letting.governance.strikes, 0);
    let entry = agent_of(&svm, &agent.pubkey());
    assert_eq!(entry.locations[0].assigned_count, 0);
    assert_eq!(entry.locations[0].deposit, AGENT_DEPOSIT - 3 * SLASH_AMOUNT);
    assert_eq!(balance_at(&svm, &xcav_ata(&treasury())), 3 * SLASH_AMOUNT);
}

#[test]
fn slash_is_capped_by_the_recorded_deposit() {
    let (mut svm, admin, agent) = gov_setup();
    // Leave less deposit than one slash is worth.
    set_location_deposit(&mut svm, &agent.pubkey(), SLASH_AMOUNT / 2);
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    let voter = new_holder(&mut svm, &admin, ASSET, 60);
    vote_challenge(&mut svm, &voter, 1, VoteChoice::Yes, 60);
    warp(&mut svm, VOTING_TIME + 1);
    finalize_challenge(&mut svm, &challenger, 1, Some(&agent.pubkey()));

    assert_eq!(balance_at(&svm, &xcav_ata(&treasury())), SLASH_AMOUNT / 2);
    assert_eq!(agent_of(&svm, &agent.pubkey()).locations[0].deposit, 0);
    assert_eq!(letting_of(&svm, ASSET).governance.strikes, 1);
}

#[test]
fn passed_challenge_against_a_departed_agent_only_refunds() {
    let (mut svm, admin, agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    let voter = new_holder(&mut svm, &admin, ASSET, 60);
    vote_challenge(&mut svm, &voter, 1, VoteChoice::Yes, 60);
    // The agent leaves the seat while the vote runs.
    set_letting_agent(&mut svm, ASSET, &Pubkey::default());
    warp(&mut svm, VOTING_TIME + 1);
    finalize_challenge(&mut svm, &challenger, 1, None);

    assert_eq!(
        balance_at(&svm, &xcav_ata(&challenger.pubkey())),
        CHALLENGE_DEPOSIT
    );
    assert_eq!(balance_at(&svm, &xcav_ata(&treasury())), 0);
    assert_eq!(
        agent_of(&svm, &agent.pubkey()).locations[0].deposit,
        AGENT_DEPOSIT
    );
}

#[test]
fn challenge_vote_closes_at_expiry() {
    let (mut svm, admin, _agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    warp(&mut svm, VOTING_TIME + 1);
    fails_with(
        &mut svm,
        vote_challenge_ix(&voter.pubkey(), ASSET, 1, VoteChoice::Yes, 10),
        &voter,
        &[&voter, &sponsor()],
        "VotingClosed",
    );
}

// Same boundary rule for challenges: the expiry second belongs to the
// finalizer, not the voters.
#[test]
fn the_challenge_expiry_second_belongs_to_the_finalizer() {
    let (mut svm, admin, agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    vote_challenge(&mut svm, &voter, 1, VoteChoice::Yes, 30);

    let expiry = challenge_of(&svm, ASSET, 1).expiry;
    warp_to(&mut svm, expiry);
    fails_with(
        &mut svm,
        vote_challenge_ix(&voter.pubkey(), ASSET, 1, VoteChoice::Yes, 10),
        &voter,
        &[&voter, &sponsor()],
        "VotingClosed",
    );
    finalize_challenge(&mut svm, &challenger, 1, Some(&agent.pubkey()));
    assert!(account_gone(&svm, &challenge_pda(ASSET, 1)));
}

#[test]
fn strike_removal_leaves_a_closable_notice() {
    let (mut svm, admin, agent) = gov_setup();
    // The agent gives notice, then gets struck out before it runs down.
    ok(
        &mut svm,
        resign_ix(&agent.pubkey(), ASSET),
        &agent,
        &[&agent],
    );
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    let voter = new_holder(&mut svm, &admin, ASSET, 90);
    for id in 1..=3u64 {
        challenge(&mut svm, &challenger, id);
        vote_challenge(&mut svm, &voter, id, VoteChoice::Yes, 30);
        warp(&mut svm, VOTING_TIME + 1);
        finalize_challenge(&mut svm, &challenger, id, Some(&agent.pubkey()));
    }
    assert_eq!(letting_of(&svm, ASSET).agent, Pubkey::default());

    // The stale notice still closes, without touching the released
    // assignment again.
    warp(&mut svm, NOTICE_PERIOD + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_resignation_ix(&cranker.pubkey(), &agent.pubkey(), ASSET),
        &cranker,
        &[&cranker],
    );
    assert!(account_gone(&svm, &resignation_pda(ASSET)));
    assert_eq!(
        agent_of(&svm, &agent.pubkey()).locations[0].assigned_count,
        0
    );
}

#[test]
fn challenge_votes_unlock_after_expiry() {
    let (mut svm, admin, _agent) = gov_setup();
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    vote_challenge(&mut svm, &voter, 1, VoteChoice::Yes, 30);
    fails_with(
        &mut svm,
        unlock_challenge_votes_ix(&voter.pubkey(), ASSET, 1),
        &voter,
        &[&voter],
        "VotingStillOngoing",
    );
    warp(&mut svm, VOTING_TIME + 1);
    ok(
        &mut svm,
        unlock_challenge_votes_ix(&voter.pubkey(), ASSET, 1),
        &voter,
        &[&voter],
    );
    assert_eq!(holding_of(&svm, ASSET, &voter.pubkey()).locked(), 0);
}

#[test]
fn listed_shares_carry_no_vote_weight() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    // A holder with 30 of 50 shares up for sale can only vote the rest.
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    seed_holding_full(&mut svm, ASSET, &voter.pubkey(), 50, 0, 30);
    fails_with(
        &mut svm,
        vote_proposal_ix(&voter.pubkey(), ASSET, 1, VoteChoice::Yes, 21),
        &voter,
        &[&voter, &sponsor()],
        "NotEnoughShares",
    );
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 20);
}

#[test]
fn proposal_and_challenge_votes_carry_full_weight_at_once() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let challenger = new_holder(&mut svm, &admin, ASSET, 10);
    give_xcav(&mut svm, &challenger.pubkey(), FUND_XCAV);
    challenge(&mut svm, &challenger, 1);

    // Locks per reason overlap instead of adding up, so both votes get the
    // holder's whole weight and the effective lock stays at their balance.
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 50);
    vote_challenge(&mut svm, &voter, 1, VoteChoice::No, 50);

    assert_eq!(proposal_of(&svm, ASSET, 1).tally.yes, 50);
    assert_eq!(challenge_of(&svm, ASSET, 1).tally.no, 50);
    let holding = holding_of(&svm, ASSET, &voter.pubkey());
    assert_eq!(holding.locks[LockReason::Proposal as usize], 50);
    assert_eq!(holding.locks[LockReason::Challenge as usize], 50);
    assert_eq!(holding.locked(), 50);

    // Each vote still can't exceed the balance on its own.
    fails_with(
        &mut svm,
        vote_proposal_ix(&voter.pubkey(), ASSET, 1, VoteChoice::Yes, 51),
        &voter,
        &[&voter, &sponsor()],
        "NotEnoughShares",
    );

    // The unlocks are independent: releasing one vote leaves the other's
    // lock standing.
    warp(&mut svm, VOTING_TIME + 1);
    ok(
        &mut svm,
        unlock_proposal_votes_ix(&voter.pubkey(), ASSET, 1),
        &voter,
        &[&voter],
    );
    assert_eq!(holding_of(&svm, ASSET, &voter.pubkey()).locked(), 50);
    ok(
        &mut svm,
        unlock_challenge_votes_ix(&voter.pubkey(), ASSET, 1),
        &voter,
        &[&voter],
    );
    assert_eq!(holding_of(&svm, ASSET, &voter.pubkey()).locked(), 0);
}

#[test]
fn unlock_pins_the_proposal_account() {
    let (mut svm, admin, agent) = gov_setup();
    propose(&mut svm, &agent, 1, MID_AMOUNT);
    let voter = new_holder(&mut svm, &admin, ASSET, 50);
    vote(&mut svm, &voter, 1, VoteChoice::Yes, 30);
    warp(&mut svm, VOTING_TIME + 1);

    // A different proposal's account standing where this vote's must be.
    let mut ix = unlock_proposal_votes_ix(&voter.pubkey(), ASSET, 1);
    swap_account(&mut ix, proposal_pda(ASSET, 1), proposal_pda(ASSET, 2));
    fails_with(&mut svm, ix, &voter, &[&voter], "WrongGovernanceId");
}
