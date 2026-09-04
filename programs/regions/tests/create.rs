//! v2 region lifecycle: a passed proposal is claimed by its proposer (creating
//! the region), and an open seat is taken over first-come by another operator
//! bonding 0.1% of XCAV supply. No auctions.

mod common;
use common::*;

// ============================ create (claim a passed region) ============================

#[test]
fn create_region_makes_proposer_the_operator() {
    let (mut svm, operator, authority) = setup();
    reach_passed(&mut svm, &operator, &authority);
    assert_eq!(region_state_of(&svm, 1).status, RegionStatus::Passed);

    ok(
        &mut svm,
        create_region_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );

    let region = region_of(&svm, 1);
    assert_eq!(region.owner, operator.pubkey());
    // The bond locked when proposing (DEPOSIT) is now the region's collateral.
    assert_eq!(region.collateral, DEPOSIT);
    // The region state was closed.
    assert!(svm
        .get_account(&region_state(1))
        .is_none_or(|a| a.data.is_empty()));
}

#[test]
fn create_region_only_by_proposer() {
    let (mut svm, operator, authority) = setup();
    reach_passed(&mut svm, &operator, &authority);

    // A different operator can't claim a region they didn't propose.
    let other = new_operator(&mut svm, &authority);
    fails_with(
        &mut svm,
        create_region_ix(&other.pubkey(), 1),
        &other,
        &[&other],
        "NotProposer",
    );
}

#[test]
fn create_region_requires_passed() {
    let (mut svm, operator, _authority) = setup();
    let id = next_proposal_id(&svm);
    ok(
        &mut svm,
        propose_ix(&operator.pubkey(), 1, id),
        &operator,
        &[&operator],
    );
    // Still Proposing (not finalized), so it can't be created yet.
    fails_with(
        &mut svm,
        create_region_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
        "RegionNotPassed",
    );
}

#[test]
fn create_region_rechecks_operator_role() {
    let (mut svm, operator, authority) = setup();
    reach_passed(&mut svm, &operator, &authority);

    // The proposer loses their RegionalOperator role before claiming: the role
    // PDA no longer resolves, so they can't take the seat.
    let admin = funded(&mut svm);
    ok(
        &mut svm,
        roles_add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );
    let payer = role_rent_payer(&svm, &operator.pubkey(), Role::RegionalOperator);
    ok(
        &mut svm,
        roles_remove_ix(
            &admin.pubkey(),
            &operator.pubkey(),
            Role::RegionalOperator,
            &payer,
        ),
        &admin,
        &[&admin],
    );
    fails_with(
        &mut svm,
        create_region_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
        "AccountNotInitialized",
    );
}

// Resignation is an exit path, so it works even after the operator's role is
// revoked; their collateral countdown must never depend on an admin's mercy.
#[test]
fn resignation_works_without_role() {
    let (mut svm, operator, authority) = setup();
    reach_created(&mut svm, &operator, &authority);

    let admin = funded(&mut svm);
    ok(
        &mut svm,
        roles_add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );
    let payer = role_rent_payer(&svm, &operator.pubkey(), Role::RegionalOperator);
    ok(
        &mut svm,
        roles_remove_ix(
            &admin.pubkey(),
            &operator.pubkey(),
            Role::RegionalOperator,
            &payer,
        ),
        &admin,
        &[&admin],
    );

    ok(
        &mut svm,
        resign_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );
    warp(&mut svm, 6_000);
    // Another operator can then claim the seat, refunding the collateral.
    let newop = new_operator(&mut svm, &authority);
    ok(
        &mut svm,
        claim_open_region_ix(&newop.pubkey(), 1, &operator.pubkey()),
        &newop,
        &[&newop],
    );
    assert_eq!(xcav_balance(&svm, &operator.pubkey()), FUND_XCAV);
}

// ============================ claim an open seat ============================

#[test]
fn claim_open_region_changes_operator_and_refunds_old() {
    let (mut svm, operator, authority) = setup();
    // Region created, then the seat opened via resignation + warp past notice.
    reach_seat_open(&mut svm, &operator, &authority);

    let old_before = xcav_balance(&svm, &operator.pubkey());
    let newop = new_operator(&mut svm, &authority);
    let new_before = xcav_balance(&svm, &newop.pubkey());

    ok(
        &mut svm,
        claim_open_region_ix(&newop.pubkey(), 1, &operator.pubkey()),
        &newop,
        &[&newop],
    );

    let region = region_of(&svm, 1);
    assert_eq!(region.owner, newop.pubkey());
    assert_eq!(region.collateral, DEPOSIT);
    assert_eq!(xcav_balance(&svm, &operator.pubkey()) - old_before, DEPOSIT);
    assert_eq!(new_before - xcav_balance(&svm, &newop.pubkey()), DEPOSIT);
}

