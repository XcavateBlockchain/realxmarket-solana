//! Region proposal lifecycle: proposing, voting, finalizing, and reclaiming or
//! clearing state afterwards.

mod common;
use common::*;

// ============================ propose ============================

#[test]
fn propose_new_region_works() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    let op_before = xcav_balance(&svm, &operator.pubkey());

    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let acc = svm.get_account(&proposal_pda(id)).unwrap();
    let proposal = RegionProposal::try_deserialize(&mut &acc.data[..]).unwrap();
    assert_eq!(proposal.proposer, operator.pubkey());
    assert_eq!(proposal.region_id, 1);
    assert_eq!(proposal.yes_power, 0);
    // The bond and the name are tracked on the region state.
    let state = region_state_of(&svm, 1);
    assert_eq!(state.deposit, DEPOSIT);
    assert_eq!(state.name, REGION_NAME);
    // Bond moved from proposer into the vault.
    assert_eq!(op_before - xcav_balance(&svm, &operator.pubkey()), DEPOSIT);
    assert_eq!(vault_balance(&svm), DEPOSIT);
    // Counter advanced.
    assert_eq!(next_proposal_id(&svm), id + 1);
}

#[test]
fn propose_fails_for_non_operator() {
    let (mut svm, _operator, _authority) = setup();
    let stranger = actor(&mut svm);
    let id = next_proposal_id(&svm);
    // No RegionalOperator role -> the role PDA doesn't exist.
    fails_with(
        &mut svm,
        propose_ix(&stranger.pubkey(), 1, id),
        &stranger,
        &[&stranger],
        "AccountNotInitialized",
    );
}

// Any nonzero id may be proposed; zero is reserved.
#[test]
fn propose_accepts_any_nonzero_id_and_rejects_zero() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    fails_with(
        &mut svm,
        propose_ix(&operator.pubkey(), 0, id),
        &operator,
        &[&operator],
        "InvalidRegion",
    );
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 999, id),
        &operator,
        &[&operator],
    );
    assert_eq!(region_state_of(&svm, 999).region_id, 999);
}

#[test]
fn propose_rejects_bad_names() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    let too_long = "A".repeat(regions::state::MAX_REGION_NAME_LEN + 1);
    for name in ["", too_long.as_str()] {
        fails_with(
            &mut svm,
            propose_ix_named(&operator.pubkey(), 1, name, id, u64::MAX),
            &operator,
            &[&operator],
            "InvalidRegionName",
        );
    }
    let max = "A".repeat(regions::state::MAX_REGION_NAME_LEN);
    ok(
        &mut svm,
        propose_ix_named(&operator.pubkey(), 1, &max, id, u64::MAX),
        &operator,
        &[&operator],
    );
}

#[test]
fn propose_fails_when_region_already_has_proposal() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    // Second proposal for region 1 -> the region pointer already exists.
    let id2 = next_proposal_id(&svm);
    fails_with(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id2),
        &operator,
        &[&operator],
        "already in use",
    );
}

// ============================ vote ============================

#[test]
fn vote_records_power() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let voter = actor(&mut svm);
    let voter_before = xcav_balance(&svm, &voter.pubkey());
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );

    let proposal =
        RegionProposal::try_deserialize(&mut &svm.get_account(&proposal_pda(id)).unwrap().data[..])
            .unwrap();
    assert_eq!(proposal.yes_power, 200_000_000);

    let vr_acc = svm
        .get_account(&vote_record_pda(id, &voter.pubkey()))
        .unwrap();
    let vr = VoteRecord::try_deserialize(&mut &vr_acc.data[..]).unwrap();
    assert_eq!(vr.power, 200_000_000);
    assert_eq!(vr.vote, Vote::Yes);
    // Power moved from voter into the vault (on top of the proposal deposit).
    assert_eq!(
        voter_before - xcav_balance(&svm, &voter.pubkey()),
        200_000_000
    );
    assert_eq!(vault_balance(&svm), DEPOSIT + 200_000_000);
}

#[test]
fn vote_below_minimum_fails() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let voter = actor(&mut svm);
    fails_with(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 1),
        &voter,
        &[&voter],
        "BelowMinimumVotingAmount",
    );
}

#[test]
fn revote_replaces_previous_vote() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let voter = actor(&mut svm);
    let voter_before = xcav_balance(&svm, &voter.pubkey());
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );
    // Change vote to No with a different amount.
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::No, 300_000_000),
        &voter,
        &[&voter],
    );

    let proposal =
        RegionProposal::try_deserialize(&mut &svm.get_account(&proposal_pda(id)).unwrap().data[..])
            .unwrap();
    assert_eq!(proposal.yes_power, 0);
    assert_eq!(proposal.no_power, 300_000_000);

    let vr = VoteRecord::try_deserialize(
        &mut &svm
            .get_account(&vote_record_pda(id, &voter.pubkey()))
            .unwrap()
            .data[..],
    )
    .unwrap();
    assert_eq!(vr.vote, Vote::No);
    assert_eq!(vr.power, 300_000_000);
    // Net XCAV locked equals only the new vote; the old lock was refunded.
    assert_eq!(
        voter_before - xcav_balance(&svm, &voter.pubkey()),
        300_000_000
    );
    assert_eq!(vault_balance(&svm), DEPOSIT + 300_000_000);
}

