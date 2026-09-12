//! Random op walks over the whole program, cross-checking the accounts
//! against each other after every step: each counter matches the live
//! records it counts, nothing outlives its parent, and every lamport of
//! rent is either back in a wallet or held by a live record.

mod common;
use common::*;

use anchor_lang::solana_program::instruction::Instruction;
use proptest::prelude::*;

const ACTORS: usize = 6;
const VIEWER_KEYS: [[u8; 32]; 3] = [[1; 32], [2; 32], [3; 32]];
const TAGS: [&str; 3] = ["deed", "plan", "report"];
/// Also bounds the message ids a bucket can have seen.
const MAX_OPS: usize = 80;
const FEE: u64 = 5_000;

/// The `i`th live id, wrapping; `None` when there is nothing to aim at.
fn aim(ids: &[u64], i: usize) -> Option<u64> {
    if ids.is_empty() {
        None
    } else {
        Some(ids[i % ids.len()])
    }
}

fn live_namespaces(svm: &LiteSVM) -> Vec<u64> {
    (0..config_of(svm).next_namespace_id)
        .filter(|&id| exists(svm, &namespace_pda(id)))
        .collect()
}

fn live_buckets(svm: &LiteSVM) -> Vec<u64> {
    (0..config_of(svm).next_bucket_id)
        .filter(|&id| exists(svm, &bucket_pda(id)))
        .collect()
}

fn live_messages(svm: &LiteSVM, bucket_id: u64) -> Vec<u64> {
    (0..bucket_of(svm, bucket_id).next_message_id)
        .filter(|&id| exists(svm, &message_pda(bucket_id, id)))
        .collect()
}

/// Actor indices holding the seat `pda` derives.
fn holders(svm: &LiteSVM, actors: &[Keypair], pda: impl Fn(&Pubkey) -> Pubkey) -> Vec<usize> {
    (0..actors.len())
        .filter(|&i| exists(svm, &pda(&actors[i].pubkey())))
        .collect()
}

fn wallets(svm: &LiteSVM, actors: &[Keypair]) -> u64 {
    actors.iter().map(|a| lamports(svm, &a.pubkey())).sum()
}

/// Cross-checks every record against its counters and returns the rent
/// held by all live records.
fn check(svm: &LiteSVM, actors: &[Keypair]) -> Result<u64, TestCaseError> {
    let config = config_of(svm);
    let mut rent = 0;

    for id in 0..config.next_namespace_id {
        if !exists(svm, &namespace_pda(id)) {
            for a in actors {
                prop_assert!(!exists(svm, &manager_pda(id, &a.pubkey())));
            }
            continue;
        }
        rent += lamports(svm, &namespace_pda(id));
        let ns = namespace_of(svm, id);
        let managers = holders(svm, actors, |w| manager_pda(id, w));
        for &i in &managers {
            rent += lamports(svm, &manager_pda(id, &actors[i].pubkey()));
        }
        prop_assert_eq!(ns.manager_count as usize, managers.len());
        let buckets = live_buckets(svm)
            .into_iter()
            .filter(|&b| bucket_of(svm, b).namespace_id == id)
            .count();
        prop_assert_eq!(ns.bucket_count as usize, buckets);
    }

    for id in 0..config.next_bucket_id {
        if !exists(svm, &bucket_pda(id)) {
            for a in actors {
                prop_assert!(!exists(svm, &admin_pda(id, &a.pubkey())));
                prop_assert!(!exists(svm, &contributor_pda(id, &a.pubkey())));
            }
            for key in &VIEWER_KEYS {
                prop_assert!(!exists(svm, &viewer_pda(id, key)));
            }
            for tag in TAGS {
                prop_assert!(!exists(svm, &tag_pda(id, tag)));
            }
            for m in 0..MAX_OPS as u64 {
                prop_assert!(!exists(svm, &message_pda(id, m)));
            }
            continue;
        }
        rent += lamports(svm, &bucket_pda(id));
        let bucket = bucket_of(svm, id);
        prop_assert!(exists(svm, &namespace_pda(bucket.namespace_id)));

        let admins = holders(svm, actors, |w| admin_pda(id, w));
        let contributors = holders(svm, actors, |w| contributor_pda(id, w));
        for &i in &admins {
            rent += lamports(svm, &admin_pda(id, &actors[i].pubkey()));
        }
        for &i in &contributors {
            rent += lamports(svm, &contributor_pda(id, &actors[i].pubkey()));
        }
        let mut viewers = 0;
        for key in &VIEWER_KEYS {
            if exists(svm, &viewer_pda(id, key)) {
                viewers += 1;
                rent += lamports(svm, &viewer_pda(id, key));
            }
        }
        let mut tags = Vec::new();
        for tag in TAGS {
            if exists(svm, &tag_pda(id, tag)) {
                tags.push(tag);
                rent += lamports(svm, &tag_pda(id, tag));
            }
        }
        prop_assert_eq!(bucket.admin_count as usize, admins.len());
        prop_assert_eq!(bucket.contributor_count as usize, contributors.len());
        prop_assert_eq!(bucket.viewer_count as usize, viewers);
        prop_assert_eq!(bucket.tag_count as usize, tags.len());

        let mut tagged = [0u64; 3];
        let messages = live_messages(svm, id);
        for &m in &messages {
            rent += lamports(svm, &message_pda(id, m));
            let message = message_of(svm, id, m);
            prop_assert_eq!(message.bucket_id, id);
            if let Some(tag) = message.tag.as_deref() {
                // A tag cannot go while a message still carries it.
                let slot = TAGS.iter().position(|t| *t == tag).unwrap();
                prop_assert!(tags.contains(&tag));
                tagged[slot] += 1;
            }
        }
        prop_assert_eq!(bucket.message_count as usize, messages.len());
        for (slot, tag) in TAGS.iter().enumerate() {
            if tags.contains(tag) {
                prop_assert_eq!(tag_of(svm, id, tag).message_count, tagged[slot]);
            }
        }
    }
    Ok(rent)
}

