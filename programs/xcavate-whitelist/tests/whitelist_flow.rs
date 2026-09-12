//! Behavioural tests for the roles & compliance registry.
//!
//! The program does no token transfers, so LiteSVM covers every path end to
//! end, and nothing here needs a Surfpool integration run.

use anchor_lang::{
    prelude::Pubkey, solana_program::instruction::Instruction, AccountDeserialize, InstructionData,
    ToAccountMetas,
};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use xcavate_whitelist::state::{Admin, Compliance, ComplianceStatus, Config, Role, RoleAccount};
use xcavate_whitelist::{ADMIN_SEED, COMPLIANCE_SEED, CONFIG_SEED, ROLE_SEED};

const SYS: Pubkey = anchor_lang::system_program::ID;

// Reads the program binary from target/deploy at runtime rather than via
// include_bytes!, so the test crate compiles on a fresh clone where the .so
// doesn't exist yet (anchor's IDL pass compiles tests too).
fn program_bytes() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/deploy/xcavate_whitelist.so");
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{} missing, run `anchor build` first", path.display()))
}

// --- PDA helpers ---

fn pid() -> Pubkey {
    xcavate_whitelist::id()
}

fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &pid()).0
}

fn admin_pda(who: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[ADMIN_SEED, who.as_ref()], &pid()).0
}

fn role_pda(user: &Pubkey, role: Role) -> Pubkey {
    Pubkey::find_program_address(&[ROLE_SEED, user.as_ref(), &[role.seed_byte()]], &pid()).0
}

fn compliance_pda(user: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[COMPLIANCE_SEED, user.as_ref()], &pid()).0
}

// --- instruction builders ---

// The programdata account the upgradeable loader keeps beside the program.
fn program_data_pda() -> Pubkey {
    Pubkey::find_program_address(
        &[pid().as_ref()],
        &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
    )
    .0
}

// Point the program's upgrade authority at `authority` so the authority-bound
// initialize passes. The loader metadata is 4 bytes of enum tag, 8 of slot,
// then an optional pubkey.
fn bind_upgrade_authority(svm: &mut LiteSVM, authority: &Pubkey) {
    let pd = program_data_pda();
    let mut acc = svm.get_account(&pd).unwrap();
    acc.data[12] = 1;
    acc.data[13..45].copy_from_slice(authority.as_ref());
    svm.set_account(pd, acc).unwrap();
}

fn init_ix(authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::InitializeConfig {}.data(),
        xcavate_whitelist::accounts::InitializeConfig {
            authority: *authority,
            program: pid(),
            program_data: program_data_pda(),
            config: config_pda(),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

fn add_admin_ix(authority: &Pubkey, new_admin: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::AddAdmin {}.data(),
        xcavate_whitelist::accounts::AddAdmin {
            authority: *authority,
            config: config_pda(),
            new_admin: *new_admin,
            admin: admin_pda(new_admin),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

fn remove_admin_ix(authority: &Pubkey, target: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::RemoveAdmin { admin_key: *target }.data(),
        xcavate_whitelist::accounts::RemoveAdmin {
            authority: *authority,
            config: config_pda(),
            admin: admin_pda(target),
        }
        .to_account_metas(None),
    )
}

fn assign_ix(admin: &Pubkey, user: &Pubkey, role: Role) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::AssignRole { role }.data(),
        xcavate_whitelist::accounts::AssignRole {
            admin_signer: *admin,
            admin: admin_pda(admin),
            user: *user,
            role_account: role_pda(user, role),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

fn remove_role_ix(admin: &Pubkey, user: &Pubkey, role: Role, rent_payer: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::RemoveRole { role }.data(),
        xcavate_whitelist::accounts::RemoveRole {
            admin_signer: *admin,
            admin: admin_pda(admin),
            user: *user,
            rent_payer: *rent_payer,
            role_account: role_pda(user, role),
        }
        .to_account_metas(None),
    )
}

fn renounce_ix(user: &Pubkey, rent_payer: &Pubkey, role: Role) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::RenounceRole { role }.data(),
        xcavate_whitelist::accounts::RenounceRole {
            user: *user,
            rent_payer: *rent_payer,
            role_account: role_pda(user, role),
        }
        .to_account_metas(None),
    )
}

fn update_authority_ix(authority: &Pubkey, new_authority: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::UpdateAuthority { new_authority }.data(),
        xcavate_whitelist::accounts::UpdateAuthority {
            authority: *authority,
            config: config_pda(),
        }
        .to_account_metas(None),
    )
}

