//! Lawyer registry: registration deposits, the active-case unregister guard,
//! and the role/region gates.

mod common;
use common::*;

/// Full setup plus a created region 1, ready for lawyers to register against.
fn setup_with_region() -> (LiteSVM, Keypair, Keypair) {
    let (mut svm, admin, authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    (svm, admin, authority)
}

#[test]
fn register_locks_deposit_and_creates_account() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);

    let before = xcav_balance(&svm, &lawyer.pubkey());
    let vault_before = vault_balance(&svm);
    ok(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 1),
        &lawyer,
        &[&lawyer, &sponsor()],
    );

    let entry = lawyer_of(&svm, &lawyer.pubkey());
    assert_eq!(entry.lawyer, lawyer.pubkey());
    assert_eq!(entry.region_id, 1);
    assert_eq!(entry.deposit, LAWYER_DEPOSIT);
    assert_eq!(entry.active_cases, 0);
    assert_eq!(
        before - xcav_balance(&svm, &lawyer.pubkey()),
        LAWYER_DEPOSIT
    );
    assert_eq!(vault_balance(&svm), vault_before + LAWYER_DEPOSIT);
}

#[test]
fn register_requires_lawyer_role() {
    let (mut svm, _admin, _authority) = setup_with_region();

    // No Lawyer role: the role PDA doesn't resolve.
    let stranger = actor(&mut svm);
    fails_with(
        &mut svm,
        register_lawyer_ix(&stranger.pubkey(), 1),
        &stranger,
        &[&stranger, &sponsor()],
        "AccountNotInitialized",
    );
}

#[test]
fn register_requires_existing_region() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);

    // Region 2 was never created, so its PDA is empty.
    fails_with(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 2),
        &lawyer,
        &[&lawyer, &sponsor()],
        "AccountNotInitialized",
    );
}

#[test]
fn register_twice_fails() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);
    ok(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 1),
        &lawyer,
        &[&lawyer, &sponsor()],
    );

    // One registration per wallet, even for another region.
    let other_operator = funded(&mut svm);
    seed_region(&mut svm, 3, &other_operator.pubkey());
    fails_with(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 3),
        &lawyer,
        &[&lawyer, &sponsor()],
        "already in use",
    );
}

// Registering checks role possession only. The compliance flag gates the
// marketplace's investor-fund flows, not registry membership.
#[test]
fn register_ignores_compliance() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);
    set_compliance(&mut svm, &admin, &lawyer.pubkey(), false);

    ok(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 1),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    assert_eq!(lawyer_of(&svm, &lawyer.pubkey()).region_id, 1);
}

#[test]
fn unregister_returns_deposit_and_closes() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);
    ok(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 1),
        &lawyer,
        &[&lawyer, &sponsor()],
    );

    let before = xcav_balance(&svm, &lawyer.pubkey());
    let vault_before = vault_balance(&svm);
    let sponsor_before = svm.get_account(&sponsor().pubkey()).unwrap().lamports;
    ok(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
    );

    // The deposit is the lawyer's own money; the entry's rent was fronted by
    // the sponsor and goes back there.
    assert_eq!(
        xcav_balance(&svm, &lawyer.pubkey()) - before,
        LAWYER_DEPOSIT
    );
    assert_eq!(vault_balance(&svm), vault_before - LAWYER_DEPOSIT);
    assert!(svm.get_account(&sponsor().pubkey()).unwrap().lamports > sponsor_before);
    assert!(svm
        .get_account(&lawyer_pda(&lawyer.pubkey()))
        .is_none_or(|a| a.data.is_empty()));
}

// The refund is the deposit recorded at registration, not the current config
// rate, so a deposit raise can never over-draw the vault.
#[test]
fn unregister_refunds_recorded_deposit_after_config_change() {
    let (mut svm, admin, authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);
    ok(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 1),
        &lawyer,
        &[&lawyer, &sponsor()],
    );

    let mut params = default_params();
    params.lawyer_deposit = 2 * LAWYER_DEPOSIT;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    let before = xcav_balance(&svm, &lawyer.pubkey());
    ok(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
    );
    assert_eq!(
        xcav_balance(&svm, &lawyer.pubkey()) - before,
        LAWYER_DEPOSIT
    );
}

#[test]
fn unregister_with_active_cases_fails() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);
    ok(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 1),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    set_active_cases(&mut svm, &lawyer.pubkey(), 1);

    fails_with(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
        "LawyerStillActive",
    );

    // Once the last case closes, the exit works again.
    set_active_cases(&mut svm, &lawyer.pubkey(), 0);
    ok(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
    );
}

#[test]
fn unregister_without_registration_fails() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);

    fails_with(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
        "AccountNotInitialized",
    );
}

#[test]
fn register_rejects_deposit_above_cap() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);
    fails_with(
        &mut svm,
        register_lawyer_ix_capped(&lawyer.pubkey(), 1, LAWYER_DEPOSIT - 1),
        &lawyer,
        &[&lawyer, &sponsor()],
        "DepositTooHigh",
    );
}

// Unregistering is an exit path: it works even after the lawyer's role is
// revoked, so an admin action can never strand the deposit.
#[test]
fn unregister_works_after_role_removed() {
    let (mut svm, admin, _authority) = setup_with_region();
    let lawyer = new_lawyer(&mut svm, &admin);
    ok(
        &mut svm,
        register_lawyer_ix(&lawyer.pubkey(), 1),
        &lawyer,
        &[&lawyer, &sponsor()],
    );
    ok(
        &mut svm,
        roles_remove_ix(
            &admin.pubkey(),
            &lawyer.pubkey(),
            Role::Lawyer,
            &admin.pubkey(),
        ),
        &admin,
        &[&admin],
    );

    let before = xcav_balance(&svm, &lawyer.pubkey());
    ok(
        &mut svm,
        unregister_lawyer_ix(&lawyer.pubkey()),
        &lawyer,
        &[&lawyer],
    );
    assert_eq!(
        xcav_balance(&svm, &lawyer.pubkey()) - before,
        LAWYER_DEPOSIT
    );
}
