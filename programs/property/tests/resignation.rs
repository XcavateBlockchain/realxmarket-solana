//! Agent resignation: the notice period, the crank that opens the seat, and
//! the seat lifecycle around it.

mod common;
use common::*;

const REGION: u16 = 1;
const ASSET: u64 = 7;

/// Full cycle to an assigned agent: register, claim, win the vote, finalize.
fn setup_with_assigned_agent() -> (LiteSVM, Keypair, Keypair) {
    let (mut svm, admin, _authority) = setup();
    seed_location(&mut svm, REGION, POSTCODE);
    seed_property_asset(&mut svm, ASSET, REGION, POSTCODE);
    let agent = new_agent(&mut svm, &admin);
    ok(
        &mut svm,
        add_agent_ix(&agent.pubkey(), REGION, POSTCODE, u64::MAX),
        &agent,
        &[&agent],
    );
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    let holder = new_holder(&mut svm, &admin, ASSET, 40);
    let sponsor = sponsor();
    ok(
        &mut svm,
        vote_agent_ix(&holder.pubkey(), ASSET, 1, &agent.pubkey(), None, 40),
        &holder,
        &[&holder, &sponsor],
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
    assert_eq!(letting_of(&svm, ASSET).agent, agent.pubkey());
    (svm, admin, agent)
}

#[test]
fn seat_blocks_claims_while_assigned() {
    let (mut svm, admin, _agent) = setup_with_assigned_agent();
    let challenger = new_agent(&mut svm, &admin);
    ok(
        &mut svm,
        add_agent_ix(&challenger.pubkey(), REGION, POSTCODE, u64::MAX),
        &challenger,
        &[&challenger],
    );
    fails_with(
        &mut svm,
        claim_property_ix(&challenger.pubkey(), ASSET, 2),
        &challenger,
        &[&challenger],
        "SeatTaken",
    );
}

#[test]
fn assignment_blocks_leaving_the_location() {
    let (mut svm, _admin, agent) = setup_with_assigned_agent();
    fails_with(
        &mut svm,
        remove_agent_ix(&agent.pubkey(), POSTCODE),
        &agent,
        &[&agent],
        "AgentStillAssigned",
    );
}

#[test]
fn resign_files_a_notice() {
    let (mut svm, _admin, agent) = setup_with_assigned_agent();
    ok(
        &mut svm,
        resign_ix(&agent.pubkey(), ASSET),
        &agent,
        &[&agent],
    );

    let notice = notice_of(&svm, ASSET);
    assert_eq!(notice.agent, agent.pubkey());
    assert!(notice.due_ts > 0);
    // Still the assigned agent until the notice runs out.
    assert_eq!(letting_of(&svm, ASSET).agent, agent.pubkey());
    // And only one notice per property.
    fails_with(
        &mut svm,
        resign_ix(&agent.pubkey(), ASSET),
        &agent,
        &[&agent],
        "already in use",
    );
}

#[test]
fn resign_is_for_the_assigned_agent_only() {
    let (mut svm, admin, _agent) = setup_with_assigned_agent();
    let outsider = new_agent(&mut svm, &admin);
    fails_with(
        &mut svm,
        resign_ix(&outsider.pubkey(), ASSET),
        &outsider,
        &[&outsider],
        "NotAssignedAgent",
    );
}

#[test]
fn finalize_waits_out_the_notice_period() {
    let (mut svm, _admin, agent) = setup_with_assigned_agent();
    ok(
        &mut svm,
        resign_ix(&agent.pubkey(), ASSET),
        &agent,
        &[&agent],
    );

    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_resignation_ix(&cranker.pubkey(), &agent.pubkey(), ASSET),
        &cranker,
        &[&cranker],
        "NoticePeriodRunning",
    );

    warp(&mut svm, NOTICE_PERIOD + 1);
    ok(
        &mut svm,
        finalize_resignation_ix(&cranker.pubkey(), &agent.pubkey(), ASSET),
        &cranker,
        &[&cranker],
    );

    assert_eq!(letting_of(&svm, ASSET).agent, Pubkey::default());
    assert_eq!(
        agent_of(&svm, &agent.pubkey()).locations[0].assigned_count,
        0
    );
    assert!(account_gone(&svm, &resignation_pda(ASSET)));
    // With the assignment released, the agent can leave the location again.
    ok(
        &mut svm,
        remove_agent_ix(&agent.pubkey(), POSTCODE),
        &agent,
        &[&agent],
    );
}

// The due second itself is the first one where the notice can settle.
#[test]
fn the_due_second_frees_the_agent() {
    let (mut svm, _admin, agent) = setup_with_assigned_agent();
    ok(
        &mut svm,
        resign_ix(&agent.pubkey(), ASSET),
        &agent,
        &[&agent],
    );

    let cranker = funded(&mut svm);
    warp(&mut svm, NOTICE_PERIOD - 1);
    fails_with(
        &mut svm,
        finalize_resignation_ix(&cranker.pubkey(), &agent.pubkey(), ASSET),
        &cranker,
        &[&cranker],
        "NoticePeriodRunning",
    );
    warp(&mut svm, 1);
    ok(
        &mut svm,
        finalize_resignation_ix(&cranker.pubkey(), &agent.pubkey(), ASSET),
        &cranker,
        &[&cranker],
    );
    assert_eq!(letting_of(&svm, ASSET).agent, Pubkey::default());
}

#[test]
fn seat_reopens_for_a_fresh_election_after_resignation() {
    let (mut svm, admin, agent) = setup_with_assigned_agent();
    ok(
        &mut svm,
        resign_ix(&agent.pubkey(), ASSET),
        &agent,
        &[&agent],
    );
    warp(&mut svm, NOTICE_PERIOD + 1);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_resignation_ix(&cranker.pubkey(), &agent.pubkey(), ASSET),
        &cranker,
        &[&cranker],
    );

    let challenger = new_agent(&mut svm, &admin);
    ok(
        &mut svm,
        add_agent_ix(&challenger.pubkey(), REGION, POSTCODE, u64::MAX),
        &challenger,
        &[&challenger],
    );
    ok(
        &mut svm,
        claim_property_ix(&challenger.pubkey(), ASSET, 2),
        &challenger,
        &[&challenger],
    );
    assert_eq!(letting_of(&svm, ASSET).election.round, 2);
}