#[test]
fn incumbent_renews_own_open_seat() {
    let (mut svm, operator, authority) = setup();
    // Seat opened via the operator's own resignation.
    reach_seat_open(&mut svm, &operator, &authority);

    let before = xcav_balance(&svm, &operator.pubkey());
    let vault_before = vault_balance(&svm);
    let old_change = region_of(&svm, 1).next_owner_change;

    ok(
        &mut svm,
        renew_region_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );

    let region = region_of(&svm, 1);
    // Same operator keeps the seat; the term is pushed out again.
    assert_eq!(region.owner, operator.pubkey());
    assert_eq!(region.collateral, DEPOSIT);
    assert!(region.next_owner_change > old_change);
    // The bond is unchanged (supply is fixed in the test), so nothing moves.
    assert_eq!(xcav_balance(&svm, &operator.pubkey()), before);
    assert_eq!(vault_balance(&svm), vault_before);
}

#[test]
fn incumbent_renew_tops_up_when_bond_rises() {
    let (mut svm, operator, authority) = setup();
    reach_seat_open(&mut svm, &operator, &authority);
    // XCAV supply doubles, so the live 0.1% bond does too; renewing costs the delta.
    set_mint_supply(&mut svm, DEPOSIT * 2_000);

    let before = xcav_balance(&svm, &operator.pubkey());
    let vault_before = vault_balance(&svm);
    ok(
        &mut svm,
        renew_region_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );

    // Collateral rises to the new bond; only the difference moves into the vault.
    assert_eq!(region_of(&svm, 1).collateral, DEPOSIT * 2);
    assert_eq!(xcav_balance(&svm, &operator.pubkey()), before - DEPOSIT);
    assert_eq!(vault_balance(&svm), vault_before + DEPOSIT);
}

#[test]
fn incumbent_renew_refunds_when_bond_falls() {
    let (mut svm, operator, authority) = setup();
    reach_seat_open(&mut svm, &operator, &authority);
    // XCAV supply halves, so the live bond does too; the surplus is refunded.
    set_mint_supply(&mut svm, DEPOSIT * 500);

    let before = xcav_balance(&svm, &operator.pubkey());
    let vault_before = vault_balance(&svm);
    ok(
        &mut svm,
        renew_region_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );

    // Collateral falls to the new bond; the surplus returns to the operator.
    assert_eq!(region_of(&svm, 1).collateral, DEPOSIT / 2);
    assert_eq!(xcav_balance(&svm, &operator.pubkey()), before + DEPOSIT / 2);
    assert_eq!(vault_balance(&svm), vault_before - DEPOSIT / 2);
}