fn accept_authority_ix(new_authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::AcceptAuthority {}.data(),
        xcavate_whitelist::accounts::AcceptAuthority {
            new_authority: *new_authority,
            config: config_pda(),
        }
        .to_account_metas(None),
    )
}

// --- send helpers ---

fn process(
    svm: &mut LiteSVM,
    ix: Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
) -> Result<TransactionMetadata, FailedTransactionMetadata> {
    // Fresh blockhash per send: otherwise two identical instructions (e.g. a
    // double-assign) hash to the same signature and the runtime rejects the
    // retry as `AlreadyProcessed` before the program ever runs.
    svm.expire_blockhash();
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    svm.send_transaction(tx)
}

fn ok(svm: &mut LiteSVM, ix: Instruction, payer: &Keypair, signers: &[&Keypair]) {
    if let Err(failed) = process(svm, ix, payer, signers) {
        panic!("expected tx to succeed, failed with: {:?}", failed.err);
    }
}

/// Assert the tx fails AND that the Anchor error matches `expected` (matched
/// against the program logs, e.g. "NotAuthority", "AccountNotInitialized").
fn fails_with(
    svm: &mut LiteSVM,
    ix: Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
    expected: &str,
) {
    match process(svm, ix, payer, signers) {
        Ok(_) => panic!("expected tx to fail with `{expected}`, but it succeeded"),
        Err(failed) => {
            let detail = format!("{:?}\n{}", failed.err, failed.meta.logs.join("\n"));
            assert!(
                detail.contains(expected),
                "expected error `{expected}`, got:\n{detail}",
            );
        }
    }
}

fn funded(svm: &mut LiteSVM) -> Keypair {
    let kp = Keypair::new();
    svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    kp
}

// Fresh SVM with the program loaded and an initialized config whose sudo is
// `authority`.
fn setup() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    svm.add_program(pid(), &program_bytes()).unwrap();
    let authority = funded(&mut svm);
    bind_upgrade_authority(&mut svm, &authority.pubkey());
    ok(
        &mut svm,
        init_ix(&authority.pubkey()),
        &authority,
        &[&authority],
    );
    (svm, authority)
}

// As above, plus one registered admin.
fn setup_with_admin() -> (LiteSVM, Keypair, Keypair) {
    let (mut svm, authority) = setup();
    let admin = funded(&mut svm);
    ok(
        &mut svm,
        add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );
    (svm, authority, admin)
}

fn read_config(svm: &LiteSVM) -> Config {
    let acc = svm.get_account(&config_pda()).unwrap();
    Config::try_deserialize(&mut &acc.data[..]).unwrap()
}

fn read_role(svm: &LiteSVM, user: &Pubkey, role: Role) -> RoleAccount {
    let acc = svm.get_account(&role_pda(user, role)).unwrap();
    RoleAccount::try_deserialize(&mut &acc.data[..]).unwrap()
}

// ============================ add_admin ============================

#[test]
fn add_admin_works() {
    let (mut svm, authority) = setup();
    let admin = Keypair::new().pubkey();
    ok(
        &mut svm,
        add_admin_ix(&authority.pubkey(), &admin),
        &authority,
        &[&authority],
    );

    let acc = svm.get_account(&admin_pda(&admin)).unwrap();
    let parsed = Admin::try_deserialize(&mut &acc.data[..]).unwrap();
    assert_eq!(parsed.admin, admin);
}

#[test]
fn add_admin_fails_for_non_authority() {
    let (mut svm, _authority) = setup();
    let imposter = funded(&mut svm);
    let admin = Keypair::new().pubkey();
    fails_with(
        &mut svm,
        add_admin_ix(&imposter.pubkey(), &admin),
        &imposter,
        &[&imposter],
        "NotAuthority",
    );
}

#[test]
fn add_admin_fails_when_already_admin() {
    let (mut svm, authority, admin) = setup_with_admin();
    // Re-registering the same admin hits the `init` reinit guard.
    fails_with(
        &mut svm,
        add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
        "already in use",
    );
}

// ============================ remove_admin ============================

