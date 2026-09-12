//! Where the rent goes when a record is removed.

mod common;
use common::*;

#[test]
fn every_removal_refunds_its_recorded_payer() {
    let (mut svm, authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let writer = funded(&mut svm);
    let outsider = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = open_bucket(&mut svm, &manager, ns, &admin, &writer);
    ok(&mut svm, add_viewer_ix(&admin.pubkey(), b, KEY), &admin);
    ok(&mut svm, create_tag_ix(&admin.pubkey(), b, "deed"), &admin);
    ok(
        &mut svm,
        write_ix(&writer.pubkey(), b, 0, msg_input("cid-1", None), None),
        &writer,
    );

    // Every site pins the refund to the stored payer, so a caller cannot
    // send someone else's rent to themselves.
    let wrong = outsider.pubkey();
    let auth = authority.pubkey();
    let cases: Vec<(_, &Keypair)> = vec![
        (
            remove_admin_ix(&manager.pubkey(), ns, b, &admin.pubkey(), &wrong),
            &manager,
        ),
        (
            remove_contributor_ix(&admin.pubkey(), b, &writer.pubkey(), &wrong),
            &admin,
        ),
        (remove_viewer_ix(&admin.pubkey(), b, KEY, &wrong), &admin),
        (
            force_remove_message_ix(&auth, b, 0, &wrong, None),
            &authority,
        ),
        (force_remove_tag_ix(&auth, b, "deed", &wrong), &authority),
        (force_remove_bucket_ix(&auth, ns, b, &wrong), &authority),
        (force_remove_namespace_ix(&auth, ns, &wrong), &authority),
    ];
    for (ix, signer) in cases {
        fails_with(&mut svm, ix, signer, "WrongRentPayer");
    }
    assert!(exists(&svm, &message_pda(b, 0)));
    assert!(exists(&svm, &tag_pda(b, "deed")));
    assert!(exists(&svm, &viewer_pda(b, &KEY)));
}