// The deadline second closes the claim and opens the clear-out.
#[test]
fn the_deadline_second_closes_the_claim() {
    let (mut svm, operator, authority) = setup();
    reach_passed(&mut svm, &operator, &authority);

    let deadline = region_state_of(&svm, 1).claim_deadline;
    warp_to(&mut svm, deadline);
    fails_with(
        &mut svm,
        create_region_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
        "ClaimWindowClosed",
    );
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        clear_ix(&cranker.pubkey(), 1, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
}

// The term's last second still belongs to the incumbent; at the boundary the
// lame-duck rule and the open seat flip at the same instant.
#[test]
fn the_term_boundary_opens_the_seat() {
    let (mut svm, operator, authority) = setup();
    reach_created(&mut svm, &operator, &authority);
    let newop = new_operator(&mut svm, &authority);

    let term_end = region_of(&svm, 1).next_owner_change;
    warp_to(&mut svm, term_end - 1);
    fails_with(
        &mut svm,
        claim_open_region_ix(&newop.pubkey(), 1, &operator.pubkey()),
        &newop,
        &[&newop],
        "RegionOwnerCantBeChanged",
    );
    warp(&mut svm, 1);
    fails_with(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
        "SeatOpen",
    );
    ok(
        &mut svm,
        claim_open_region_ix(&newop.pubkey(), 1, &operator.pubkey()),
        &newop,
        &[&newop],
    );
    assert_eq!(region_of(&svm, 1).owner, newop.pubkey());
}

// ============================ resignation ============================

#[test]
fn resignation_requires_owner() {
    let (mut svm, operator, authority) = setup();
    reach_created(&mut svm, &operator, &authority);

    let other = new_operator(&mut svm, &authority);
    fails_with(
        &mut svm,
        resign_ix(&other.pubkey(), 1),
        &other,
        &[&other],
        "NotRegionOwner",
    );
}

#[test]
fn resign_fails_when_change_already_scheduled() {
    let (mut svm, operator, authority) = setup();
    reach_created(&mut svm, &operator, &authority);

    ok(
        &mut svm,
        resign_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );
    // A second resignation can't push the change back out.
    fails_with(
        &mut svm,
        resign_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
        "OwnerChangeAlreadyScheduled",
    );
}

// ============================ stale passed cleanup ============================

#[test]
fn stale_passed_region_clears_and_refunds_bond() {
    let (mut svm, operator, authority) = setup();
    reach_passed(&mut svm, &operator, &authority);

    // The proposer never claims; past the claim deadline (owner_change_period,
    // 10_000) anyone can clear the state and the bond is refunded.
    let op_before = xcav_balance(&svm, &operator.pubkey());
    warp(&mut svm, 11_000);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        clear_ix(&cranker.pubkey(), 1, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );

    assert_eq!(xcav_balance(&svm, &operator.pubkey()) - op_before, DEPOSIT);
    assert!(svm
        .get_account(&region_state(1))
        .is_none_or(|a| a.data.is_empty()));
}

// A takeover charges the deposits the locations actually locked, not the
// current config rate, so a config change between lock and turnover never
// drifts the standing bond or the vault.
#[test]
fn takeover_charges_recorded_location_deposits() {
    let (mut svm, operator, authority) = setup();
    reach_created(&mut svm, &operator, &authority);
    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );
    let locked = DEPOSIT + LOCATION_DEPOSIT;
    assert_eq!(region_of(&svm, 1).collateral, locked);
    assert_eq!(region_of(&svm, 1).location_collateral, LOCATION_DEPOSIT);

    // The location deposit doubles after the operator already paid theirs.
    let mut params = default_params();
    params.location_deposit = 2 * LOCATION_DEPOSIT;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    ok(
        &mut svm,
        resign_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );
    warp(&mut svm, 6_000);

    let old_before = xcav_balance(&svm, &operator.pubkey());
    let vault_before = vault_balance(&svm);
    let newop = new_operator(&mut svm, &authority);
    let new_before = xcav_balance(&svm, &newop.pubkey());
    ok(
        &mut svm,
        claim_open_region_ix(&newop.pubkey(), 1, &operator.pubkey()),
        &newop,
        &[&newop],
    );

    // The newcomer assumes exactly what the locations locked; the outgoing
    // operator gets exactly that back; the vault doesn't move.
    assert_eq!(region_of(&svm, 1).collateral, locked);
    assert_eq!(new_before - xcav_balance(&svm, &newop.pubkey()), locked);
    assert_eq!(xcav_balance(&svm, &operator.pubkey()) - old_before, locked);
    assert_eq!(vault_balance(&svm), vault_before);
}

// The bond invariant survives a deposit change, a takeover, and a removal in
// sequence: removing the last location leaves the new operator holding
// exactly the operator bond, never less.
#[test]
fn takeover_then_remove_location_keeps_bond_intact() {
    let (mut svm, operator, authority) = setup();
    reach_created(&mut svm, &operator, &authority);
    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );

    // The configured deposit collapses to a hundredth after the lock.
    let mut params = default_params();
    params.location_deposit = LOCATION_DEPOSIT / 100;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );
    ok(
        &mut svm,
        resign_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );
    warp(&mut svm, 6_000);
    let newop = new_operator(&mut svm, &authority);
    ok(
        &mut svm,
        claim_open_region_ix(&newop.pubkey(), 1, &operator.pubkey()),
        &newop,
        &[&newop],
    );

    let before = xcav_balance(&svm, &newop.pubkey());
    ok(
        &mut svm,
        remove_location_ix(&newop.pubkey(), 1, b"SW1A1AA"),
        &newop,
        &[&newop],
    );

    // The recorded deposit comes back and the seat still carries the full
    // operator bond.
    assert_eq!(
        xcav_balance(&svm, &newop.pubkey()) - before,
        LOCATION_DEPOSIT
    );
    let region = region_of(&svm, 1);
    assert_eq!(region.collateral, DEPOSIT);
    assert_eq!(region.location_collateral, 0);
    assert_eq!(region.location_count, 0);
}

#[test]
fn claim_open_region_rejects_deposit_above_cap() {
    let (mut svm, operator, authority) = setup();
    reach_seat_open(&mut svm, &operator, &authority);

    let newop = new_operator(&mut svm, &authority);
    fails_with(
        &mut svm,
        claim_open_region_ix_capped(&newop.pubkey(), 1, &operator.pubkey(), DEPOSIT - 1),
        &newop,
        &[&newop],
        "DepositTooHigh",
    );
}
