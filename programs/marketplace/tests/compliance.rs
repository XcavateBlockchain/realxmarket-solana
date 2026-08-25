//! The compliance gate: who it clears, who it turns away, and the two ways a
//! wallet stops being cleared. Roles are the whitelist program's own suite;
//! this covers what the marketplace does with the record. `list_property`
//! stands in for every gate, since they all check it the same way.

mod common;
use common::*;

/// A funded developer with a region and location to list into. `new_developer`
/// clears them as part of onboarding.
fn listable() -> (LiteSVM, Keypair, Keypair) {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    (svm, admin, developer)
}

fn now(svm: &LiteSVM) -> i64 {
    svm.get_sysvar::<Clock>().unix_timestamp
}

/// A successful listing consumes its id, so the cases that list more than once
/// have to move on to the next one.
fn lists_ok(svm: &mut LiteSVM, developer: &Keypair, listing_id: u64) {
    ok(
        svm,
        list_ix(&developer.pubkey(), listing_id),
        developer,
        &[developer],
    );
}

fn listing_fails(svm: &mut LiteSVM, developer: &Keypair, listing_id: u64, error: &str) {
    fails_with(
        svm,
        list_ix(&developer.pubkey(), listing_id),
        developer,
        &[developer],
        error,
    );
}

#[test]
fn a_cleared_wallet_passes() {
    let (mut svm, _admin, developer) = listable();
    lists_ok(&mut svm, &developer, 0);
}

// Screening is dated, so a record that lapses stops clearing the wallet with
// nobody having to act. This is the case a one-time onboarding check misses.
#[test]
fn lapsed_screening_is_rejected() {
    let (mut svm, admin, developer) = listable();
    let expires_at = now(&svm) + 1_000;
    clear_compliance_until(&mut svm, &admin, &developer.pubkey(), expires_at);
    lists_ok(&mut svm, &developer, 0);

    warp(&mut svm, 1_001);
    listing_fails(&mut svm, &developer, 1, "NotCompliant");
}

// Re-screening renews the same record rather than needing a new account.
#[test]
fn renewing_clears_the_wallet_again() {
    let (mut svm, admin, developer) = listable();
    let expires_at = now(&svm) + 1_000;
    clear_compliance_until(&mut svm, &admin, &developer.pubkey(), expires_at);
    warp(&mut svm, 1_001);
    listing_fails(&mut svm, &developer, 0, "NotCompliant");

    let renewed = now(&svm) + 10_000;
    clear_compliance_until(&mut svm, &admin, &developer.pubkey(), renewed);
    lists_ok(&mut svm, &developer, 0);
}

// A sanctions hit blocks the wallet outright, and unlike a lapse it does not
// heal on its own.
#[test]
fn a_blocked_wallet_is_rejected() {
    let (mut svm, admin, developer) = listable();
    set_compliance(&mut svm, &admin, &developer.pubkey(), false);

    listing_fails(&mut svm, &developer, 0, "NotCompliant");
}

#[test]
fn blocking_survives_the_passage_of_time() {
    let (mut svm, admin, developer) = listable();
    set_compliance(&mut svm, &admin, &developer.pubkey(), false);
    warp(&mut svm, 400 * 86_400);

    listing_fails(&mut svm, &developer, 0, "NotCompliant");
}

// A wallet holding a role but never screened has no record at all, which the
// seeds constraint turns into an uninitialized account rather than a verdict.
#[test]
fn an_unscreened_wallet_is_rejected() {
    let (mut svm, admin, _developer) = listable();
    let stranger = actor(&mut svm);
    ok(
        &mut svm,
        roles_assign_ix(
            &admin.pubkey(),
            &stranger.pubkey(),
            Role::RealEstateDeveloper,
        ),
        &admin,
        &[&admin],
    );

    listing_fails(&mut svm, &stranger, 0, "AccountNotInitialized");
}

// The record is seeds-pinned to its wallet, so a cleared holder's record can't
// be passed in to cover someone who isn't.
#[test]
fn another_wallets_record_is_rejected() {
    let (mut svm, admin, developer) = listable();
    let cleared = new_developer(&mut svm, &admin);
    set_compliance(&mut svm, &admin, &developer.pubkey(), false);

    let mut ix = list_ix(&developer.pubkey(), 0);
    for meta in &mut ix.accounts {
        if meta.pubkey == compliance_pda(&developer.pubkey()) {
            meta.pubkey = compliance_pda(&cleared.pubkey());
        }
    }
    fails_with(&mut svm, ix, &developer, &[&developer], "ConstraintSeeds");
}
