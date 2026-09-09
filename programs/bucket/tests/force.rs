//! Authority-only manager seating and teardown.

mod common;
use common::*;

#[test]
fn authority_seats_and_unseats_managers() {
    let (mut svm, authority) = setup();
    let creator = funded(&mut svm);
    let other = funded(&mut svm);
    let ns = namespace(&mut svm, &creator);

    fails_with(
        &mut svm,
        force_add_manager_ix(&creator.pubkey(), ns, &other.pubkey()),
        &creator,
        "NotAuthority",
    );
    ok(
        &mut svm,
        force_add_manager_ix(&authority.pubkey(), ns, &other.pubkey()),
        &authority,
    );
    assert_eq!(namespace_of(&svm, ns).manager_count, 2);
    let seated = lamports(&svm, &authority.pubkey());

    // The authority paid the seat, so the refund is theirs.
    fails_with(
        &mut svm,
        force_remove_manager_ix(&authority.pubkey(), ns, &other.pubkey(), &creator.pubkey()),
        &authority,
        "WrongRentPayer",
    );
    ok(
        &mut svm,
        force_remove_manager_ix(
            &authority.pubkey(),
            ns,
            &other.pubkey(),
            &authority.pubkey(),
        ),
        &authority,
    );
    assert!(lamports(&svm, &authority.pubkey()) > seated);

    // Unlike remove_manager, the last manager can go.
    fails_with(
        &mut svm,
        remove_manager_ix(&creator.pubkey(), ns, &creator.pubkey(), &creator.pubkey()),
        &creator,
        "LastManager",
    );
    ok(
        &mut svm,
        force_remove_manager_ix(
            &authority.pubkey(),
            ns,
            &creator.pubkey(),
            &creator.pubkey(),
        ),
        &authority,
    );
    assert_eq!(namespace_of(&svm, ns).manager_count, 0);
    assert!(!exists(&svm, &manager_pda(ns, &creator.pubkey())));
}

