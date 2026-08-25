//! Shared test scaffolding for region governance: PDA and token helpers,
//! instruction builders, the send/assert helpers (`ok`, `fails_with`), and the
//! `setup` / `reach_*` drivers.
//!
//! Governance stakes are XCAV (a classic SPL token) held in the program vault.
//! LiteSVM loads the SPL Token program by default, and the XCAV mint and each
//! participant's token account are seeded directly with `set_account`. The roles
//! program is loaded alongside regions so the cross-program RegionalOperator gate
//! is exercised for real. Each test file pulls this in with
//! `mod common; use common::*;`.
//!
//! Each test file is its own binary that uses a subset of this, so unused
//! helpers are expected.
#![allow(dead_code, unused_imports)]

pub use anchor_lang::prelude::Pubkey;
pub use anchor_lang::solana_program::clock::Clock;
pub use anchor_lang::AccountDeserialize;
pub use litesvm::LiteSVM;
pub use regions::state::{
    Config as RegionsConfig, RegionProposal, RegionState, RegionStatus, Vote, VoteRecord,
};
pub use solana_keypair::Keypair;
pub use solana_signer::Signer;
pub use xcavate_whitelist::state::Role;

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program_option::COption;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token::state::{Account as SplAccount, AccountState, Mint as SplMint};
use anchor_spl::token::ID as TOKEN_PROGRAM_ID;
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
pub use solana_account::Account;
use solana_message::{Message, VersionedMessage};
use solana_transaction::versioned::VersionedTransaction;

use regions::instructions::ConfigParams;
use regions::{CONFIG_SEED, PROPOSAL_SEED, REGION_SEED, REGION_STATE_SEED, VAULT_SEED, VOTE_SEED};

pub const SYS: Pubkey = anchor_lang::system_program::ID;
pub const DEPOSIT: u64 = 1_000_000_000;
pub const DECIMALS: u8 = 9;
pub const FUND_XCAV: u64 = 100_000_000_000;
pub const LOCATION_DEPOSIT: u64 = 50_000_000;
pub const LISTING_DURATION: i64 = 100_000;
pub const TAX_BPS: u16 = 300;

// --- ids / PDAs ---

pub fn rid() -> Pubkey {
    regions::id()
}
pub fn roles_id() -> Pubkey {
    xcavate_whitelist::id()
}

pub fn regions_config() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &rid()).0
}
pub fn vault() -> Pubkey {
    Pubkey::find_program_address(&[VAULT_SEED], &rid()).0
}
pub fn region_state(region_id: u16) -> Pubkey {
    Pubkey::find_program_address(&[REGION_STATE_SEED, &region_id.to_le_bytes()], &rid()).0
}
pub fn proposal_pda(proposal_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[PROPOSAL_SEED, &proposal_id.to_le_bytes()], &rid()).0
}
pub fn vote_record_pda(proposal_id: u64, voter: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[VOTE_SEED, &proposal_id.to_le_bytes(), voter.as_ref()],
        &rid(),
    )
    .0
}
pub fn region_pda(region_id: u16) -> Pubkey {
    Pubkey::find_program_address(&[REGION_SEED, &region_id.to_le_bytes()], &rid()).0
}
pub fn location_pda(region_id: u16, postcode: &[u8]) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::LOCATION_SEED, &region_id.to_le_bytes(), postcode],
        &rid(),
    )
    .0
}

pub fn roles_config() -> Pubkey {
    Pubkey::find_program_address(&[xcavate_whitelist::CONFIG_SEED], &roles_id()).0
}
pub fn admin_pda(who: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[xcavate_whitelist::ADMIN_SEED, who.as_ref()], &roles_id()).0
}
pub fn role_pda(user: &Pubkey, role: Role) -> Pubkey {
    Pubkey::find_program_address(
        &[
            xcavate_whitelist::ROLE_SEED,
            user.as_ref(),
            &[role.seed_byte()],
        ],
        &roles_id(),
    )
    .0
}

// --- XCAV mint / token accounts (seeded directly) ---

/// Fixed address for the test XCAV mint.
pub fn xcav_mint() -> Pubkey {
    Pubkey::new_from_array([7u8; 32])
}