#[test]
fn remove_admin_works() {
    let (mut svm, authority, admin) = setup_with_admin();
    ok(
        &mut svm,
        remove_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );
    assert!(svm
        .get_account(&admin_pda(&admin.pubkey()))
        .is_none_or(|a| a.data.is_empty()));
}

#[test]
fn remove_admin_fails_for_non_authority() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let imposter = funded(&mut svm);
    fails_with(
        &mut svm,
        remove_admin_ix(&imposter.pubkey(), &admin.pubkey()),
        &imposter,
        &[&imposter],
        "NotAuthority",
    );
}

#[test]
fn remove_admin_fails_when_not_admin() {
    let (mut svm, authority) = setup();
    let never_admin = Keypair::new().pubkey();
    fails_with(
        &mut svm,
        remove_admin_ix(&authority.pubkey(), &never_admin),
        &authority,
        &[&authority],
        "AccountNotInitialized",
    );
}

// ============================ assign_role ============================

#[test]
fn assign_role_works() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = Keypair::new().pubkey();
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RealEstateDeveloper),
        &admin,
        &[&admin],
    );

    let parsed = read_role(&svm, &user, Role::RealEstateDeveloper);
    assert_eq!(parsed.user, user);
    assert_eq!(parsed.role, Role::RealEstateDeveloper);
    // A role that was never granted has no account.
    assert!(svm
        .get_account(&role_pda(&user, Role::LettingAgent))
        .is_none_or(|a| a.data.is_empty()));
}

#[test]
fn assign_role_fails_when_already_assigned() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = Keypair::new().pubkey();
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::LettingAgent),
        &admin,
        &[&admin],
    );
    fails_with(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::LettingAgent),
        &admin,
        &[&admin],
        "already in use",
    );
}

#[test]
fn assign_role_fails_for_non_admin() {
    let (mut svm, _authority) = setup();
    let imposter = funded(&mut svm);
    let user = Keypair::new().pubkey();
    fails_with(
        &mut svm,
        assign_ix(&imposter.pubkey(), &user, Role::LettingAgent),
        &imposter,
        &[&imposter],
        "AccountNotInitialized",
    );
}

// ============================ remove_role ============================

#[test]
fn remove_role_works() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = Keypair::new().pubkey();
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RealEstateInvestor),
        &admin,
        &[&admin],
    );
    ok(
        &mut svm,
        remove_role_ix(
            &admin.pubkey(),
            &user,
            Role::RealEstateInvestor,
            &admin.pubkey(),
        ),
        &admin,
        &[&admin],
    );
    assert!(svm
        .get_account(&role_pda(&user, Role::RealEstateInvestor))
        .is_none_or(|a| a.data.is_empty()));
}

#[test]
fn remove_role_fails_for_non_admin() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = Keypair::new().pubkey();
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RealEstateInvestor),
        &admin,
        &[&admin],
    );
    let imposter = funded(&mut svm);
    fails_with(
        &mut svm,
        remove_role_ix(
            &imposter.pubkey(),
            &user,
            Role::RealEstateInvestor,
            &admin.pubkey(),
        ),
        &imposter,
        &[&imposter],
        "AccountNotInitialized",
    );
}

#[test]
fn remove_role_fails_when_not_assigned() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = Keypair::new().pubkey();
    fails_with(
        &mut svm,
        remove_role_ix(
            &admin.pubkey(),
            &user,
            Role::RealEstateInvestor,
            &admin.pubkey(),
        ),
        &admin,
        &[&admin],
        "AccountNotInitialized",
    );
}

// ============================ renounce_role ============================

#[test]
fn renounce_role_works() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = funded(&mut svm);
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user.pubkey(), Role::Lawyer),
        &admin,
        &[&admin],
    );

    // The holder gives the role up themselves; the rent goes back to the
    // admin who paid it at assignment, not the holder.
    let user_before = svm.get_account(&user.pubkey()).unwrap().lamports;
    let admin_before = svm.get_account(&admin.pubkey()).unwrap().lamports;
    ok(
        &mut svm,
        renounce_ix(&user.pubkey(), &admin.pubkey(), Role::Lawyer),
        &user,
        &[&user],
    );
    assert!(svm
        .get_account(&role_pda(&user.pubkey(), Role::Lawyer))
        .is_none_or(|a| a.data.is_empty()));
    assert!(svm.get_account(&admin.pubkey()).unwrap().lamports > admin_before);
    assert!(svm.get_account(&user.pubkey()).unwrap().lamports <= user_before);
}