// Power can't be rented for one slot: the 100s minimum hold puts the
// cutoff 100s before expiry, and its first second is already too late.
#[test]
fn the_cutoff_second_is_too_late_to_vote() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    warp(&mut svm, 1_000 - 100 - 1);
    let early = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&early.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &early,
        &[&early],
    );
    warp(&mut svm, 1);
    let late = actor(&mut svm);
    fails_with(
        &mut svm,
        vote_ix(&late.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &late,
        &[&late],
        "VoteTooLate",
    );
}

// At the expiry second voting is over and finalizing is allowed, with no
// second belonging to both.
#[test]
fn the_expiry_second_belongs_to_the_finalizer() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );

    warp(&mut svm, 1_000);
    let late = actor(&mut svm);
    fails_with(
        &mut svm,
        vote_ix(&late.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &late,
        &[&late],
        "ProposalExpired",
    );
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Passed);
}

#[test]
fn vote_with_insufficient_balance_fails() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    // A voter who holds more than the minimum but less than they try to lock.
    let poor = funded(&mut svm);
    give_xcav(&mut svm, &poor.pubkey(), 150_000_000);
    fails_with(
        &mut svm,
        vote_ix(&poor.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &poor,
        &[&poor],
        "insufficient funds",
    );
}

// ============================ finalize ============================

#[test]
fn finalize_passes_and_marks_claimable() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );

    let op_before = xcav_balance(&svm, &operator.pubkey());
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );

    let rs = region_state_of(&svm, 1);
    assert_eq!(rs.status, RegionStatus::Passed);
    assert_eq!(rs.deposit, DEPOSIT);
    // On a pass the bond stays locked as the region's collateral-to-be, so the
    // proposer's XCAV balance is unchanged and the vault keeps bond + vote lock.
    assert_eq!(xcav_balance(&svm, &operator.pubkey()), op_before);
    assert_eq!(vault_balance(&svm), DEPOSIT + 200_000_000);
    // Proposal account was closed.
    assert!(svm
        .get_account(&proposal_pda(id))
        .is_none_or(|a| a.data.is_empty()));
}

#[test]
fn finalize_rejects_and_refunds_bond() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    // No votes -> quorum not met -> rejected.

    let op_before = xcav_balance(&svm, &operator.pubkey());
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );

    let rs = region_state_of(&svm, 1);
    assert_eq!(rs.status, RegionStatus::Rejected);
    // The bond is returned in full ("unbonded").
    assert_eq!(xcav_balance(&svm, &operator.pubkey()) - op_before, DEPOSIT);
    assert_eq!(vault_balance(&svm), 0);
}

#[test]
fn finalize_fails_while_voting_ongoing() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
        "VotingStillOngoing",
    );
}

// ============================ cleanup ============================

#[test]
fn unlock_voting_token_works() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );
    let voter_before = xcav_balance(&svm, &voter.pubkey());

    warp_past_voting(&mut svm);
    ok(&mut svm, unlock_ix(&voter.pubkey(), id), &voter, &[&voter]);

    // Vote record closed and the locked power returned as XCAV.
    assert!(svm
        .get_account(&vote_record_pda(id, &voter.pubkey()))
        .is_none_or(|a| a.data.is_empty()));
    assert_eq!(
        xcav_balance(&svm, &voter.pubkey()) - voter_before,
        200_000_000
    );
    // Only the proposal deposit is left in the vault.
    assert_eq!(vault_balance(&svm), DEPOSIT);
}

#[test]
fn unlock_fails_while_voting_ongoing() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );
    // Window is still open, so the lock can't be reclaimed yet.
    fails_with(
        &mut svm,
        unlock_ix(&voter.pubkey(), id),
        &voter,
        &[&voter],
        "VotingStillOngoing",
    );
}

#[test]
fn unlock_without_vote_fails() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    // Someone who never voted has no vote record to unlock.
    let stranger = actor(&mut svm);
    warp_past_voting(&mut svm);
    fails_with(
        &mut svm,
        unlock_ix(&stranger.pubkey(), id),
        &stranger,
        &[&stranger],
        "AccountNotInitialized",
    );
}

#[test]
fn clear_region_state_after_reject() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    // No votes -> rejected on finalize.
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Rejected);

    ok(
        &mut svm,
        clear_ix(&cranker.pubkey(), 1, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert!(svm
        .get_account(&region_state(1))
        .is_none_or(|a| a.data.is_empty()));
}

// ============================ threshold / quorum ============================

// Abstain power counts toward quorum but carries no Yes support, so
// an all-abstain proposal must NOT pass on `0 >= 0`. It is rejected even when
// quorum is cleared.
#[test]
fn finalize_all_abstain_is_rejected() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Abstain, 200_000_000),
        &voter,
        &[&voter],
    );
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Rejected);
}

