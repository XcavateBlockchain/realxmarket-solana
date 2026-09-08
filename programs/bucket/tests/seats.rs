//! Bucket creation and the admin, contributor and viewer seats.

mod common;
use common::*;

#[test]
fn managers_create_locked_buckets() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let outsider = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);

    fails_with(
        &mut svm,
        create_bucket_ix(&outsider.pubkey(), ns, 0, bucket_meta("deeds")),
        &outsider,
        "AccountNotInitialized",
    );
    ok(
        &mut svm,
        create_bucket_ix(&manager.pubkey(), ns, 0, bucket_meta("deeds")),
        &manager,
    );
    let b = bucket_of(&svm, 0);
    assert_eq!(b.namespace_id, ns);
    assert_eq!(b.name, "deeds");
    assert_eq!(b.category, "legal");
    assert_eq!(b.encryption_key, None);
    assert_eq!(b.next_message_id, 0);
    assert_eq!(b.admin_count, 0);
    assert_eq!(b.rent_payer, manager.pubkey());
    assert_eq!(namespace_of(&svm, ns).bucket_count, 1);
    assert_eq!(config_of(&svm).next_bucket_id, 1);
    let size = svm.get_account(&bucket_pda(0)).unwrap().data.len();
    assert_eq!(size, Bucket::space(&bucket_meta("deeds")));

    // Bucket ids are global, so a second namespace's bucket is number 1.
    let ns2 = namespace(&mut svm, &manager);
    ok(
        &mut svm,
        create_bucket_ix(&manager.pubkey(), ns2, 1, bucket_meta("plans")),
        &manager,
    );
    assert_eq!(bucket_of(&svm, 1).namespace_id, ns2);
}

#[test]
fn create_bounds_the_metadata() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let mut meta = bucket_meta("x");
    meta.category = "c".repeat(51);
    fails_with(
        &mut svm,
        create_bucket_ix(&manager.pubkey(), ns, 0, meta),
        &manager,
        "MetadataTooLong",
    );
}

#[test]
fn managers_seat_admins_and_only_managers() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let other = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = bucket(&mut svm, &manager, ns, &admin);
    assert_eq!(bucket_of(&svm, b).admin_count, 1);

    // An admin is not a manager, and a manager of another namespace is not
    // a manager here.
    fails_with(
        &mut svm,
        add_admin_ix(&admin.pubkey(), ns, b, &other.pubkey()),
        &admin,
        "AccountNotInitialized",
    );
    let ns2 = namespace(&mut svm, &other);
    fails_with(
        &mut svm,
        add_admin_ix(&other.pubkey(), ns2, b, &other.pubkey()),
        &other,
        "ConstraintSeeds",
    );

    let before = lamports(&svm, &manager.pubkey());
    ok(
        &mut svm,
        remove_admin_ix(&manager.pubkey(), ns, b, &admin.pubkey(), &manager.pubkey()),
        &manager,
    );
    assert_eq!(bucket_of(&svm, b).admin_count, 0);
    assert!(!exists(&svm, &admin_pda(b, &admin.pubkey())));
    assert!(lamports(&svm, &manager.pubkey()) > before);
}

#[test]
fn admins_seat_contributors_and_viewers() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let writer = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = bucket(&mut svm, &manager, ns, &admin);

    // Managers do not hold the admin seat.
    fails_with(
        &mut svm,
        add_contributor_ix(&manager.pubkey(), b, &writer.pubkey()),
        &manager,
        "AccountNotInitialized",
    );
    ok(
        &mut svm,
        add_contributor_ix(&admin.pubkey(), b, &writer.pubkey()),
        &admin,
    );
    ok(&mut svm, add_viewer_ix(&admin.pubkey(), b, KEY), &admin);
    let state = bucket_of(&svm, b);
    assert_eq!(state.contributor_count, 1);
    assert_eq!(state.viewer_count, 1);
    assert!(exists(&svm, &viewer_pda(b, &KEY)));

    ok(
        &mut svm,
        remove_contributor_ix(&admin.pubkey(), b, &writer.pubkey(), &admin.pubkey()),
        &admin,
    );
    ok(
        &mut svm,
        remove_viewer_ix(&admin.pubkey(), b, KEY, &admin.pubkey()),
        &admin,
    );
    let state = bucket_of(&svm, b);
    assert_eq!(state.contributor_count, 0);
    assert_eq!(state.viewer_count, 0);
    assert!(!exists(&svm, &contributor_pda(b, &writer.pubkey())));
}

#[test]
fn seats_are_per_bucket() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let writer = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b1 = bucket(&mut svm, &manager, ns, &admin);
    let b2 = config_of(&svm).next_bucket_id;
    ok(
        &mut svm,
        create_bucket_ix(&manager.pubkey(), ns, b2, bucket_meta("plans")),
        &manager,
    );

    fails_with(
        &mut svm,
        add_contributor_ix(&admin.pubkey(), b2, &writer.pubkey()),
        &admin,
        "AccountNotInitialized",
    );
    ok(
        &mut svm,
        add_contributor_ix(&admin.pubkey(), b1, &writer.pubkey()),
        &admin,
    );
}

#[test]
fn a_seat_cannot_be_granted_twice() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = bucket(&mut svm, &manager, ns, &admin);

    fails_with(
        &mut svm,
        add_admin_ix(&manager.pubkey(), ns, b, &admin.pubkey()),
        &manager,
        "already in use",
    );
    assert_eq!(bucket_of(&svm, b).admin_count, 1);
}