#[test]
fn renounce_role_fails_when_not_assigned() {
    let (mut svm, authority, _admin) = setup_with_admin();
    let user = funded(&mut svm);
    fails_with(
        &mut svm,
        renounce_ix(
            &user.pubkey(),
            &authority.pubkey(),
            Role::RealEstateInvestor,
        ),
        &user,
        &[&user],
        "AccountNotInitialized",
    );
}

#[test]
fn renounce_role_cannot_target_another_user() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let victim = Keypair::new().pubkey();
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &victim, Role::RealEstateInvestor),
        &admin,
        &[&admin],
    );

    // The role account seed is bound to the signer, so pointing the
    // instruction at someone else's role account cannot derive.
    let attacker = funded(&mut svm);
    let ix = Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::RenounceRole {
            role: Role::RealEstateInvestor,
        }
        .data(),
        xcavate_whitelist::accounts::RenounceRole {
            user: attacker.pubkey(),
            rent_payer: admin.pubkey(),
            role_account: role_pda(&victim, Role::RealEstateInvestor),
        }
        .to_account_metas(None),
    );
    fails_with(&mut svm, ix, &attacker, &[&attacker], "ConstraintSeeds");
}

#[test]
fn renounce_role_rejects_wrong_rent_destination() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = funded(&mut svm);
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user.pubkey(), Role::Lawyer),
        &admin,
        &[&admin],
    );

    // Redirecting the rent anywhere but the recorded payer is refused.
    fails_with(
        &mut svm,
        renounce_ix(&user.pubkey(), &user.pubkey(), Role::Lawyer),
        &user,
        &[&user],
        "WrongRentPayer",
    );
}

// ============================ update_authority ============================

#[test]
fn update_authority_is_two_step() {
    let (mut svm, authority) = setup();
    let new_authority = funded(&mut svm);

    // Proposing alone hands over nothing: the current authority stays in
    // power and the proposal is only recorded as pending.
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), new_authority.pubkey()),
        &authority,
        &[&authority],
    );
    let config = read_config(&svm);
    assert_eq!(config.authority, authority.pubkey());
    assert_eq!(config.pending_authority, Some(new_authority.pubkey()));
    let admin = Keypair::new().pubkey();
    ok(
        &mut svm,
        add_admin_ix(&authority.pubkey(), &admin),
        &authority,
        &[&authority],
    );

    // Accepting completes the handover; power switches atomically.
    ok(
        &mut svm,
        accept_authority_ix(&new_authority.pubkey()),
        &new_authority,
        &[&new_authority],
    );
    let config = read_config(&svm);
    assert_eq!(config.authority, new_authority.pubkey());
    assert_eq!(config.pending_authority, None);
    let admin2 = Keypair::new().pubkey();
    fails_with(
        &mut svm,
        add_admin_ix(&authority.pubkey(), &admin2),
        &authority,
        &[&authority],
        "NotAuthority",
    );
    ok(
        &mut svm,
        add_admin_ix(&new_authority.pubkey(), &admin2),
        &new_authority,
        &[&new_authority],
    );
}

#[test]
fn accept_authority_fails_without_matching_proposal() {
    let (mut svm, authority) = setup();

    // Nothing pending yet.
    let imposter = funded(&mut svm);
    fails_with(
        &mut svm,
        accept_authority_ix(&imposter.pubkey()),
        &imposter,
        &[&imposter],
        "NotPendingAuthority",
    );

    // A proposal for someone else doesn't let a third party accept.
    let proposed = funded(&mut svm);
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), proposed.pubkey()),
        &authority,
        &[&authority],
    );
    fails_with(
        &mut svm,
        accept_authority_ix(&imposter.pubkey()),
        &imposter,
        &[&imposter],
        "NotPendingAuthority",
    );
}

