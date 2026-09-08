//! Pausing, resuming and rotating a bucket's key.

mod common;
use common::*;

#[test]
fn admins_open_and_pause_buckets() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = bucket(&mut svm, &manager, ns, &admin);

    // Managers hold no admin seat, so they cannot touch the key.
    fails_with(
        &mut svm,
        resume_ix(&manager.pubkey(), b, KEY),
        &manager,
        "AccountNotInitialized",
    );
    ok(&mut svm, resume_ix(&admin.pubkey(), b, KEY), &admin);
    assert_eq!(bucket_of(&svm, b).encryption_key, Some(KEY));

    ok(&mut svm, pause_ix(&admin.pubkey(), b), &admin);
    assert_eq!(bucket_of(&svm, b).encryption_key, None);
    // Pausing a paused bucket is a no-op, not an error.
    ok(&mut svm, pause_ix(&admin.pubkey(), b), &admin);
}

#[test]
fn rotate_needs_an_open_bucket() {
    let (mut svm, _authority) = setup();
    let manager = funded(&mut svm);
    let admin = funded(&mut svm);
    let ns = namespace(&mut svm, &manager);
    let b = bucket(&mut svm, &manager, ns, &admin);

    fails_with(
        &mut svm,
        rotate_ix(&admin.pubkey(), b, KEY2),
        &admin,
        "BucketLocked",
    );
    ok(&mut svm, resume_ix(&admin.pubkey(), b, KEY), &admin);
    ok(&mut svm, rotate_ix(&admin.pubkey(), b, KEY2), &admin);
    assert_eq!(bucket_of(&svm, b).encryption_key, Some(KEY2));
}
