//! Shared test scaffolding for the bucket program: PDA helpers, instruction
//! builders, the send/assert helpers (`ok`, `fails_with`), readers, and the
//! `setup` / `namespace` / `bucket` drivers. Each test file pulls this in
//! with `mod common; use common::*;`.
//!
//! Each test file is its own binary that uses a subset of this, so unused
//! helpers are expected.
#![allow(dead_code, unused_imports)]

pub use anchor_lang::prelude::Pubkey;
pub use anchor_lang::AccountDeserialize;
pub use bucket::state::{
    Bucket, BucketMetadata, Config, Namespace, NamespaceMetadata, Property, MAX_NAME_LEN,
    MAX_PROPERTIES,
};
pub use litesvm::LiteSVM;
pub use solana_keypair::Keypair;
pub use solana_signer::Signer;

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::{InstructionData, ToAccountMetas};
use bucket::{
    ADMIN_SEED, BUCKET_SEED, CONFIG_SEED, CONTRIBUTOR_SEED, MANAGER_SEED, NAMESPACE_SEED,
    VIEWER_SEED,
};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use solana_message::{Message as TxMessage, VersionedMessage};
use solana_transaction::versioned::VersionedTransaction;

pub const SYS: Pubkey = anchor_lang::system_program::ID;
pub const KEY: [u8; 32] = [7; 32];

// --- PDAs ---

pub fn pid() -> Pubkey {
    bucket::id()
}
pub fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &pid()).0
}
pub fn namespace_pda(id: u64) -> Pubkey {
    Pubkey::find_program_address(&[NAMESPACE_SEED, &id.to_le_bytes()], &pid()).0
}
pub fn manager_pda(namespace_id: u64, wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[MANAGER_SEED, &namespace_id.to_le_bytes(), wallet.as_ref()],
        &pid(),
    )
    .0
}
pub fn bucket_pda(id: u64) -> Pubkey {
    Pubkey::find_program_address(&[BUCKET_SEED, &id.to_le_bytes()], &pid()).0
}
pub fn admin_pda(bucket_id: u64, wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[ADMIN_SEED, &bucket_id.to_le_bytes(), wallet.as_ref()],
        &pid(),
    )
    .0
}
pub fn contributor_pda(bucket_id: u64, wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[CONTRIBUTOR_SEED, &bucket_id.to_le_bytes(), wallet.as_ref()],
        &pid(),
    )
    .0
}
pub fn viewer_pda(bucket_id: u64, key: &[u8; 32]) -> Pubkey {
    Pubkey::find_program_address(&[VIEWER_SEED, &bucket_id.to_le_bytes(), key], &pid()).0
}

// --- metadata ---

pub fn prop(key: &str, value: &str) -> Property {
    Property {
        key: key.into(),
        value: value.into(),
    }
}

pub fn ns_meta(name: &str) -> NamespaceMetadata {
    NamespaceMetadata {
        name: name.into(),
        schema_uri: Some("https://xcavate.io/schema/property".into()),
        properties: vec![prop("propertyId", "42")],
    }
}

pub fn bucket_meta(name: &str) -> BucketMetadata {
    BucketMetadata {
        name: name.into(),
        category: "legal".into(),
        properties: vec![],
    }
}

// --- instruction builders ---

pub fn program_data_pda() -> Pubkey {
    Pubkey::find_program_address(
        &[pid().as_ref()],
        &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
    )
    .0
}

// Point the program's upgrade authority at `authority` so the authority-bound
// initialize passes. The loader metadata is 4 bytes of enum tag, 8 of slot,
// then an optional pubkey.
pub fn bind_upgrade_authority(svm: &mut LiteSVM, authority: &Pubkey) {
    let pd = program_data_pda();
    let mut acc = svm.get_account(&pd).unwrap();
    acc.data[12] = 1;
    acc.data[13..45].copy_from_slice(authority.as_ref());
    svm.set_account(pd, acc).unwrap();
}

fn ix(data: Vec<u8>, accounts: impl ToAccountMetas) -> Instruction {
    Instruction::new_with_bytes(pid(), &data, accounts.to_account_metas(None))
}

pub fn init_ix(authority: &Pubkey) -> Instruction {
    ix(
        bucket::instruction::InitializeConfig {}.data(),
        bucket::accounts::InitializeConfig {
            authority: *authority,
            program: pid(),
            program_data: program_data_pda(),
            config: config_pda(),
            system_program: SYS,
        },
    )
}

pub fn update_authority_ix(authority: &Pubkey, new_authority: &Pubkey) -> Instruction {
    ix(
        bucket::instruction::UpdateAuthority {
            new_authority: *new_authority,
        }
        .data(),
        bucket::accounts::UpdateAuthority {
            authority: *authority,
            config: config_pda(),
        },
    )
}