#[test]
fn messages_and_tags_are_removed_leaf_first() {
    let (mut svm, authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let writer = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = open_bucket(&mut svm, &manager, ns, &admin, &writer);
    ok(&mut svm, create_tag_ix(&admin.pubkey(), b, "deed"), &admin);
    let deed = tag_pda(b, "deed");
    ok(
        &mut svm,
        write_ix(
            &writer.pubkey(),
            b,
            0,
            msg_input("cid-1", Some("deed")),
            Some(deed),
        ),
        &writer,
    );
    ok(
        &mut svm,
        write_ix(&writer.pubkey(), b, 1, msg_input("cid-2", None), None),
        &writer,
    );

    fails_with(
        &mut svm,
        force_remove_tag_ix(&authority.pubkey(), b, "deed", &admin.pubkey()),
        &authority,
        "TagInUse",
    );
    fails_with(
        &mut svm,
        force_remove_message_ix(&admin.pubkey(), b, 0, &writer.pubkey(), Some(deed)),
        &admin,
        "NotAuthority",
    );
    // A tagged message needs its tag passed, an untagged one must not have one.
    fails_with(
        &mut svm,
        force_remove_message_ix(&authority.pubkey(), b, 0, &writer.pubkey(), None),
        &authority,
        "WrongTag",
    );
    fails_with(
        &mut svm,
        force_remove_message_ix(&authority.pubkey(), b, 1, &writer.pubkey(), Some(deed)),
        &authority,
        "WrongTag",
    );

    let before = lamports(&svm, &writer.pubkey());
    ok(
        &mut svm,
        force_remove_message_ix(&authority.pubkey(), b, 0, &writer.pubkey(), Some(deed)),
        &authority,
    );
    ok(
        &mut svm,
        force_remove_message_ix(&authority.pubkey(), b, 1, &writer.pubkey(), None),
        &authority,
    );
    assert!(lamports(&svm, &writer.pubkey()) > before);
    assert!(!exists(&svm, &message_pda(b, 0)));
    assert_eq!(tag_of(&svm, b, "deed").message_count, 0);
    let state = bucket_of(&svm, b);
    assert_eq!(state.message_count, 0);
    assert_eq!(state.next_message_id, 2);

    ok(
        &mut svm,
        force_remove_tag_ix(&authority.pubkey(), b, "deed", &admin.pubkey()),
        &authority,
    );
    assert!(!exists(&svm, &deed));
    assert_eq!(bucket_of(&svm, b).tag_count, 0);
}

#[test]
fn buckets_and_namespaces_go_last() {
    let (mut svm, authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let writer = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = open_bucket(&mut svm, &manager, ns, &admin, &writer);
    ok(&mut svm, add_viewer_ix(&admin.pubkey(), b, KEY2), &admin);
    ok(&mut svm, create_tag_ix(&admin.pubkey(), b, "deed"), &admin);
    ok(
        &mut svm,
        write_ix(&writer.pubkey(), b, 0, msg_input("cid-1", None), None),
        &writer,
    );

    let auth = authority.pubkey();
    fails_with(
        &mut svm,
        force_remove_namespace_ix(&auth, ns, &manager.pubkey()),
        &authority,
        "DanglingBuckets",
    );
    let remove_bucket = || force_remove_bucket_ix(&auth, ns, b, &manager.pubkey());
    // Contributors and viewers are removed by an admin, so the admin seat
    // comes and goes while the other blockers are checked.
    let seat_admin = |svm: &mut LiteSVM| {
        ok(
            svm,
            add_admin_ix(&manager.pubkey(), ns, b, &admin.pubkey()),
            &manager,
        )
    };
    let unseat_admin = |svm: &mut LiteSVM| {
        ok(
            svm,
            remove_admin_ix(&manager.pubkey(), ns, b, &admin.pubkey(), &manager.pubkey()),
            &manager,
        )
    };

    fails_with(&mut svm, remove_bucket(), &authority, "DanglingMessages");
    ok(
        &mut svm,
        force_remove_message_ix(&auth, b, 0, &writer.pubkey(), None),
        &authority,
    );
    fails_with(&mut svm, remove_bucket(), &authority, "DanglingAdmins");
    unseat_admin(&mut svm);
    fails_with(
        &mut svm,
        remove_bucket(),
        &authority,
        "DanglingContributors",
    );
    seat_admin(&mut svm);
    ok(
        &mut svm,
        remove_contributor_ix(&admin.pubkey(), b, &writer.pubkey(), &admin.pubkey()),
        &admin,
    );
    unseat_admin(&mut svm);
    fails_with(&mut svm, remove_bucket(), &authority, "DanglingViewers");
    seat_admin(&mut svm);
    ok(
        &mut svm,
        remove_viewer_ix(&admin.pubkey(), b, KEY2, &admin.pubkey()),
        &admin,
    );
    unseat_admin(&mut svm);
    fails_with(&mut svm, remove_bucket(), &authority, "DanglingTags");
    ok(
        &mut svm,
        force_remove_tag_ix(&auth, b, "deed", &admin.pubkey()),
        &authority,
    );

    let before = lamports(&svm, &manager.pubkey());
    ok(&mut svm, remove_bucket(), &authority);
    assert!(lamports(&svm, &manager.pubkey()) > before);
    assert!(!exists(&svm, &bucket_pda(b)));
    assert_eq!(namespace_of(&svm, ns).bucket_count, 0);

    fails_with(
        &mut svm,
        force_remove_namespace_ix(&auth, ns, &manager.pubkey()),
        &authority,
        "DanglingManagers",
    );
    ok(
        &mut svm,
        force_remove_manager_ix(&auth, ns, &manager.pubkey(), &manager.pubkey()),
        &authority,
    );
    let before = lamports(&svm, &manager.pubkey());
    ok(
        &mut svm,
        force_remove_namespace_ix(&auth, ns, &manager.pubkey()),
        &authority,
    );
    assert!(lamports(&svm, &manager.pubkey()) > before);
    assert!(!exists(&svm, &namespace_pda(ns)));
    // Ids are never reused.
    assert_eq!(config_of(&svm).next_namespace_id, ns + 1);
}
