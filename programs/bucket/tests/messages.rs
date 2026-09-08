//! Tags and writing messages.

mod common;
use common::*;

#[test]
fn contributors_write_to_open_buckets() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let writer = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = bucket(&mut svm, &manager, ns, &admin);
    ok(
        &mut svm,
        add_contributor_ix(&admin.pubkey(), b, &writer.pubkey()),
        &admin,
    );

    fails_with(
        &mut svm,
        write_ix(&writer.pubkey(), b, 0, msg_input("cid-1", None), None),
        &writer,
        "BucketLocked",
    );
    ok(&mut svm, resume_ix(&admin.pubkey(), b, KEY), &admin);

    // Admins are not contributors.
    fails_with(
        &mut svm,
        write_ix(&admin.pubkey(), b, 0, msg_input("cid-1", None), None),
        &admin,
        "AccountNotInitialized",
    );
    let before = lamports(&svm, &writer.pubkey());
    ok(
        &mut svm,
        write_ix(&writer.pubkey(), b, 0, msg_input("cid-1", None), None),
        &writer,
    );
    let m = message_of(&svm, b, 0);
    assert_eq!(m.bucket_id, b);
    assert_eq!(m.id, 0);
    assert_eq!(m.reference, "cid-1");
    assert_eq!(m.tag, None);
    assert_eq!(m.content_hash, HASH);
    assert_eq!(m.properties, vec![prop("pages", "3")]);
    assert_eq!(m.contributor, writer.pubkey());
    assert!(lamports(&svm, &writer.pubkey()) < before);
    let size = svm.get_account(&message_pda(b, 0)).unwrap().data.len();
    assert_eq!(size, Message::space(&msg_input("cid-1", None)));

    ok(
        &mut svm,
        write_ix(&writer.pubkey(), b, 1, msg_input("cid-2", None), None),
        &writer,
    );
    let state = bucket_of(&svm, b);
    assert_eq!(state.next_message_id, 2);
    assert_eq!(state.message_count, 2);

    // Pausing stops further writes.
    ok(&mut svm, pause_ix(&admin.pubkey(), b), &admin);
    fails_with(
        &mut svm,
        write_ix(&writer.pubkey(), b, 2, msg_input("cid-3", None), None),
        &writer,
        "BucketLocked",
    );
}

#[test]
fn write_bounds_the_input() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let writer = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = open_bucket(&mut svm, &manager, ns, &admin, &writer);

    let long = "r".repeat(MAX_REFERENCE_LEN + 1);
    fails_with(
        &mut svm,
        write_ix(&writer.pubkey(), b, 0, msg_input(&long, None), None),
        &writer,
        "MetadataTooLong",
    );
}

#[test]
fn admins_create_tags() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = bucket(&mut svm, &manager, ns, &admin);

    fails_with(
        &mut svm,
        create_tag_ix(&manager.pubkey(), b, "deed"),
        &manager,
        "AccountNotInitialized",
    );
    fails_with(
        &mut svm,
        create_tag_ix(&admin.pubkey(), b, &"t".repeat(MAX_TAG_LEN + 1)),
        &admin,
        "MetadataTooLong",
    );
    ok(&mut svm, create_tag_ix(&admin.pubkey(), b, "deed"), &admin);
    let tag = tag_of(&svm, b, "deed");
    assert_eq!(tag.bucket_id, b);
    assert_eq!(tag.tag, "deed");
    assert_eq!(tag.message_count, 0);
    assert_eq!(tag.rent_payer, admin.pubkey());
    assert_eq!(bucket_of(&svm, b).tag_count, 1);
    let size = svm.get_account(&tag_pda(b, "deed")).unwrap().data.len();
    assert_eq!(size, Tag::space("deed"));

    fails_with(
        &mut svm,
        create_tag_ix(&admin.pubkey(), b, "deed"),
        &admin,
        "already in use",
    );
}

#[test]
fn tagged_writes_need_the_matching_tag() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let writer = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = open_bucket(&mut svm, &manager, ns, &admin, &writer);
    let other = open_bucket(&mut svm, &manager, ns, &admin, &writer);
    ok(&mut svm, create_tag_ix(&admin.pubkey(), b, "deed"), &admin);
    ok(&mut svm, create_tag_ix(&admin.pubkey(), b, "plan"), &admin);
    ok(
        &mut svm,
        create_tag_ix(&admin.pubkey(), other, "deed"),
        &admin,
    );

    // Named but not passed, passed but not named, a different tag, and the
    // same tag on another bucket.
    let cases = [
        (Some("deed"), None),
        (None, Some(tag_pda(b, "deed"))),
        (Some("deed"), Some(tag_pda(b, "plan"))),
        (Some("deed"), Some(tag_pda(other, "deed"))),
    ];
    for (named, passed) in cases {
        fails_with(
            &mut svm,
            write_ix(&writer.pubkey(), b, 0, msg_input("cid-1", named), passed),
            &writer,
            "WrongTag",
        );
    }

    ok(
        &mut svm,
        write_ix(
            &writer.pubkey(),
            b,
            0,
            msg_input("cid-1", Some("deed")),
            Some(tag_pda(b, "deed")),
        ),
        &writer,
    );
    assert_eq!(message_of(&svm, b, 0).tag.as_deref(), Some("deed"));
    assert_eq!(tag_of(&svm, b, "deed").message_count, 1);
    assert_eq!(tag_of(&svm, b, "plan").message_count, 0);
}