#[test]
fn update_authority_reproposal_overwrites_pending() {
    let (mut svm, authority) = setup();
    let first = funded(&mut svm);
    let second = funded(&mut svm);
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), first.pubkey()),
        &authority,
        &[&authority],
    );
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), second.pubkey()),
        &authority,
        &[&authority],
    );

    // The stale proposal is dead; only the latest can accept.
    fails_with(
        &mut svm,
        accept_authority_ix(&first.pubkey()),
        &first,
        &[&first],
        "NotPendingAuthority",
    );
    ok(
        &mut svm,
        accept_authority_ix(&second.pubkey()),
        &second,
        &[&second],
    );
    assert_eq!(read_config(&svm).authority, second.pubkey());
}

#[test]
fn update_authority_fails_for_non_authority() {
    let (mut svm, _authority) = setup();
    let imposter = funded(&mut svm);
    fails_with(
        &mut svm,
        update_authority_ix(&imposter.pubkey(), imposter.pubkey()),
        &imposter,
        &[&imposter],
        "NotAuthority",
    );
}

#[test]
fn update_authority_fails_for_zero_address() {
    let (mut svm, authority) = setup();
    // Handing the sudo authority to the zero address would brick the registry.
    fails_with(
        &mut svm,
        update_authority_ix(&authority.pubkey(), Pubkey::default()),
        &authority,
        &[&authority],
        "InvalidAuthority",
    );
}

// ============================ singleton / role isolation ============================

#[test]
fn initialize_config_fails_on_double_init() {
    let (mut svm, authority) = setup();
    // The config is a singleton PDA; a second init hits the existing account.
    fails_with(
        &mut svm,
        init_ix(&authority.pubkey()),
        &authority,
        &[&authority],
        "already in use",
    );
}

#[test]
fn assign_multiple_distinct_roles_coexist() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = Keypair::new().pubkey();

    // Two different roles for one user live in independent PDAs (seed-byte isolation).
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RegionalOperator),
        &admin,
        &[&admin],
    );
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RealEstateDeveloper),
        &admin,
        &[&admin],
    );

    assert_eq!(
        read_role(&svm, &user, Role::RegionalOperator).role,
        Role::RegionalOperator
    );
    assert_eq!(
        read_role(&svm, &user, Role::RealEstateDeveloper).role,
        Role::RealEstateDeveloper
    );
}

#[test]
fn reassign_role_after_removal() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = Keypair::new().pubkey();

    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RealEstateDeveloper),
        &admin,
        &[&admin],
    );
    ok(
        &mut svm,
        remove_role_ix(
            &admin.pubkey(),
            &user,
            Role::RealEstateDeveloper,
            &admin.pubkey(),
        ),
        &admin,
        &[&admin],
    );
    // The PDA was closed; assigning again must re-create it cleanly.
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RealEstateDeveloper),
        &admin,
        &[&admin],
    );
    assert_eq!(
        read_role(&svm, &user, Role::RealEstateDeveloper).role,
        Role::RealEstateDeveloper
    );
}

#[test]
fn removed_admin_loses_power() {
    let (mut svm, authority, admin) = setup_with_admin();
    let user = Keypair::new().pubkey();

    ok(
        &mut svm,
        remove_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );
    // With the Admin PDA closed, the ex-admin's account no longer resolves.
    fails_with(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RealEstateDeveloper),
        &admin,
        &[&admin],
        "AccountNotInitialized",
    );
}

// ============================ initialize gating ============================

#[test]
fn initialize_config_requires_upgrade_authority() {
    let mut svm = LiteSVM::new();
    svm.add_program(pid(), &program_bytes()).unwrap();
    let deployer = funded(&mut svm);
    bind_upgrade_authority(&mut svm, &deployer.pubkey());

    // Someone other than the deployer cannot claim the config.
    let imposter = funded(&mut svm);
    fails_with(
        &mut svm,
        init_ix(&imposter.pubkey()),
        &imposter,
        &[&imposter],
        "NotUpgradeAuthority",
    );

    // The deployer can.
    ok(
        &mut svm,
        init_ix(&deployer.pubkey()),
        &deployer,
        &[&deployer],
    );
}