/// Turns one random tuple into an instruction aimed at live records. The
/// signer is a holder of the needed seat, or under `wild` any actor, so
/// most ops land while wrong-role rejections still happen. `None` means
/// there was nothing to aim at.
fn build(
    svm: &LiteSVM,
    actors: &[Keypair],
    kind: u8,
    a: usize,
    b: usize,
    n: usize,
    wild: bool,
) -> Option<(Instruction, usize)> {
    let key = |i: usize| actors[i].pubkey();
    let pick = |ids: &[u64]| aim(ids, n);
    let seated = |holders: Vec<usize>| {
        if wild || holders.is_empty() {
            a
        } else {
            holders[a % holders.len()]
        }
    };
    let target = |holders: Vec<usize>| {
        if wild || holders.is_empty() {
            b
        } else {
            holders[b % holders.len()]
        }
    };
    let managers = |ns: u64| holders(svm, actors, |w| manager_pda(ns, w));
    let admins = |bk: u64| holders(svm, actors, |w| admin_pda(bk, w));
    let contributors = |bk: u64| holders(svm, actors, |w| contributor_pda(bk, w));
    // Force ops come from the authority unless the walk goes wild.
    let auth = if wild { a } else { 0 };
    let namespaces = live_namespaces(svm);
    let buckets = live_buckets(svm);

    let op = match kind {
        0 => (
            create_namespace_ix(&key(a), config_of(svm).next_namespace_id, ns_meta("walk")),
            a,
        ),
        1 => {
            let ns = pick(&namespaces)?;
            let s = seated(managers(ns));
            (add_manager_ix(&key(s), ns, &key(b)), s)
        }
        2 => {
            let ns = pick(&namespaces)?;
            let s = seated(managers(ns));
            let t = target(managers(ns));
            let payer = if exists(svm, &manager_pda(ns, &key(t))) {
                manager_of(svm, ns, &key(t)).rent_payer
            } else {
                key(t)
            };
            (remove_manager_ix(&key(s), ns, &key(t), &payer), s)
        }
        3 => {
            let ns = pick(&namespaces)?;
            let s = seated(managers(ns));
            let id = config_of(svm).next_bucket_id;
            (create_bucket_ix(&key(s), ns, id, bucket_meta("walk")), s)
        }
        4 => {
            let bk = pick(&buckets)?;
            let ns = bucket_of(svm, bk).namespace_id;
            let s = seated(managers(ns));
            (add_admin_ix(&key(s), ns, bk, &key(b)), s)
        }
        5 => {
            let bk = pick(&buckets)?;
            let ns = bucket_of(svm, bk).namespace_id;
            let s = seated(managers(ns));
            let t = target(admins(bk));
            let payer = if exists(svm, &admin_pda(bk, &key(t))) {
                admin_of(svm, bk, &key(t)).rent_payer
            } else {
                key(t)
            };
            (remove_admin_ix(&key(s), ns, bk, &key(t), &payer), s)
        }
        6 => {
            let bk = pick(&buckets)?;
            let s = seated(admins(bk));
            (add_contributor_ix(&key(s), bk, &key(b)), s)
        }
        7 => {
            let bk = pick(&buckets)?;
            let s = seated(admins(bk));
            let t = target(contributors(bk));
            let payer = if exists(svm, &contributor_pda(bk, &key(t))) {
                contributor_of(svm, bk, &key(t)).rent_payer
            } else {
                key(t)
            };
            (remove_contributor_ix(&key(s), bk, &key(t), &payer), s)
        }
        8 => {
            let bk = pick(&buckets)?;
            let s = seated(admins(bk));
            (add_viewer_ix(&key(s), bk, VIEWER_KEYS[b % 3]), s)
        }
        9 => {
            let bk = pick(&buckets)?;
            let s = seated(admins(bk));
            let vk = VIEWER_KEYS[b % 3];
            let payer = if exists(svm, &viewer_pda(bk, &vk)) {
                viewer_of(svm, bk, &vk).rent_payer
            } else {
                key(b)
            };
            (remove_viewer_ix(&key(s), bk, vk, &payer), s)
        }
        10 => {
            let bk = pick(&buckets)?;
            let s = seated(admins(bk));
            (pause_ix(&key(s), bk), s)
        }
        11 => {
            let bk = pick(&buckets)?;
            let s = seated(admins(bk));
            (resume_ix(&key(s), bk, KEY), s)
        }
        12 => {
            let bk = pick(&buckets)?;
            let s = seated(admins(bk));
            (rotate_ix(&key(s), bk, KEY2), s)
        }
        13 => {
            let bk = pick(&buckets)?;
            let s = seated(admins(bk));
            (create_tag_ix(&key(s), bk, TAGS[b % 3]), s)
        }
        14 => {
            let bk = pick(&buckets)?;
            let s = seated(contributors(bk));
            let tag = TAGS[b % 3];
            // Untagged, tagged with the account, or tagged without it.
            let (named, passed) = match n % 3 {
                0 => (None, None),
                1 => (Some(tag), Some(tag_pda(bk, tag))),
                _ => (Some(tag), None),
            };
            let id = bucket_of(svm, bk).next_message_id;
            let input = msg_input(&format!("cid-{bk}-{id}"), named);
            (write_ix(&key(s), bk, id, input, passed), s)
        }
        15 => {
            let bk = pick(&buckets)?;
            let m = aim(&live_messages(svm, bk), b)?;
            let message = message_of(svm, bk, m);
            let tag = message
                .tag
                .as_deref()
                .filter(|_| !wild)
                .map(|t| tag_pda(bk, t));
            (
                force_remove_message_ix(&key(auth), bk, m, &message.contributor, tag),
                auth,
            )
        }
        16 => {
            let bk = pick(&buckets)?;
            let tag = TAGS[b % 3];
            let payer = if exists(svm, &tag_pda(bk, tag)) {
                tag_of(svm, bk, tag).rent_payer
            } else {
                key(b)
            };
            (force_remove_tag_ix(&key(auth), bk, tag, &payer), auth)
        }
        17 => {
            let bk = pick(&buckets)?;
            let bucket = bucket_of(svm, bk);
            (
                force_remove_bucket_ix(&key(auth), bucket.namespace_id, bk, &bucket.rent_payer),
                auth,
            )
        }
        18 => {
            let ns = pick(&namespaces)?;
            let payer = namespace_of(svm, ns).rent_payer;
            (force_remove_namespace_ix(&key(auth), ns, &payer), auth)
        }
        19 => {
            let ns = pick(&namespaces)?;
            (force_add_manager_ix(&key(auth), ns, &key(b)), auth)
        }
        _ => {
            let ns = pick(&namespaces)?;
            let t = target(managers(ns));
            let payer = if exists(svm, &manager_pda(ns, &key(t))) {
                manager_of(svm, ns, &key(t)).rent_payer
            } else {
                key(t)
            };
            (
                force_remove_manager_ix(&key(auth), ns, &key(t), &payer),
                auth,
            )
        }
    };
    Some(op)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]
    #[test]
    fn random_walks_keep_the_books(
        ops in proptest::collection::vec(
            (0u8..21u8, 0usize..ACTORS, 0usize..ACTORS, 0usize..8usize, 0u8..4u8),
            30..MAX_OPS,
        )
    ) {
        let (mut svm, authority) = setup();
        // Actor 0 is the authority; the rest take whatever seats the walk hands out.
        let mut actors = vec![authority];
        for _ in 1..ACTORS {
            actors.push(funded(&mut svm));
        }
        // A full tree up front, so deep ops have a target even in walks
        // whose random prefix never builds one.
        let ns = namespace(&mut svm, &actors[1]);
        let b = open_bucket(&mut svm, &actors[1], ns, &actors[2], &actors[3]);
        ok(&mut svm, create_tag_ix(&actors[2].pubkey(), b, "deed"), &actors[2]);
        let input = msg_input("cid-0", Some("deed"));
        ok(
            &mut svm,
            write_ix(&actors[3].pubkey(), b, 0, input, Some(tag_pda(b, "deed"))),
            &actors[3],
        );
        let mut total = wallets(&svm, &actors) + check(&svm, &actors)?;

        for (kind, a, b, n, wild) in ops {
            let Some((ix, signer)) = build(&svm, &actors, kind, a, b, n, wild == 0) else {
                continue;
            };
            let landed = process(&mut svm, ix, &actors[signer], &[&actors[signer]]).is_ok();
            let now = wallets(&svm, &actors) + check(&svm, &actors)?;
            // Rent only moves between wallets and live records: a landed op
            // costs exactly its fee, a rejected one at most that.
            if landed {
                prop_assert_eq!(now, total - FEE);
            } else {
                prop_assert!(now == total || now == total - FEE);
            }
            total = now;
        }
    }
}