pub fn accept_authority_ix(new_authority: &Pubkey) -> Instruction {
    ix(
        bucket::instruction::AcceptAuthority {}.data(),
        bucket::accounts::AcceptAuthority {
            new_authority: *new_authority,
            config: config_pda(),
        },
    )
}

pub fn create_namespace_ix(
    creator: &Pubkey,
    namespace_id: u64,
    metadata: NamespaceMetadata,
) -> Instruction {
    ix(
        bucket::instruction::CreateNamespace { metadata }.data(),
        bucket::accounts::CreateNamespace {
            creator: *creator,
            config: config_pda(),
            namespace: namespace_pda(namespace_id),
            manager: manager_pda(namespace_id, creator),
            system_program: SYS,
        },
    )
}

pub fn add_manager_ix(signer: &Pubkey, namespace_id: u64, new_manager: &Pubkey) -> Instruction {
    ix(
        bucket::instruction::AddManager {}.data(),
        bucket::accounts::AddManager {
            manager_signer: *signer,
            namespace: namespace_pda(namespace_id),
            manager: manager_pda(namespace_id, signer),
            new_manager: *new_manager,
            new_manager_account: manager_pda(namespace_id, new_manager),
            system_program: SYS,
        },
    )
}

pub fn remove_manager_ix(
    signer: &Pubkey,
    namespace_id: u64,
    target: &Pubkey,
    rent_payer: &Pubkey,
) -> Instruction {
    ix(
        bucket::instruction::RemoveManager {}.data(),
        bucket::accounts::RemoveManager {
            manager_signer: *signer,
            namespace: namespace_pda(namespace_id),
            manager: manager_pda(namespace_id, signer),
            rent_payer: *rent_payer,
            target: manager_pda(namespace_id, target),
        },
    )
}

pub fn create_bucket_ix(
    manager: &Pubkey,
    namespace_id: u64,
    bucket_id: u64,
    metadata: BucketMetadata,
) -> Instruction {
    ix(
        bucket::instruction::CreateBucket { metadata }.data(),
        bucket::accounts::CreateBucket {
            manager_signer: *manager,
            config: config_pda(),
            namespace: namespace_pda(namespace_id),
            manager: manager_pda(namespace_id, manager),
            bucket: bucket_pda(bucket_id),
            system_program: SYS,
        },
    )
}

pub fn add_admin_ix(
    manager: &Pubkey,
    namespace_id: u64,
    bucket_id: u64,
    new_admin: &Pubkey,
) -> Instruction {
    ix(
        bucket::instruction::AddAdmin {}.data(),
        bucket::accounts::AddAdmin {
            manager_signer: *manager,
            bucket: bucket_pda(bucket_id),
            manager: manager_pda(namespace_id, manager),
            new_admin: *new_admin,
            admin: admin_pda(bucket_id, new_admin),
            system_program: SYS,
        },
    )
}

pub fn remove_admin_ix(
    manager: &Pubkey,
    namespace_id: u64,
    bucket_id: u64,
    admin: &Pubkey,
    rent_payer: &Pubkey,
) -> Instruction {
    ix(
        bucket::instruction::RemoveAdmin {}.data(),
        bucket::accounts::RemoveAdmin {
            manager_signer: *manager,
            bucket: bucket_pda(bucket_id),
            manager: manager_pda(namespace_id, manager),
            rent_payer: *rent_payer,
            admin: admin_pda(bucket_id, admin),
        },
    )
}

pub fn add_contributor_ix(admin: &Pubkey, bucket_id: u64, new_contributor: &Pubkey) -> Instruction {
    ix(
        bucket::instruction::AddContributor {}.data(),
        bucket::accounts::AddContributor {
            admin_signer: *admin,
            bucket: bucket_pda(bucket_id),
            admin: admin_pda(bucket_id, admin),
            new_contributor: *new_contributor,
            contributor: contributor_pda(bucket_id, new_contributor),
            system_program: SYS,
        },
    )
}

pub fn remove_contributor_ix(
    admin: &Pubkey,
    bucket_id: u64,
    contributor: &Pubkey,
    rent_payer: &Pubkey,
) -> Instruction {
    ix(
        bucket::instruction::RemoveContributor {}.data(),
        bucket::accounts::RemoveContributor {
            admin_signer: *admin,
            bucket: bucket_pda(bucket_id),
            admin: admin_pda(bucket_id, admin),
            rent_payer: *rent_payer,
            contributor: contributor_pda(bucket_id, contributor),
        },
    )
}