// Only Yes power counts toward quorum (Governor Bravo style), so an operator
// can't self-approve a region by padding a small Yes with a large Abstain.
#[test]
fn abstain_padding_cannot_reach_quorum() {
    let (mut svm, operator, authority) = setup();
    // Raise quorum above the voting minimum so a below-quorum Yes is castable.
    let mut params = default_params();
    params.quorum = 300_000_000;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    // Yes is below quorum; the abstain would top the old yes+no+abstain total.
    let yes_voter = actor(&mut svm);
    let abstain_voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&yes_voter.pubkey(), 1, id, Vote::Yes, 100_000_000),
        &yes_voter,
        &[&yes_voter],
    );
    ok(
        &mut svm,
        vote_ix(&abstain_voter.pubkey(), 1, id, Vote::Abstain, 300_000_000),
        &abstain_voter,
        &[&abstain_voter],
    );
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Rejected);
}

#[test]
fn finalize_exact_threshold_passes() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    // yes == no is exactly the 50% threshold (yes*2 >= yes+no holds at equality).
    let yes_voter = actor(&mut svm);
    let no_voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&yes_voter.pubkey(), 1, id, Vote::Yes, 100_000_000),
        &yes_voter,
        &[&yes_voter],
    );
    ok(
        &mut svm,
        vote_ix(&no_voter.pubkey(), 1, id, Vote::No, 100_000_000),
        &no_voter,
        &[&no_voter],
    );
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Passed);
}

#[test]
fn finalize_below_threshold_rejects() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    // yes < no fails the threshold even though quorum is met.
    let yes_voter = actor(&mut svm);
    let no_voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&yes_voter.pubkey(), 1, id, Vote::Yes, 100_000_000),
        &yes_voter,
        &[&yes_voter],
    );
    ok(
        &mut svm,
        vote_ix(&no_voter.pubkey(), 1, id, Vote::No, 200_000_000),
        &no_voter,
        &[&no_voter],
    );
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Rejected);
}

#[test]
fn finalize_below_quorum_rejects() {
    let (mut svm, _operator, authority) = setup();
    // Raise the quorum above a single vote so a lone yes cannot reach it.
    let mut params = default_params();
    params.quorum = 300_000_000;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    let operator = new_operator(&mut svm, &authority);
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    // Unanimous yes, but total 200M is below the 300M quorum.
    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Rejected);
}

#[test]
fn finalize_exact_quorum_passes() {
    let (mut svm, _operator, authority) = setup();
    // Quorum is the minimum total power: a turnout exactly at the quorum counts.
    let mut params = default_params();
    params.quorum = 200_000_000;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    let operator = new_operator(&mut svm, &authority);
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Passed);
}

#[test]
fn finalize_pass_without_proposer_token_works() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );
    warp_past_voting(&mut svm);

    // A pass keeps the bond as collateral (no XCAV moves to the proposer), so
    // the crank settles even after the proposer closed their token account.
    svm.set_account(token_acc(&operator.pubkey()), Account::default())
        .unwrap();
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Passed);
}

// A proposer closing their token account must not wedge the reject path: the
// crank recreates the associated account and the refund lands there.
#[test]
fn finalize_reject_survives_closed_proposer_token() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    warp_past_voting(&mut svm);

    svm.set_account(token_acc(&operator.pubkey()), Account::default())
        .unwrap();
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Rejected);
    assert_eq!(xcav_balance(&svm, &operator.pubkey()), DEPOSIT);
}

// A passing proposal keeps the bond locked as the region's collateral; the
// proposer gets nothing back until the seat turns over.
#[test]
fn finalize_pass_keeps_bond_locked() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );

    let op_before = xcav_balance(&svm, &operator.pubkey());
    warp_past_voting(&mut svm);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );

    // Bond stays locked (balance unchanged).
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Passed);
    assert_eq!(xcav_balance(&svm, &operator.pubkey()), op_before);
    assert_eq!(vault_balance(&svm), DEPOSIT + 200_000_000);
}

#[test]
fn propose_rejects_deposit_above_cap() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    fails_with(
        &mut svm,
        propose_ix_capped(&operator.pubkey(), 1, id, DEPOSIT - 1),
        &operator,
        &[&operator],
        "DepositTooHigh",
    );
}

// The vote cutoff is snapshotted when the proposal opens; a later config
// change can't shrink (or silence) an in-flight proposal's window.
#[test]
fn vote_cutoff_is_snapshotted_at_propose() {
    let (mut svm, operator, authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );

    // The authority stretches both the window and the hold so that, read
    // live, the whole remaining window would be inside the hold.
    let mut params = default_params();
    params.voting_period = 10_000;
    params.min_vote_hold = 9_999;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    // 150s before the original expiry: still outside the original 100s hold.
    warp(&mut svm, 850);
    let voter = actor(&mut svm);
    ok(
        &mut svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );
}