#[test]
fn initialize_config_rejects_spoofed_program_data() {
    // The program<->program_data binding is what stops an attacker supplying a
    // fabricated ProgramData that names themselves as upgrade authority to seize
    // the singleton config the other programs trust cross-program.
    let mut svm = LiteSVM::new();
    svm.add_program(pid(), &program_bytes()).unwrap();

    // Forge a well-formed ProgramData at a foreign address whose upgrade
    // authority is the imposter, so it clears the authority-equals-signer check.
    let imposter = funded(&mut svm);
    let fake_pd = Pubkey::new_unique();
    let mut acc = svm.get_account(&program_data_pda()).unwrap();
    acc.data[12] = 1;
    acc.data[13..45].copy_from_slice(imposter.pubkey().as_ref());
    svm.set_account(fake_pd, acc).unwrap();

    // It isn't THIS program's data account, so the binding rejects it even though
    // its recorded authority matches the signer.
    let ix = Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::InitializeConfig {}.data(),
        xcavate_whitelist::accounts::InitializeConfig {
            authority: imposter.pubkey(),
            program: pid(),
            program_data: fake_pd,
            config: config_pda(),
            system_program: SYS,
        }
        .to_account_metas(None),
    );
    fails_with(&mut svm, ix, &imposter, &[&imposter], "NotUpgradeAuthority");
}

// ============================ seed-byte stability ============================

#[test]
fn role_seed_bytes_are_stable() {
    // PDA derivations depend on these exact bytes; a reorder of the enum must
    // never change them. If this test fails, existing on-chain RoleAccounts
    // would become unreachable.
    assert_eq!(Role::RegionalOperator.seed_byte(), 0);
    assert_eq!(Role::RealEstateInvestor.seed_byte(), 1);
    assert_eq!(Role::RealEstateDeveloper.seed_byte(), 2);
    assert_eq!(Role::Lawyer.seed_byte(), 3);
    assert_eq!(Role::LettingAgent.seed_byte(), 4);
    assert_eq!(Role::SpvConfirmation.seed_byte(), 5);
}

#[test]
fn readd_admin_after_removal() {
    let (mut svm, authority, admin) = setup_with_admin();
    ok(
        &mut svm,
        remove_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );
    // The Admin PDA was closed; registering again must re-create it cleanly.
    ok(
        &mut svm,
        add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );
    let user = Keypair::new().pubkey();
    ok(
        &mut svm,
        assign_ix(&admin.pubkey(), &user, Role::RealEstateInvestor),
        &admin,
        &[&admin],
    );
}

// ============================ compliance ============================

fn set_compliance_ix(
    admin: &Pubkey,
    user: &Pubkey,
    status: ComplianceStatus,
    expires_at: i64,
) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::SetCompliance { status, expires_at }.data(),
        xcavate_whitelist::accounts::SetCompliance {
            admin_signer: *admin,
            admin: admin_pda(admin),
            user: *user,
            compliance: compliance_pda(user),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

fn remove_compliance_ix(admin: &Pubkey, user: &Pubkey, rent_payer: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &xcavate_whitelist::instruction::RemoveCompliance {}.data(),
        xcavate_whitelist::accounts::RemoveCompliance {
            admin_signer: *admin,
            admin: admin_pda(admin),
            user: *user,
            rent_payer: *rent_payer,
            compliance: compliance_pda(user),
        }
        .to_account_metas(None),
    )
}

fn read_compliance(svm: &LiteSVM, user: &Pubkey) -> Compliance {
    let account = svm.get_account(&compliance_pda(user)).unwrap();
    Compliance::try_deserialize(&mut account.data.as_slice()).unwrap()
}

fn now(svm: &LiteSVM) -> i64 {
    svm.get_sysvar::<anchor_lang::solana_program::clock::Clock>()
        .unix_timestamp
}

#[test]
fn set_compliance_creates_then_renews() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = funded(&mut svm).pubkey();
    let first = now(&svm) + 1_000;

    ok(
        &mut svm,
        set_compliance_ix(&admin.pubkey(), &user, ComplianceStatus::Cleared, first),
        &admin,
        &[&admin],
    );
    let record = read_compliance(&svm, &user);
    assert_eq!(record.user, user);
    assert_eq!(record.status, ComplianceStatus::Cleared);
    assert_eq!(record.expires_at, first);
    assert_eq!(record.rent_payer, admin.pubkey());

    // Re-screening reuses the account rather than needing a new one.
    let renewed = now(&svm) + 5_000;
    ok(
        &mut svm,
        set_compliance_ix(&admin.pubkey(), &user, ComplianceStatus::Cleared, renewed),
        &admin,
        &[&admin],
    );
    assert_eq!(read_compliance(&svm, &user).expires_at, renewed);
}