pub fn add_viewer_ix(admin: &Pubkey, bucket_id: u64, viewer_key: [u8; 32]) -> Instruction {
    ix(
        bucket::instruction::AddViewer { viewer_key }.data(),
        bucket::accounts::AddViewer {
            admin_signer: *admin,
            bucket: bucket_pda(bucket_id),
            admin: admin_pda(bucket_id, admin),
            viewer: viewer_pda(bucket_id, &viewer_key),
            system_program: SYS,
        },
    )
}

pub fn remove_viewer_ix(
    admin: &Pubkey,
    bucket_id: u64,
    viewer_key: [u8; 32],
    rent_payer: &Pubkey,
) -> Instruction {
    ix(
        bucket::instruction::RemoveViewer { viewer_key }.data(),
        bucket::accounts::RemoveViewer {
            admin_signer: *admin,
            bucket: bucket_pda(bucket_id),
            admin: admin_pda(bucket_id, admin),
            rent_payer: *rent_payer,
            viewer: viewer_pda(bucket_id, &viewer_key),
        },
    )
}

// --- send / assert ---

pub fn process(
    svm: &mut LiteSVM,
    ix: Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
) -> Result<TransactionMetadata, FailedTransactionMetadata> {
    svm.expire_blockhash();
    let blockhash = svm.latest_blockhash();
    let msg = TxMessage::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    svm.send_transaction(tx)
}

pub fn ok(svm: &mut LiteSVM, ix: Instruction, signer: &Keypair) {
    if let Err(failed) = process(svm, ix, signer, &[signer]) {
        panic!("expected success, failed with: {:?}", failed.err);
    }
}

pub fn fails_with(svm: &mut LiteSVM, ix: Instruction, signer: &Keypair, expected: &str) {
    match process(svm, ix, signer, &[signer]) {
        Ok(_) => panic!("expected failure `{expected}`, but it succeeded"),
        Err(failed) => {
            let detail = format!("{:?}\n{}", failed.err, failed.meta.logs.join("\n"));
            assert!(
                detail.contains(expected),
                "expected `{expected}`, got:\n{detail}"
            );
        }
    }
}

/// A SOL-funded keypair (for fees + account rent).
pub fn funded(svm: &mut LiteSVM) -> Keypair {
    let kp = Keypair::new();
    svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    kp
}

pub fn lamports(svm: &LiteSVM, who: &Pubkey) -> u64 {
    svm.get_account(who).map(|a| a.lamports).unwrap_or(0)
}

pub fn exists(svm: &LiteSVM, key: &Pubkey) -> bool {
    svm.get_account(key).is_some_and(|a| !a.data.is_empty())
}

// --- readers ---

fn read<T: AccountDeserialize>(svm: &LiteSVM, key: &Pubkey) -> T {
    let acc = svm.get_account(key).expect("account missing");
    T::try_deserialize(&mut acc.data.as_slice()).unwrap()
}

pub fn config_of(svm: &LiteSVM) -> Config {
    read(svm, &config_pda())
}
pub fn namespace_of(svm: &LiteSVM, id: u64) -> Namespace {
    read(svm, &namespace_pda(id))
}
pub fn bucket_of(svm: &LiteSVM, id: u64) -> Bucket {
    read(svm, &bucket_pda(id))
}

// --- drivers ---

// Reads the program binary from target/deploy at runtime rather than via
// include_bytes!, so the test crate compiles on a fresh clone where the .so
// doesn't exist yet (anchor's IDL pass compiles tests too).
pub fn program_bytes() -> Vec<u8> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/bucket.so");
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{} missing, run `anchor build` first", path.display()))
}

/// Loads the program and initializes the config. Returns (svm, authority).
pub fn setup() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    svm.add_program(pid(), &program_bytes()).unwrap();
    let authority = funded(&mut svm);
    bind_upgrade_authority(&mut svm, &authority.pubkey());
    ok(&mut svm, init_ix(&authority.pubkey()), &authority);
    (svm, authority)
}

/// Creates a namespace managed by `creator`; returns its id.
pub fn namespace(svm: &mut LiteSVM, creator: &Keypair) -> u64 {
    let id = config_of(svm).next_namespace_id;
    ok(
        svm,
        create_namespace_ix(&creator.pubkey(), id, ns_meta("Property namespace")),
        creator,
    );
    id
}

/// Creates a bucket under `namespace_id` with `admin` seated; returns its id.
pub fn bucket(svm: &mut LiteSVM, manager: &Keypair, namespace_id: u64, admin: &Keypair) -> u64 {
    let id = config_of(svm).next_bucket_id;
    ok(
        svm,
        create_bucket_ix(&manager.pubkey(), namespace_id, id, bucket_meta("deeds")),
        manager,
    );
    ok(
        svm,
        add_admin_ix(&manager.pubkey(), namespace_id, id, &admin.pubkey()),
        manager,
    );
    id
}