/// The owner's associated XCAV token account. A real ATA, since the refund
/// paths recreate recipients' accounts idempotently at the canonical address.
pub fn token_acc(owner: &Pubkey) -> Pubkey {
    anchor_spl::associated_token::get_associated_token_address(owner, &xcav_mint())
}

pub const ATA_PROGRAM_ID: Pubkey = anchor_spl::associated_token::ID;

pub fn set_mint(svm: &mut LiteSVM) {
    // Chosen so the 0.1% operator bond (supply / 1000) equals DEPOSIT.
    set_mint_supply(svm, DEPOSIT * 1_000);
}

/// Rewrite the XCAV mint's supply, so the live 0.1% bond can be moved between an
/// operator's original bond and a later renewal.
pub fn set_mint_supply(svm: &mut LiteSVM, supply: u64) {
    let mint = SplMint {
        mint_authority: COption::None,
        supply,
        decimals: DECIMALS,
        is_initialized: true,
        freeze_authority: COption::None,
    };
    let mut data = vec![0u8; SplMint::LEN];
    mint.pack_into_slice(&mut data);
    svm.set_account(
        xcav_mint(),
        Account {
            lamports: 100_000_000,
            data,
            owner: TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

pub fn set_token_account(svm: &mut LiteSVM, address: Pubkey, owner: &Pubkey, amount: u64) {
    let acc = SplAccount {
        mint: xcav_mint(),
        owner: *owner,
        amount,
        delegate: COption::None,
        state: AccountState::Initialized,
        is_native: COption::None,
        delegated_amount: 0,
        close_authority: COption::None,
    };
    let mut data = vec![0u8; SplAccount::LEN];
    acc.pack_into_slice(&mut data);
    svm.set_account(
        address,
        Account {
            lamports: 100_000_000,
            data,
            owner: TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

pub fn give_xcav(svm: &mut LiteSVM, owner: &Pubkey, amount: u64) {
    set_token_account(svm, token_acc(owner), owner, amount);
}

pub fn xcav_balance(svm: &LiteSVM, owner: &Pubkey) -> u64 {
    let acc = svm.get_account(&token_acc(owner)).unwrap();
    SplAccount::unpack(&acc.data).unwrap().amount
}

pub fn vault_balance(svm: &LiteSVM) -> u64 {
    let acc = svm.get_account(&vault()).unwrap();
    SplAccount::unpack(&acc.data).unwrap().amount
}

// --- send helpers ---

pub fn process(
    svm: &mut LiteSVM,
    ix: Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
) -> Result<TransactionMetadata, FailedTransactionMetadata> {
    svm.expire_blockhash();
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    svm.send_transaction(tx)
}

pub fn ok(svm: &mut LiteSVM, ix: Instruction, payer: &Keypair, signers: &[&Keypair]) {
    if let Err(failed) = process(svm, ix, payer, signers) {
        panic!("expected success, failed with: {:?}", failed.err);
    }
}

pub fn fails_with(
    svm: &mut LiteSVM,
    ix: Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
    expected: &str,
) {
    match process(svm, ix, payer, signers) {
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
    svm.airdrop(&kp.pubkey(), 100_000_000_000).unwrap();
    kp
}

/// A SOL-funded keypair that also holds XCAV.
pub fn actor(svm: &mut LiteSVM) -> Keypair {
    let kp = funded(svm);
    give_xcav(svm, &kp.pubkey(), FUND_XCAV);
    kp
}

// --- roles instruction builders ---

// The programdata account the upgradeable loader keeps beside each program.
pub fn program_data_pda(program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[program_id.as_ref()],
        &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
    )
    .0
}

// Point a deployed program's upgrade authority at `authority` so the
// authority-bound initialize passes. The loader metadata is 4 bytes of enum
// tag, 8 of slot, then an optional pubkey.
pub fn bind_upgrade_authority(svm: &mut LiteSVM, program_id: &Pubkey, authority: &Pubkey) {
    let pd = program_data_pda(program_id);
    let mut acc = svm.get_account(&pd).unwrap();
    acc.data[12] = 1;
    acc.data[13..45].copy_from_slice(authority.as_ref());
    svm.set_account(pd, acc).unwrap();
}

pub fn roles_init_ix(authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        roles_id(),
        &xcavate_whitelist::instruction::InitializeConfig {}.data(),
        xcavate_whitelist::accounts::InitializeConfig {
            authority: *authority,
            program: roles_id(),
            program_data: program_data_pda(&roles_id()),
            config: roles_config(),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn roles_add_admin_ix(authority: &Pubkey, new_admin: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        roles_id(),
        &xcavate_whitelist::instruction::AddAdmin {}.data(),
        xcavate_whitelist::accounts::AddAdmin {
            authority: *authority,
            config: roles_config(),
            new_admin: *new_admin,
            admin: admin_pda(new_admin),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn roles_remove_ix(
    admin: &Pubkey,
    user: &Pubkey,
    role: Role,
    rent_payer: &Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        roles_id(),
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

pub fn roles_assign_ix(admin: &Pubkey, user: &Pubkey, role: Role) -> Instruction {
    Instruction::new_with_bytes(
        roles_id(),
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

// --- regions instruction builders ---

pub fn default_params() -> ConfigParams {
    ConfigParams {
        minimum_voting_amount: 100_000_000,
        voting_period: 1_000,
        owner_change_period: 10_000,
        threshold_bps: 5_000,
        quorum: 100_000_000,
        notice_period: 5_000,
        min_vote_hold: 100,
        max_listing_duration: 1_000_000,
        max_tax_bps: 1_000,
        location_deposit: LOCATION_DEPOSIT,
    }
}

pub fn regions_init_ix(authority: &Pubkey) -> Instruction {
    regions_init_ix_with(authority, default_params())
}

pub fn regions_init_ix_with(authority: &Pubkey, params: ConfigParams) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::InitializeConfig { params }.data(),
        regions::accounts::InitializeConfig {
            authority: *authority,
            program: rid(),
            program_data: program_data_pda(&rid()),
            config: regions_config(),
            xcav_mint: xcav_mint(),
            vault: vault(),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn update_config_ix(authority: &Pubkey, params: ConfigParams) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::UpdateConfig { params }.data(),
        regions::accounts::UpdateConfig {
            authority: *authority,
            config: regions_config(),
        }
        .to_account_metas(None),
    )
}

pub fn update_authority_ix(authority: &Pubkey, new_authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::UpdateAuthority {
            new_authority: *new_authority,
        }
        .data(),
        regions::accounts::UpdateAuthority {
            authority: *authority,
            config: regions_config(),
        }
        .to_account_metas(None),
    )
}

pub fn propose_ix(proposer: &Pubkey, region_id: u16, proposal_id: u64) -> Instruction {
    propose_ix_capped(proposer, region_id, proposal_id, u64::MAX)
}

pub fn propose_ix_capped(
    proposer: &Pubkey,
    region_id: u16,
    proposal_id: u64,
    max_deposit: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::ProposeNewRegion {
            region_id,
            max_deposit,
        }
        .data(),
        regions::accounts::ProposeNewRegion {
            proposer: *proposer,
            config: regions_config(),
            xcav_mint: xcav_mint(),
            proposer_token: token_acc(proposer),
            vault: vault(),
            operator_role: role_pda(proposer, Role::RegionalOperator),
            region: region_pda(region_id),
            region_state: region_state(region_id),
            proposal: proposal_pda(proposal_id),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn vote_ix(
    voter: &Pubkey,
    region_id: u16,
    proposal_id: u64,
    vote: Vote,
    amount: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::VoteOnRegionProposal {
            region_id,
            vote,
            amount,
        }
        .data(),
        regions::accounts::VoteOnRegionProposal {
            voter: *voter,
            config: regions_config(),
            xcav_mint: xcav_mint(),
            voter_token: token_acc(voter),
            vault: vault(),
            region_state: region_state(region_id),
            proposal: proposal_pda(proposal_id),
            vote_record: vote_record_pda(proposal_id, voter),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn finalize_ix(
    cranker: &Pubkey,
    region_id: u16,
    proposal_id: u64,
    proposer: &Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::FinalizeRegionProposal { region_id }.data(),
        regions::accounts::FinalizeRegionProposal {
            cranker: *cranker,
            config: regions_config(),
            xcav_mint: xcav_mint(),
            vault: vault(),
            region_state: region_state(region_id),
            proposal: proposal_pda(proposal_id),
            proposer: *proposer,
            proposer_token: token_acc(proposer),
            token_program: TOKEN_PROGRAM_ID,
            associated_token_program: ATA_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// Claim a passed region (the proposer creates it) with the default listing
/// duration and tax.
pub fn create_region_ix(creator: &Pubkey, region_id: u16) -> Instruction {
    create_region_ix_with(creator, region_id, LISTING_DURATION, TAX_BPS)
}

pub fn create_region_ix_with(
    creator: &Pubkey,
    region_id: u16,
    listing_duration: i64,
    tax_bps: u16,
) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::CreateRegion {
            region_id,
            listing_duration,
            tax_bps,
        }
        .data(),
        regions::accounts::CreateRegion {
            creator: *creator,
            config: regions_config(),
            creator_role: role_pda(creator, Role::RegionalOperator),
            region_state: region_state(region_id),
            region: region_pda(region_id),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// Take over an open region seat as a different operator, bonding 0.1% of XCAV
/// supply and refunding the outgoing operator.
pub fn claim_open_region_ix(
    new_operator: &Pubkey,
    region_id: u16,
    old_owner: &Pubkey,
) -> Instruction {
    claim_open_region_ix_capped(new_operator, region_id, old_owner, u64::MAX)
}

pub fn claim_open_region_ix_capped(
    new_operator: &Pubkey,
    region_id: u16,
    old_owner: &Pubkey,
    max_deposit: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::ClaimOpenRegion {
            region_id,
            max_deposit,
        }
        .data(),
        regions::accounts::ClaimOpenRegion {
            new_operator: *new_operator,
            config: regions_config(),
            operator_role: role_pda(new_operator, Role::RegionalOperator),
            xcav_mint: xcav_mint(),
            new_operator_token: token_acc(new_operator),
            vault: vault(),
            region: region_pda(region_id),
            old_owner: *old_owner,
            old_owner_token: token_acc(old_owner),
            token_program: TOKEN_PROGRAM_ID,
            associated_token_program: ATA_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// Renew the incumbent's own open seat (only the bond difference moves).
pub fn renew_region_ix(operator: &Pubkey, region_id: u16) -> Instruction {
    claim_open_region_ix(operator, region_id, operator)
}

pub fn unlock_ix(voter: &Pubkey, proposal_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::UnlockVotingToken { proposal_id }.data(),
        regions::accounts::UnlockVotingToken {
            voter: *voter,
            config: regions_config(),
            xcav_mint: xcav_mint(),
            voter_token: token_acc(voter),
            vault: vault(),
            vote_record: vote_record_pda(proposal_id, voter),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
    )
}

pub fn clear_ix(cranker: &Pubkey, region_id: u16, proposer: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::ClearRegionState { region_id }.data(),
        regions::accounts::ClearRegionState {
            cranker: *cranker,
            config: regions_config(),
            xcav_mint: xcav_mint(),
            vault: vault(),
            region_state: region_state(region_id),
            proposer: *proposer,
            proposer_token: token_acc(proposer),
            token_program: TOKEN_PROGRAM_ID,
            associated_token_program: ATA_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// Push the clock past the proposal voting window so finalize can run.
pub fn warp_past_voting(svm: &mut LiteSVM) {
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += 2_000;
    svm.set_sysvar(&clock);
}

// Reads a program binary from target/deploy at runtime rather than via
// include_bytes!, so the test crates compile on a fresh clone where the .so
// files don't exist yet (anchor's IDL pass compiles tests too).
pub fn program_bytes(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/deploy")
        .join(format!("{name}.so"));
    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{} missing, run `anchor build` first", path.display()))
}

// Loads both programs, seeds the XCAV mint, wires up a compliant
// RegionalOperator (with XCAV), and initializes the regions config. Returns
// (svm, operator, authority).
pub fn setup() -> (LiteSVM, Keypair, Keypair) {
    let mut svm = LiteSVM::new();
    svm.add_program(roles_id(), &program_bytes("xcavate_whitelist"))
        .unwrap();
    svm.add_program(rid(), &program_bytes("regions")).unwrap();
    set_mint(&mut svm);

    let authority = funded(&mut svm);
    bind_upgrade_authority(&mut svm, &roles_id(), &authority.pubkey());
    bind_upgrade_authority(&mut svm, &rid(), &authority.pubkey());
    ok(
        &mut svm,
        roles_init_ix(&authority.pubkey()),
        &authority,
        &[&authority],
    );

    let admin = funded(&mut svm);
    ok(
        &mut svm,
        roles_add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );

    let operator = actor(&mut svm);
    ok(
        &mut svm,
        roles_assign_ix(&admin.pubkey(), &operator.pubkey(), Role::RegionalOperator),
        &admin,
        &[&admin],
    );

    ok(
        &mut svm,
        regions_init_ix(&authority.pubkey()),
        &authority,
        &[&authority],
    );
    (svm, operator, authority)
}

pub fn next_proposal_id(svm: &LiteSVM) -> u64 {
    let acc = svm.get_account(&regions_config()).unwrap();
    RegionsConfig::try_deserialize(&mut &acc.data[..])
        .unwrap()
        .proposal_counter
}

/// Reads who paid a role account's rent (the assigning admin), which any
/// teardown must name as the refund destination.
pub fn role_rent_payer(svm: &LiteSVM, user: &Pubkey, role: Role) -> Pubkey {
    xcavate_whitelist::state::RoleAccount::try_deserialize(
        &mut &svm.get_account(&role_pda(user, role)).unwrap().data[..],
    )
    .unwrap()
    .rent_payer
}

// Adds a second compliant RegionalOperator (with XCAV).
pub fn new_operator(svm: &mut LiteSVM, authority: &Keypair) -> Keypair {
    let admin = funded(svm);
    ok(
        svm,
        roles_add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        authority,
        &[authority],
    );
    let op = actor(svm);
    ok(
        svm,
        roles_assign_ix(&admin.pubkey(), &op.pubkey(), Role::RegionalOperator),
        &admin,
        &[&admin],
    );
    op
}

// Drives region 1 to the Passed state (proposal claimable) and returns the id.
pub fn reach_passed(svm: &mut LiteSVM, operator: &Keypair, _authority: &Keypair) -> u64 {
    let id = next_proposal_id(svm);
    ok(
        svm,
        propose_ix(&operator.pubkey(), 1, id),
        operator,
        &[operator],
    );
    let voter = actor(svm);
    ok(
        svm,
        vote_ix(&voter.pubkey(), 1, id, Vote::Yes, 200_000_000),
        &voter,
        &[&voter],
    );
    warp_past_voting(svm);
    let cranker = funded(svm);
    ok(
        svm,
        finalize_ix(&cranker.pubkey(), 1, id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    id
}

pub fn region_state_of(svm: &LiteSVM, region_id: u16) -> RegionState {
    RegionState::try_deserialize(&mut &svm.get_account(&region_state(region_id)).unwrap().data[..])
        .unwrap()
}

// --- created regions / seat turnover ---

pub fn region_of(svm: &LiteSVM, region_id: u16) -> regions::state::Region {
    regions::state::Region::try_deserialize(
        &mut &svm.get_account(&region_pda(region_id)).unwrap().data[..],
    )
    .unwrap()
}

/// Advance the clock by `secs` seconds.
pub fn warp(svm: &mut LiteSVM, secs: i64) {
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += secs;
    svm.set_sysvar(&clock);
}

/// Drives region 1 all the way to a created region owned by `operator` with a
/// 600M collateral.
pub fn reach_created(svm: &mut LiteSVM, operator: &Keypair, authority: &Keypair) {
    reach_passed(svm, operator, authority);
    // The proposer claims the passed region; the bond (DEPOSIT) becomes its
    // collateral.
    ok(
        svm,
        create_region_ix(&operator.pubkey(), 1),
        operator,
        &[operator],
    );
}

/// Drives region 1 to a created region, then opens the seat via the operator's
/// resignation and warping past the notice period (5_000).
pub fn reach_seat_open(svm: &mut LiteSVM, operator: &Keypair, authority: &Keypair) {
    reach_created(svm, operator, authority);
    ok(svm, resign_ix(&operator.pubkey(), 1), operator, &[operator]);
    warp(svm, 6_000);
}

pub fn resign_ix(operator: &Pubkey, region_id: u16) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::InitiateResignation { region_id }.data(),
        regions::accounts::InitiateResignation {
            operator: *operator,
            config: regions_config(),
            region: region_pda(region_id),
        }
        .to_account_metas(None),
    )
}

// --- authority handover ---

pub fn accept_authority_ix(new_authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::AcceptAuthority {}.data(),
        regions::accounts::AcceptAuthority {
            new_authority: *new_authority,
            config: regions_config(),
        }
        .to_account_metas(None),
    )
}

pub fn create_location_ix(operator: &Pubkey, region_id: u16, postcode: &[u8]) -> Instruction {
    create_location_ix_capped(operator, region_id, postcode, u64::MAX)
}

pub fn create_location_ix_capped(
    operator: &Pubkey,
    region_id: u16,
    postcode: &[u8],
    max_deposit: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::CreateNewLocation {
            region_id,
            postcode: postcode.to_vec(),
            max_deposit,
        }
        .data(),
        regions::accounts::CreateNewLocation {
            operator: *operator,
            config: regions_config(),
            operator_role: role_pda(operator, Role::RegionalOperator),
            xcav_mint: xcav_mint(),
            operator_token: token_acc(operator),
            vault: vault(),
            region: region_pda(region_id),
            location: location_pda(region_id, postcode),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn remove_location_ix(operator: &Pubkey, region_id: u16, postcode: &[u8]) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::RemoveLocation {
            region_id,
            postcode: postcode.to_vec(),
        }
        .data(),
        regions::accounts::RemoveLocation {
            operator: *operator,
            config: regions_config(),
            operator_role: role_pda(operator, Role::RegionalOperator),
            xcav_mint: xcav_mint(),
            operator_token: token_acc(operator),
            vault: vault(),
            region: region_pda(region_id),
            location: location_pda(region_id, postcode),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
    )
}

pub fn adjust_duration_ix(operator: &Pubkey, region_id: u16, listing_duration: i64) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::AdjustListingDuration {
            region_id,
            listing_duration,
        }
        .data(),
        adjust_region_accounts(operator, region_id),
    )
}

pub fn adjust_tax_ix(operator: &Pubkey, region_id: u16, tax_bps: u16) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::AdjustRegionTax { region_id, tax_bps }.data(),
        adjust_region_accounts(operator, region_id),
    )
}

fn adjust_region_accounts(
    operator: &Pubkey,
    region_id: u16,
) -> Vec<anchor_lang::solana_program::instruction::AccountMeta> {
    regions::accounts::AdjustRegion {
        operator: *operator,
        config: regions_config(),
        operator_role: role_pda(operator, Role::RegionalOperator),
        region: region_pda(region_id),
    }
    .to_account_metas(None)
}

// --- extra role helpers ---

/// Block a wallet's compliance record through the whitelist program, to prove
/// the regions gates don't consult it.
pub fn block_compliance(svm: &mut LiteSVM, authority: &Keypair, user: &Pubkey) {
    let admin = funded(svm);
    ok(
        svm,
        roles_add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        authority,
        &[authority],
    );
    let compliance = Pubkey::find_program_address(
        &[xcavate_whitelist::COMPLIANCE_SEED, user.as_ref()],
        &roles_id(),
    )
    .0;
    let ix = Instruction::new_with_bytes(
        roles_id(),
        &xcavate_whitelist::instruction::SetCompliance {
            status: xcavate_whitelist::state::ComplianceStatus::Blocked,
            expires_at: 0,
        }
        .data(),
        xcavate_whitelist::accounts::SetCompliance {
            admin_signer: admin.pubkey(),
            admin: admin_pda(&admin.pubkey()),
            user: *user,
            compliance,
            system_program: SYS,
        }
        .to_account_metas(None),
    );
    ok(svm, ix, &admin, &[&admin]);
}