// A second admin renewing must not become the rent destination, or removal
// would refund the wrong wallet.
#[test]
fn renewal_keeps_the_original_rent_payer() {
    let (mut svm, authority, admin) = setup_with_admin();
    let user = funded(&mut svm).pubkey();
    let other = funded(&mut svm);
    ok(
        &mut svm,
        add_admin_ix(&authority.pubkey(), &other.pubkey()),
        &authority,
        &[&authority],
    );

    ok(
        &mut svm,
        set_compliance_ix(&admin.pubkey(), &user, ComplianceStatus::Cleared, 0),
        &admin,
        &[&admin],
    );
    ok(
        &mut svm,
        set_compliance_ix(&other.pubkey(), &user, ComplianceStatus::Blocked, 0),
        &other,
        &[&other],
    );

    let record = read_compliance(&svm, &user);
    assert_eq!(record.status, ComplianceStatus::Blocked);
    assert_eq!(record.rent_payer, admin.pubkey());
}

// An already-expired clearance is never what the caller meant, and would sit
// on file looking like a screening that had been done.
#[test]
fn cleared_rejects_an_expiry_in_the_past() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = funded(&mut svm).pubkey();
    let past = now(&svm) - 1;

    fails_with(
        &mut svm,
        set_compliance_ix(&admin.pubkey(), &user, ComplianceStatus::Cleared, past),
        &admin,
        &[&admin],
        "InvalidExpiry",
    );
}

// The liveness check is strict, so a clearance expiring right now would be
// dead on arrival.
#[test]
fn cleared_rejects_an_expiry_of_now() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = funded(&mut svm).pubkey();
    // Move off the epoch, where "now" is the no-expiry sentinel 0.
    let mut clock = svm.get_sysvar::<anchor_lang::solana_program::clock::Clock>();
    clock.unix_timestamp = 1_000;
    svm.set_sysvar(&clock);

    fails_with(
        &mut svm,
        set_compliance_ix(&admin.pubkey(), &user, ComplianceStatus::Cleared, 1_000),
        &admin,
        &[&admin],
        "InvalidExpiry",
    );
}

// Blocking does not lapse, so an expiry on it would be a contradiction.
#[test]
fn blocked_rejects_an_expiry() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = funded(&mut svm).pubkey();
    let future = now(&svm) + 1_000;

    fails_with(
        &mut svm,
        set_compliance_ix(&admin.pubkey(), &user, ComplianceStatus::Blocked, future),
        &admin,
        &[&admin],
        "InvalidExpiry",
    );
}

#[test]
fn set_compliance_is_admin_only() {
    let (mut svm, _authority, _admin) = setup_with_admin();
    let imposter = funded(&mut svm);
    let user = funded(&mut svm).pubkey();

    fails_with(
        &mut svm,
        set_compliance_ix(&imposter.pubkey(), &user, ComplianceStatus::Cleared, 0),
        &imposter,
        &[&imposter],
        "AccountNotInitialized",
    );
}

#[test]
fn remove_compliance_refunds_the_recorded_payer() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = funded(&mut svm).pubkey();
    ok(
        &mut svm,
        set_compliance_ix(&admin.pubkey(), &user, ComplianceStatus::Cleared, 0),
        &admin,
        &[&admin],
    );

    let before = svm.get_account(&admin.pubkey()).unwrap().lamports;
    ok(
        &mut svm,
        remove_compliance_ix(&admin.pubkey(), &user, &admin.pubkey()),
        &admin,
        &[&admin],
    );
    assert!(svm
        .get_account(&compliance_pda(&user))
        .is_none_or(|a| a.data.is_empty()));
    assert!(svm.get_account(&admin.pubkey()).unwrap().lamports > before);
}

#[test]
fn remove_compliance_rejects_a_foreign_rent_payer() {
    let (mut svm, _authority, admin) = setup_with_admin();
    let user = funded(&mut svm).pubkey();
    let thief = funded(&mut svm).pubkey();
    ok(
        &mut svm,
        set_compliance_ix(&admin.pubkey(), &user, ComplianceStatus::Cleared, 0),
        &admin,
        &[&admin],
    );

    fails_with(
        &mut svm,
        remove_compliance_ix(&admin.pubkey(), &user, &thief),
        &admin,
        &[&admin],
        "WrongRentPayer",
    );
}
