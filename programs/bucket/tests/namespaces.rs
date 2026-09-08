//! Config authority, namespace creation and the manager seat.

mod common;
use common::*;

#[test]
fn initialize_is_bound_to_the_upgrade_authority() {
    let mut svm = LiteSVM::new();
    svm.add_program(pid(), &program_bytes()).unwrap();
    let authority = funded(&mut svm);
    let stranger = funded(&mut svm);
    bind_upgrade_authority(&mut svm, &authority.pubkey());

    fails_with(
        &mut svm,
        init_ix(&stranger.pubkey()),
        &stranger,
        "NotUpgradeAuthority",
    );
    ok(&mut svm, init_ix(&authority.pubkey()), &authority);
    let config = config_of(&svm);
    assert_eq!(config.authority, authority.pubkey());
    assert_eq!(config.next_namespace_id, 0);
    assert_eq!(config.next_bucket_id, 0);
}

#[test]
fn authority_handover_is_two_step() {
    let (mut svm, authority) = setup();
    let next = funded(&mut svm);
    let stranger = funded(&mut svm);

    fails_with(
        &mut svm,
        update_authority_ix(&stranger.pubkey(), &next.pubkey()),
        &stranger,
        "NotAuthority",
    );
    fails_with(
        &mut svm,
        accept_authority_ix(&next.pubkey()),
        &next,
        "NotPendingAuthority",
    );
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), &next.pubkey()),
        &authority,
    );
    assert_eq!(config_of(&svm).authority, authority.pubkey());
    ok(&mut svm, accept_authority_ix(&next.pubkey()), &next);
    let config = config_of(&svm);
    assert_eq!(config.authority, next.pubkey());
    assert_eq!(config.pending_authority, None);
}

#[test]
fn create_seats_the_creator_as_manager() {
    let (mut svm, _authority) = setup();
    let creator = funded(&mut svm);
    let before = lamports(&svm, &creator.pubkey());

    let id = namespace(&mut svm, &creator);
    assert_eq!(id, 0);
    let ns = namespace_of(&svm, id);
    assert_eq!(ns.name, "Property namespace");
    assert_eq!(
        ns.schema_uri.as_deref(),
        Some("https://xcavate.io/schema/property")
    );
    assert_eq!(ns.properties, vec![prop("propertyId", "42")]);
    assert_eq!(ns.manager_count, 1);
    assert_eq!(ns.bucket_count, 0);
    assert_eq!(ns.rent_payer, creator.pubkey());
    assert!(exists(&svm, &manager_pda(id, &creator.pubkey())));
    assert!(lamports(&svm, &creator.pubkey()) < before);

    // Sized to the metadata, not to the maximum.
    let size = svm.get_account(&namespace_pda(id)).unwrap().data.len();
    assert_eq!(size, Namespace::space(&ns_meta("Property namespace")));

    assert_eq!(namespace(&mut svm, &creator), 1);
    assert_eq!(config_of(&svm).next_namespace_id, 2);
}

#[test]
fn create_bounds_the_metadata() {
    let (mut svm, _authority) = setup();
    let creator = funded(&mut svm);

    let mut long_name = ns_meta("x");
    long_name.name = "n".repeat(MAX_NAME_LEN + 1);
    let mut many = ns_meta("x");
    many.properties = (0..=MAX_PROPERTIES)
        .map(|i| prop(&format!("k{i}"), "v"))
        .collect();
    let mut dup = ns_meta("x");
    dup.properties = vec![prop("k", "a"), prop("k", "b")];
    let mut long_value = ns_meta("x");
    long_value.properties = vec![prop("k", &"v".repeat(201))];

    for (meta, expected) in [
        (long_name, "MetadataTooLong"),
        (many, "TooManyProperties"),
        (dup, "DuplicateProperty"),
        (long_value, "MetadataTooLong"),
    ] {
        fails_with(
            &mut svm,
            create_namespace_ix(&creator.pubkey(), 0, meta),
            &creator,
            expected,
        );
    }
}

#[test]
fn managers_seat_and_unseat_managers() {
    let (mut svm, _authority) = setup();
    let first = funded(&mut svm);
    let second = funded(&mut svm);
    let outsider = funded(&mut svm);
    let ns = namespace(&mut svm, &first);

    fails_with(
        &mut svm,
        add_manager_ix(&outsider.pubkey(), ns, &second.pubkey()),
        &outsider,
        "AccountNotInitialized",
    );
    ok(
        &mut svm,
        add_manager_ix(&first.pubkey(), ns, &second.pubkey()),
        &first,
    );
    assert_eq!(namespace_of(&svm, ns).manager_count, 2);
    assert!(exists(&svm, &manager_pda(ns, &second.pubkey())));

    // The new manager holds the full seat: they can unseat the first, and
    // the rent goes back to the first, who paid for their own record.
    let before = lamports(&svm, &first.pubkey());
    ok(
        &mut svm,
        remove_manager_ix(&second.pubkey(), ns, &first.pubkey(), &first.pubkey()),
        &second,
    );
    assert_eq!(namespace_of(&svm, ns).manager_count, 1);
    assert!(!exists(&svm, &manager_pda(ns, &first.pubkey())));
    assert!(lamports(&svm, &first.pubkey()) > before);
}

// The signer's seat and the target seat are the same account here.
#[test]
fn a_manager_may_step_down() {
    let (mut svm, _authority) = setup();
    let first = funded(&mut svm);
    let second = funded(&mut svm);
    let ns = namespace(&mut svm, &first);
    ok(
        &mut svm,
        add_manager_ix(&first.pubkey(), ns, &second.pubkey()),
        &first,
    );

    ok(
        &mut svm,
        remove_manager_ix(&second.pubkey(), ns, &second.pubkey(), &first.pubkey()),
        &second,
    );
    assert_eq!(namespace_of(&svm, ns).manager_count, 1);
    assert!(!exists(&svm, &manager_pda(ns, &second.pubkey())));
    assert!(exists(&svm, &manager_pda(ns, &first.pubkey())));
}

#[test]
fn the_last_manager_stays() {
    let (mut svm, _authority) = setup();
    let only = funded(&mut svm);
    let ns = namespace(&mut svm, &only);

    fails_with(
        &mut svm,
        remove_manager_ix(&only.pubkey(), ns, &only.pubkey(), &only.pubkey()),
        &only,
        "LastManager",
    );
}

#[test]
fn removal_refunds_the_recorded_payer_only() {
    let (mut svm, _authority) = setup();
    let first = funded(&mut svm);
    let second = funded(&mut svm);
    let ns = namespace(&mut svm, &first);
    ok(
        &mut svm,
        add_manager_ix(&first.pubkey(), ns, &second.pubkey()),
        &first,
    );

    fails_with(
        &mut svm,
        remove_manager_ix(&first.pubkey(), ns, &second.pubkey(), &second.pubkey()),
        &first,
        "WrongRentPayer",
    );
}
