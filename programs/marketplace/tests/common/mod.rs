//! Shared test scaffolding for the marketplace: PDA and token helpers,
//! instruction builders, the send/assert helpers (`ok`, `fails_with`), and
//! `setup`. Deposits are XCAV (a classic SPL token) held in the program vault;
//! LiteSVM loads the SPL Token program by default, and the XCAV mint and each
//! participant's token account are seeded directly with `set_account`. Each
//! test file pulls this in with `mod common; use common::*;`.
//!
//! Each test file is its own binary that uses a subset of this, so unused
//! helpers are expected.
#![allow(dead_code, unused_imports)]

pub use anchor_lang::prelude::Pubkey;
pub use anchor_lang::solana_program::clock::Clock;
pub use anchor_lang::AccountDeserialize;
pub use litesvm::LiteSVM;
pub use marketplace::state::Config as MarketplaceConfig;
pub use marketplace::state::ListingStatus;
pub use solana_keypair::Keypair;
pub use solana_signer::Signer;
pub use xcavate_whitelist::state::{ComplianceStatus, Role};

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program_option::COption;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token::state::{Account as SplAccount, AccountState, Mint as SplMint};
use anchor_spl::token::ID as TOKEN_PROGRAM_ID;
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use solana_account::Account;
use solana_message::{Message, VersionedMessage};
use solana_transaction::versioned::VersionedTransaction;

use anchor_lang::{AnchorSerialize, Discriminator};
use marketplace::instructions::ConfigParams;
use marketplace::{
    CONFIG_SEED, LAWYER_SEED, LISTING_SEED, MINT_AUTH_SEED, PROPERTY_SEED, PROPERTY_VAULT_SEED,
    SHARE_MINT_SEED, VAULT_SEED,
};

pub const SYS: Pubkey = anchor_lang::system_program::ID;
pub const DECIMALS: u8 = 9;
pub const FUND_XCAV: u64 = 100_000_000_000;
pub const LISTING_DEPOSIT: u64 = 1_000_000_000;
pub const LAWYER_DEPOSIT: u64 = 500_000_000;
pub const SHARE_PRICE: u64 = 5_000_000_000;
pub const SHARE_AMOUNT: u32 = 100;
/// Matches the `listing_duration` that `seed_region` writes.
pub const LISTING_DURATION: i64 = 100_000;
pub const CLAIMING_TIME: i64 = 50_000;
pub const POSTCODE: &[u8] = b"SW1A1AA";

// --- ids / PDAs ---

pub fn mid() -> Pubkey {
    marketplace::id()
}
pub fn roles_id() -> Pubkey {
    xcavate_whitelist::id()
}

pub fn marketplace_config() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &mid()).0
}
pub fn vault() -> Pubkey {
    Pubkey::find_program_address(&[VAULT_SEED], &mid()).0
}

pub fn lawyer_pda(wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[LAWYER_SEED, wallet.as_ref()], &mid()).0
}
pub fn region_pda(region_id: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::REGION_SEED, &region_id.to_le_bytes()],
        &regions::id(),
    )
    .0
}

pub fn property_pda(asset_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[PROPERTY_SEED, &asset_id.to_le_bytes()], &mid()).0
}
pub fn listing_pda(listing_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[LISTING_SEED, &listing_id.to_le_bytes()], &mid()).0
}
pub fn location_pda(region_id: u16, postcode: &[u8]) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::LOCATION_SEED, &region_id.to_le_bytes(), postcode],
        &regions::id(),
    )
    .0
}
pub fn share_mint_pda(asset_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[SHARE_MINT_SEED, &asset_id.to_le_bytes()], &mid()).0
}
pub fn mint_auth_pda(asset_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[MINT_AUTH_SEED, &asset_id.to_le_bytes()], &mid()).0
}
pub fn property_vault_pda(asset_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[PROPERTY_VAULT_SEED, &asset_id.to_le_bytes()], &mid()).0
}
/// The property vault's associated token account for the property's share mint
/// (Token-2022 derivation).
pub fn vault_share_account(asset_id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            property_vault_pda(asset_id).as_ref(),
            anchor_spl::token_2022::ID.as_ref(),
            share_mint_pda(asset_id).as_ref(),
        ],
        &anchor_spl::associated_token::ID,
    )
    .0
}

pub fn position_pda(listing_id: u64, investor: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            marketplace::POSITION_SEED,
            &listing_id.to_le_bytes(),
            investor.as_ref(),
        ],
        &mid(),
    )
    .0
}
pub fn holding_pda(asset_id: u64, owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            marketplace::SHARE_SEED,
            &asset_id.to_le_bytes(),
            owner.as_ref(),
        ],
        &mid(),
    )
    .0
}
pub fn listing_vault_pda(listing_id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[marketplace::LISTING_VAULT_SEED, &listing_id.to_le_bytes()],
        &mid(),
    )
    .0
}
/// The listing vault's associated tGBP account (classic-token derivation).
pub fn listing_payment_ata(listing_id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            listing_vault_pda(listing_id).as_ref(),
            TOKEN_PROGRAM_ID.as_ref(),
            tgbp_mint().as_ref(),
        ],
        &anchor_spl::associated_token::ID,
    )
    .0
}
/// An investor's associated share account (Token-2022 derivation).
pub fn investor_share_ata(asset_id: u64, investor: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            investor.as_ref(),
            anchor_spl::token_2022::ID.as_ref(),
            share_mint_pda(asset_id).as_ref(),
        ],
        &anchor_spl::associated_token::ID,
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

/// The wallet's compliance record, in the roles program.
pub fn compliance_pda(user: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[xcavate_whitelist::COMPLIANCE_SEED, user.as_ref()],
        &roles_id(),
    )
    .0
}

fn set_compliance_ix(
    admin: &Pubkey,
    user: &Pubkey,
    status: ComplianceStatus,
    expires_at: i64,
) -> Instruction {
    Instruction::new_with_bytes(
        roles_id(),
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

/// Clear a wallet, or block it. Goes through the program rather than seeding
/// the account, so the tests exercise the real screening path.
pub fn set_compliance(svm: &mut LiteSVM, admin: &Keypair, user: &Pubkey, cleared: bool) {
    let status = if cleared {
        ComplianceStatus::Cleared
    } else {
        ComplianceStatus::Blocked
    };
    let ix = set_compliance_ix(&admin.pubkey(), user, status, 0);
    ok(svm, ix, admin, &[admin]);
}

/// Clear a wallet until `expires_at`, for the lapse cases.
pub fn clear_compliance_until(svm: &mut LiteSVM, admin: &Keypair, user: &Pubkey, expires_at: i64) {
    let ix = set_compliance_ix(&admin.pubkey(), user, ComplianceStatus::Cleared, expires_at);
    ok(svm, ix, admin, &[admin]);
}

// --- XCAV mint / token accounts (seeded directly) ---

/// Fixed address for the test XCAV mint.
pub fn xcav_mint() -> Pubkey {
    Pubkey::new_from_array([7u8; 32])
}

/// Fixed address for the test tGBP payment mint.
pub fn tgbp_mint() -> Pubkey {
    Pubkey::new_from_array([6u8; 32])
}

/// A second accepted GBP stablecoin at 6 decimals, so the price-rescaling
/// paths run at a real factor in tests.
pub fn gbp6_mint() -> Pubkey {
    Pubkey::new_from_array([5u8; 32])
}

/// The sponsor wallet fronting investor rent. Deterministic, so
/// `default_params` can name it as the rent collector.
pub fn sponsor() -> Keypair {
    Keypair::new_from_array([42u8; 32])
}

/// Deterministic XCAV token account for an owner. Not a real ATA; the program
/// only checks the mint and authority, so any token account works.
pub fn token_acc(owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"xcav_token", owner.as_ref()], &mid()).0
}

pub fn set_mint(svm: &mut LiteSVM) {
    set_mint_at(svm, xcav_mint(), DECIMALS);
    // The payment mints are real mints too: config initialization proves
    // every accepted entry exists and passes the mint guard.
    set_mint_at(svm, tgbp_mint(), 9);
    set_mint_at(svm, gbp6_mint(), 6);
}

/// The marketplace's reservation PDA for a payment token account.
pub fn reservation_pda(token_account: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            marketplace::constants::RESERVATION_SEED,
            token_account.as_ref(),
        ],
        &mid(),
    )
    .0
}

pub fn set_mint_at(svm: &mut LiteSVM, address: Pubkey, decimals: u8) {
    let mint = SplMint {
        mint_authority: COption::None,
        supply: 1_000_000_000_000,
        decimals,
        is_initialized: true,
        freeze_authority: COption::None,
    };
    let mut data = vec![0u8; SplMint::LEN];
    mint.pack_into_slice(&mut data);
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

/// Seed a classic mint that still has an account-state authority; the mint
/// guard must refuse it.
pub fn set_mint_with_lock_authority(svm: &mut LiteSVM, address: Pubkey) {
    let mint = SplMint {
        mint_authority: COption::None,
        supply: 1_000_000_000_000,
        decimals: DECIMALS,
        is_initialized: true,
        freeze_authority: COption::Some(Pubkey::new_from_array([13u8; 32])),
    };
    let mut data = vec![0u8; SplMint::LEN];
    mint.pack_into_slice(&mut data);
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

/// Seed a fee-bearing Token-2022 mint (transfer-fee TLV entry); the mint guard
/// must refuse it as a payment mint.
pub fn set_fee_bearing_mint(svm: &mut LiteSVM, address: Pubkey) {
    let mut data = vec![0u8; 166 + 4 + 108];
    data[45] = 1; // is_initialized
    data[165] = 1; // account type: mint
    data[166..168].copy_from_slice(&1u16.to_le_bytes()); // transfer fee config
    data[168..170].copy_from_slice(&108u16.to_le_bytes());
    svm.set_account(
        address,
        Account {
            lamports: 100_000_000,
            data,
            owner: anchor_spl::token_2022::ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

pub fn set_token_account(svm: &mut LiteSVM, address: Pubkey, owner: &Pubkey, amount: u64) {
    set_token_account_for(svm, xcav_mint(), address, owner, amount);
}

pub fn set_token_account_for(
    svm: &mut LiteSVM,
    mint: Pubkey,
    address: Pubkey,
    owner: &Pubkey,
    amount: u64,
) {
    let acc = SplAccount {
        mint,
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

/// Deterministic tGBP token account for an owner.
pub fn tgbp_acc(owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"tgbp_token", owner.as_ref()], &mid()).0
}

pub fn give_tgbp(svm: &mut LiteSVM, owner: &Pubkey, amount: u64) {
    set_token_account_for(svm, tgbp_mint(), tgbp_acc(owner), owner, amount);
}

pub fn tgbp_balance(svm: &LiteSVM, owner: &Pubkey) -> u64 {
    let acc = svm.get_account(&tgbp_acc(owner)).unwrap();
    SplAccount::unpack(&acc.data).unwrap().amount
}

/// Deterministic 6-decimal-GBP token account for an owner.
pub fn gbp6_acc(owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"gbp6_token", owner.as_ref()], &mid()).0
}

pub fn give_gbp6(svm: &mut LiteSVM, owner: &Pubkey, amount: u64) {
    set_token_account_for(svm, gbp6_mint(), gbp6_acc(owner), owner, amount);
}

pub fn gbp6_balance(svm: &LiteSVM, owner: &Pubkey) -> u64 {
    let acc = svm.get_account(&gbp6_acc(owner)).unwrap();
    SplAccount::unpack(&acc.data).unwrap().amount
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
    process_ixs(svm, &[ix], payer, signers)
}

pub fn process_ixs(
    svm: &mut LiteSVM,
    ixs: &[Instruction],
    payer: &Keypair,
    signers: &[&Keypair],
) -> Result<TransactionMetadata, FailedTransactionMetadata> {
    svm.expire_blockhash();
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    svm.send_transaction(tx)
}

/// The compute budget a client must request alongside `accept_offer`: PDA
/// and ATA derivation costs vary with the account keys, so the 200k default
/// leaves no safe margin for unlucky ones.
pub const ACCEPT_OFFER_BUDGET: u32 = 300_000;

/// `SetComputeUnitLimit`, hand-encoded (discriminant 2 + units), so the
/// tests carry no extra dependency for one fixed instruction.
pub fn set_compute_limit_ix(units: u32) -> Instruction {
    let mut data = vec![2u8];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction::new_with_bytes(
        "ComputeBudget111111111111111111111111111111"
            .parse()
            .unwrap(),
        &data,
        vec![],
    )
}

/// Send an instruction with the explicit `accept_offer` compute budget, the
/// way a client sends it.
pub fn process_with_budget(
    svm: &mut LiteSVM,
    ix: Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
) -> Result<TransactionMetadata, FailedTransactionMetadata> {
    process_ixs(
        svm,
        &[set_compute_limit_ix(ACCEPT_OFFER_BUDGET), ix],
        payer,
        signers,
    )
}

pub fn ok_with_budget(svm: &mut LiteSVM, ix: Instruction, payer: &Keypair, signers: &[&Keypair]) {
    if let Err(failed) = process_with_budget(svm, ix, payer, signers) {
        panic!("expected success, failed with: {:?}", failed.err);
    }
}

pub fn fails_with_budget(
    svm: &mut LiteSVM,
    ix: Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
    expected: &str,
) {
    match process_with_budget(svm, ix, payer, signers) {
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

/// Replace every occurrence of one account in an instruction, for the
/// stand-in tests. If the swap stops landing after a builder change, the
/// test's `fails_with` catches the unexpectedly healthy transaction.
pub fn swap_account(ix: &mut Instruction, from: Pubkey, to: Pubkey) {
    for account in ix.accounts.iter_mut() {
        if account.pubkey == from {
            account.pubkey = to;
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

// --- upgrade authority plumbing ---

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

// --- roles instruction builders ---

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

// --- marketplace instruction builders ---

pub fn default_params() -> ConfigParams {
    ConfigParams {
        treasury: Pubkey::new_from_array([9u8; 32]),
        rent_collector: sponsor().pubkey(),
        accepted_payment_mints: vec![tgbp_mint(), gbp6_mint()],
        listing_deposit: LISTING_DEPOSIT,
        lawyer_deposit: LAWYER_DEPOSIT,
        min_property_shares: 1,
        max_property_shares: 100,
        marketplace_fee_bps: 100,
        investor_fee_bps: 100,
        max_ownership_bps: 5_000,
        claiming_time: CLAIMING_TIME,
        legal_process_time: 100_000,
        lawyer_voting_time: 10_000,
        min_voting_quorum_bps: 2_500,
    }
}

pub fn init_ix(authority: &Pubkey) -> Instruction {
    init_ix_with(authority, default_params())
}

pub fn init_ix_with(authority: &Pubkey, params: ConfigParams) -> Instruction {
    let mut accounts = marketplace::accounts::InitializeConfig {
        authority: *authority,
        program: mid(),
        program_data: program_data_pda(&mid()),
        config: marketplace_config(),
        xcav_mint: xcav_mint(),
        vault: vault(),
        token_program: TOKEN_PROGRAM_ID,
        system_program: SYS,
    }
    .to_account_metas(None);
    // Every accepted payment mint rides along as a remaining account.
    accounts.extend(payment_mint_metas(&params));
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::InitializeConfig { params }.data(),
        accounts,
    )
}

pub fn update_config_ix(authority: &Pubkey, params: ConfigParams) -> Instruction {
    let mut accounts = marketplace::accounts::UpdateConfig {
        authority: *authority,
        config: marketplace_config(),
    }
    .to_account_metas(None);
    accounts.extend(payment_mint_metas(&params));
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::UpdateConfig { params }.data(),
        accounts,
    )
}

fn payment_mint_metas(
    params: &ConfigParams,
) -> Vec<anchor_lang::solana_program::instruction::AccountMeta> {
    params
        .accepted_payment_mints
        .iter()
        .map(|mint| {
            anchor_lang::solana_program::instruction::AccountMeta::new_readonly(*mint, false)
        })
        .collect()
}

pub fn update_authority_ix(authority: &Pubkey, new_authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::UpdateAuthority {
            new_authority: *new_authority,
        }
        .data(),
        marketplace::accounts::UpdateAuthority {
            authority: *authority,
            config: marketplace_config(),
        }
        .to_account_metas(None),
    )
}

pub fn accept_authority_ix(new_authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::AcceptAuthority {}.data(),
        marketplace::accounts::AcceptAuthority {
            new_authority: *new_authority,
            config: marketplace_config(),
        }
        .to_account_metas(None),
    )
}

/// Write a created `Region` account at its canonical PDA, exactly as the
/// regions program would leave it. Registering a lawyer only needs the account
/// to exist, so tests skip the whole proposal/vote/claim dance.
pub fn seed_region(svm: &mut LiteSVM, region_id: u16, owner: &Pubkey) {
    seed_region_taxed(svm, region_id, owner, 300)
}

pub fn seed_region_taxed(svm: &mut LiteSVM, region_id: u16, owner: &Pubkey, tax_bps: u16) {
    let (address, bump) = Pubkey::find_program_address(
        &[regions::REGION_SEED, &region_id.to_le_bytes()],
        &regions::id(),
    );
    let region = regions::state::Region {
        region_id,
        owner: *owner,
        collateral: 0,
        location_collateral: 0,
        next_owner_change: i64::MAX,
        listing_duration: 100_000,
        tax_bps,
        location_count: 0,
        bump,
    };
    let mut data = regions::state::Region::DISCRIMINATOR.to_vec();
    region.serialize(&mut data).unwrap();
    svm.set_account(
        address,
        Account {
            lamports: 100_000_000,
            data,
            owner: regions::id(),
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

/// Write a registered `Location` account at its canonical PDA, exactly as the
/// regions program would leave it. Listing only needs the account to exist.
pub fn seed_location(svm: &mut LiteSVM, region_id: u16, postcode: &[u8]) {
    let (address, bump) = Pubkey::find_program_address(
        &[regions::LOCATION_SEED, &region_id.to_le_bytes(), postcode],
        &regions::id(),
    );
    let location = regions::state::Location {
        region_id,
        postcode: postcode.to_vec(),
        deposit: 50_000_000,
        bump,
    };
    let mut data = regions::state::Location::DISCRIMINATOR.to_vec();
    location.serialize(&mut data).unwrap();
    svm.set_account(
        address,
        Account {
            lamports: 100_000_000,
            data,
            owner: regions::id(),
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

/// A SOL-funded, XCAV-holding keypair with the RealEstateDeveloper role.
pub fn new_developer(svm: &mut LiteSVM, admin: &Keypair) -> Keypair {
    let kp = actor(svm);
    ok(
        svm,
        roles_assign_ix(&admin.pubkey(), &kp.pubkey(), Role::RealEstateDeveloper),
        admin,
        &[admin],
    );
    set_compliance(svm, admin, &kp.pubkey(), true);
    kp
}

pub fn list_property_ix(
    developer: &Pubkey,
    listing_id: u64,
    region_id: u16,
    postcode: &[u8],
    share_price: u64,
    share_amount: u32,
) -> Instruction {
    list_property_ix_capped(
        developer,
        listing_id,
        region_id,
        postcode,
        share_price,
        share_amount,
        u64::MAX,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn list_property_ix_capped(
    developer: &Pubkey,
    listing_id: u64,
    region_id: u16,
    postcode: &[u8],
    share_price: u64,
    share_amount: u32,
    max_deposit: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::ListProperty {
            region_id,
            postcode: postcode.to_vec(),
            share_price,
            share_amount,
            tax_paid_by_developer: false,
            max_deposit,
        }
        .data(),
        marketplace::accounts::ListProperty {
            developer: *developer,
            config: marketplace_config(),
            developer_role: role_pda(developer, Role::RealEstateDeveloper),
            developer_compliance: compliance_pda(developer),
            region: region_pda(region_id),
            location: location_pda(region_id, postcode),
            property: property_pda(listing_id),
            listing: listing_pda(listing_id),
            xcav_mint: xcav_mint(),
            developer_token: token_acc(developer),
            vault: vault(),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// List with the default region 1 / seeded postcode / default price and amount.
pub fn list_ix(developer: &Pubkey, listing_id: u64) -> Instruction {
    list_property_ix(
        developer,
        listing_id,
        1,
        POSTCODE,
        SHARE_PRICE,
        SHARE_AMOUNT,
    )
}

/// `list_ix` with a caller-supplied deposit cap.
pub fn list_ix_capped(developer: &Pubkey, listing_id: u64, max_deposit: u64) -> Instruction {
    list_property_ix_capped(
        developer,
        listing_id,
        1,
        POSTCODE,
        SHARE_PRICE,
        SHARE_AMOUNT,
        max_deposit,
    )
}

pub fn upgrade_ix(developer: &Pubkey, listing_id: u64, new_price: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::UpgradeObject {
            listing_id,
            new_price,
        }
        .data(),
        marketplace::accounts::UpgradeObject {
            config: marketplace_config(),
            developer: *developer,
            developer_role: role_pda(developer, Role::RealEstateDeveloper),
            developer_compliance: compliance_pda(developer),
            listing: listing_pda(listing_id),
        }
        .to_account_metas(None),
    )
}

pub fn init_assets_ix(developer: &Pubkey, listing_id: u64) -> Instruction {
    init_assets_ix_full(
        developer,
        listing_id,
        "10 Test Street".into(),
        "ipfs://property-docs".into(),
    )
}

pub fn init_assets_ix_full(
    developer: &Pubkey,
    listing_id: u64,
    name: String,
    uri: String,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::InitPropertyAssets {
            listing_id,
            name,
            uri,
        }
        .data(),
        marketplace::accounts::InitPropertyAssets {
            config: marketplace_config(),
            developer: *developer,
            developer_role: role_pda(developer, Role::RealEstateDeveloper),
            developer_compliance: compliance_pda(developer),
            listing: listing_pda(listing_id),
            property: property_pda(listing_id),
            share_mint: share_mint_pda(listing_id),
            mint_auth: mint_auth_pda(listing_id),
            property_vault: property_vault_pda(listing_id),
            vault_share_account: vault_share_account(listing_id),
            token_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// A SOL-funded keypair with the RealEstateInvestor role and a tGBP balance.
pub fn new_investor(svm: &mut LiteSVM, admin: &Keypair) -> Keypair {
    let kp = funded(svm);
    give_tgbp(svm, &kp.pubkey(), 1_000_000_000_000);
    ok(
        svm,
        roles_assign_ix(&admin.pubkey(), &kp.pubkey(), Role::RealEstateInvestor),
        admin,
        &[admin],
    );
    set_compliance(svm, admin, &kp.pubkey(), true);
    kp
}

pub fn buy_ix(
    investor: &Pubkey,
    payer: &Pubkey,
    listing_id: u64,
    amount: u32,
    max_total_cost: u64,
) -> Instruction {
    buy_ix_with_mint(
        investor,
        payer,
        listing_id,
        amount,
        max_total_cost,
        tgbp_mint(),
        tgbp_acc(investor),
        listing_payment_ata(listing_id),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn buy_ix_with_mint(
    investor: &Pubkey,
    payer: &Pubkey,
    listing_id: u64,
    amount: u32,
    max_total_cost: u64,
    payment_mint: Pubkey,
    investor_payment: Pubkey,
    listing_payment_account: Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::BuyPropertyShares {
            listing_id,
            amount,
            max_total_cost,
        }
        .data(),
        marketplace::accounts::BuyPropertyShares {
            investor: *investor,
            payer: *payer,
            config: marketplace_config(),
            investor_role: role_pda(investor, Role::RealEstateInvestor),
            investor_compliance: compliance_pda(investor),
            listing: listing_pda(listing_id),
            property: property_pda(listing_id),
            position: position_pda(listing_id, investor),
            holding: holding_pda(listing_id, investor),
            payment_mint,
            investor_payment,
            listing_vault: listing_vault_pda(listing_id),
            listing_payment_account,
            share_mint: share_mint_pda(listing_id),
            mint_auth: mint_auth_pda(listing_id),
            property_vault: property_vault_pda(listing_id),
            vault_share_account: vault_share_account(listing_id),
            investor_share_account: investor_share_ata(listing_id, investor),
            payment_token_program: TOKEN_PROGRAM_ID,
            share_token_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn reserve_ix(
    investor: &Pubkey,
    payer: &Pubkey,
    listing_id: u64,
    amount: u32,
    max_total_cost: u64,
) -> Instruction {
    reserve_ix_with_mint(
        investor,
        payer,
        listing_id,
        amount,
        max_total_cost,
        tgbp_mint(),
        tgbp_acc(investor),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn reserve_ix_with_mint(
    investor: &Pubkey,
    payer: &Pubkey,
    listing_id: u64,
    amount: u32,
    max_total_cost: u64,
    payment_mint: Pubkey,
    investor_payment: Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::ReserveShares {
            listing_id,
            amount,
            max_total_cost,
        }
        .data(),
        marketplace::accounts::ReserveShares {
            investor: *investor,
            payer: *payer,
            config: marketplace_config(),
            investor_role: role_pda(investor, Role::RealEstateInvestor),
            investor_compliance: compliance_pda(investor),
            listing: listing_pda(listing_id),
            property: property_pda(listing_id),
            position: position_pda(listing_id, investor),
            payment_mint,
            investor_payment,
            reservation: reservation_pda(&investor_payment),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn claim_ix(investor: &Pubkey, payer: &Pubkey, listing_id: u64) -> Instruction {
    claim_ix_with_mint(
        investor,
        payer,
        listing_id,
        tgbp_mint(),
        tgbp_acc(investor),
        listing_payment_ata(listing_id),
    )
}

pub fn claim_ix_with_mint(
    investor: &Pubkey,
    payer: &Pubkey,
    listing_id: u64,
    payment_mint: Pubkey,
    investor_payment: Pubkey,
    listing_payment_account: Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::ClaimShares { listing_id }.data(),
        marketplace::accounts::ClaimShares {
            investor: *investor,
            payer: *payer,
            config: marketplace_config(),
            investor_role: role_pda(investor, Role::RealEstateInvestor),
            investor_compliance: compliance_pda(investor),
            listing: listing_pda(listing_id),
            property: property_pda(listing_id),
            position: position_pda(listing_id, investor),
            holding: holding_pda(listing_id, investor),
            payment_mint,
            investor_payment,
            reservation: reservation_pda(&investor_payment),
            listing_vault: listing_vault_pda(listing_id),
            listing_payment_account,
            share_mint: share_mint_pda(listing_id),
            mint_auth: mint_auth_pda(listing_id),
            property_vault: property_vault_pda(listing_id),
            vault_share_account: vault_share_account(listing_id),
            investor_share_account: investor_share_ata(listing_id, investor),
            payment_token_program: TOKEN_PROGRAM_ID,
            share_token_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn unreserve_ix(investor: &Pubkey, listing_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::UnreserveShares { listing_id }.data(),
        marketplace::accounts::UnreserveShares {
            investor: *investor,
            listing: listing_pda(listing_id),
            position: position_pda(listing_id, investor),
            reservation: reservation_pda(&tgbp_acc(investor)),
        }
        .to_account_metas(None),
    )
}

pub fn release_reservation_ix(cranker: &Pubkey, listing_id: u64, investor: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::ReleaseReservation {
            listing_id,
            investor: *investor,
        }
        .data(),
        marketplace::accounts::ReleaseReservation {
            cranker: *cranker,
            config: marketplace_config(),
            rent_collector: sponsor().pubkey(),
            listing: listing_pda(listing_id),
            position: position_pda(listing_id, investor),
            reservation: reservation_pda(&tgbp_acc(investor)),
        }
        .to_account_metas(None),
    )
}

pub fn close_reservation_ix(cranker: &Pubkey, token_account: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CloseReservation {}.data(),
        marketplace::accounts::CloseReservation {
            cranker: *cranker,
            config: marketplace_config(),
            rent_collector: sponsor().pubkey(),
            reservation: reservation_pda(token_account),
        }
        .to_account_metas(None),
    )
}

pub fn reservation_of(svm: &LiteSVM, token_account: &Pubkey) -> marketplace::state::Reservation {
    marketplace::state::Reservation::try_deserialize(
        &mut &svm
            .get_account(&reservation_pda(token_account))
            .unwrap()
            .data[..],
    )
    .unwrap()
}

/// Reserve the shares of listing 0 that nobody else has yet, using throwaway
/// filler investors in cap-sized chunks, so the sale can lock in.
pub fn fill_reserve(svm: &mut LiteSVM, admin: &Keypair) -> Vec<Keypair> {
    let sponsor = sponsor();
    let listing = listing_of(svm, 0);
    let mut left =
        listing.listed_share_amount - listing.sold_share_amount - listing.reserved_share_amount;
    let mut fillers = Vec::new();
    while left > 0 {
        let amount = left.min(49);
        let filler = new_investor(svm, admin);
        ok(
            svm,
            reserve_ix(&filler.pubkey(), &sponsor.pubkey(), 0, amount, u64::MAX),
            &sponsor,
            &[&sponsor, &filler],
        );
        left -= amount;
        fillers.push(filler);
    }
    fillers
}

/// The shortest path to paid shares under the lock-in rule: the buyers
/// reserve, fillers take whatever is left so the sale is fully reserved, the
/// SPV attests, and the buyers claim. Any fillers then sit out the claim
/// window and get released, which leaves the listing `Listed` with exactly
/// the buyers' shares sold, the rest open for direct purchase, and the clock
/// past the window. Buyers summing to every share means no fillers, no warp,
/// and a sold-out listing.
pub fn acquire_many(svm: &mut LiteSVM, admin: &Keypair, buyers: &[(&Keypair, u32)]) {
    let sponsor = sponsor();
    for (investor, amount) in buyers {
        ok(
            svm,
            reserve_ix(&investor.pubkey(), &sponsor.pubkey(), 0, *amount, u64::MAX),
            &sponsor,
            &[&sponsor, investor],
        );
    }
    let fillers = fill_reserve(svm, admin);
    if !property_of(svm, 0).spv_created {
        let confirmer = new_confirmer(svm, admin);
        ok(
            svm,
            create_spv_ix(&confirmer.pubkey(), 0),
            &confirmer,
            &[&confirmer],
        );
    }
    for (investor, _) in buyers {
        ok(
            svm,
            claim_ix(&investor.pubkey(), &sponsor.pubkey(), 0),
            &sponsor,
            &[&sponsor, investor],
        );
    }
    if !fillers.is_empty() {
        warp(svm, CLAIMING_TIME + 1);
        let cranker = funded(svm);
        for filler in &fillers {
            ok(
                svm,
                release_reservation_ix(&cranker.pubkey(), 0, &filler.pubkey()),
                &cranker,
                &[&cranker],
            );
        }
    }
}

pub fn acquire(svm: &mut LiteSVM, admin: &Keypair, investor: &Keypair, amount: u32) {
    acquire_many(svm, admin, &[(investor, amount)]);
}

pub fn close_position_ix(cranker: &Pubkey, listing_id: u64, investor: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CloseCancelledPosition {
            listing_id,
            investor: *investor,
        }
        .data(),
        marketplace::accounts::CloseCancelledPosition {
            cranker: *cranker,
            config: marketplace_config(),
            rent_collector: sponsor().pubkey(),
            listing: listing_pda(listing_id),
            position: position_pda(listing_id, investor),
        }
        .to_account_metas(None),
    )
}

/// A SOL-funded keypair with the SpvConfirmation role.
pub fn new_confirmer(svm: &mut LiteSVM, admin: &Keypair) -> Keypair {
    let kp = funded(svm);
    ok(
        svm,
        roles_assign_ix(&admin.pubkey(), &kp.pubkey(), Role::SpvConfirmation),
        admin,
        &[admin],
    );
    set_compliance(svm, admin, &kp.pubkey(), true);
    kp
}

pub fn create_spv_ix(confirmer: &Pubkey, listing_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CreateSpv { listing_id }.data(),
        marketplace::accounts::CreateSpv {
            confirmer: *confirmer,
            confirmer_role: role_pda(confirmer, Role::SpvConfirmation),
            listing: listing_pda(listing_id),
            property: property_pda(listing_id),
        }
        .to_account_metas(None),
    )
}

pub fn withdraw_expired_ix(investor: &Pubkey, listing_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::WithdrawExpired { listing_id }.data(),
        marketplace::accounts::WithdrawExpired {
            investor: *investor,
            config: marketplace_config(),
            rent_collector: sponsor().pubkey(),
            listing: listing_pda(listing_id),
            property: property_pda(listing_id),
            position: position_pda(listing_id, investor),
            holding: holding_pda(listing_id, investor),
            payment_mint: tgbp_mint(),
            investor_payment: tgbp_acc(investor),
            listing_vault: listing_vault_pda(listing_id),
            listing_payment_account: listing_payment_ata(listing_id),
            share_mint: share_mint_pda(listing_id),
            mint_auth: mint_auth_pda(listing_id),
            property_vault: property_vault_pda(listing_id),
            vault_share_account: vault_share_account(listing_id),
            investor_share_account: investor_share_ata(listing_id, investor),
            payment_token_program: TOKEN_PROGRAM_ID,
            share_token_program: anchor_spl::token_2022::ID,
        }
        .to_account_metas(None),
    )
}

pub fn withdraw_legal_expired_ix(investor: &Pubkey, listing_id: u64) -> Instruction {
    let mut ix = withdraw_expired_ix(investor, listing_id);
    ix.data = marketplace::instruction::WithdrawLegalProcessExpired { listing_id }.data();
    ix
}

pub fn withdraw_cancelled_ix(investor: &Pubkey, listing_id: u64) -> Instruction {
    let mut ix = withdraw_expired_ix(investor, listing_id);
    ix.data = marketplace::instruction::WithdrawCancelled { listing_id }.data();
    ix
}

pub fn treasury() -> Pubkey {
    Pubkey::new_from_array([9u8; 32])
}

/// The treasury's associated tGBP account (classic-token derivation).
pub fn treasury_payment_ata() -> Pubkey {
    Pubkey::find_program_address(
        &[
            treasury().as_ref(),
            TOKEN_PROGRAM_ID.as_ref(),
            tgbp_mint().as_ref(),
        ],
        &anchor_spl::associated_token::ID,
    )
    .0
}

pub fn settle_cancelled_fees_ix(cranker: &Pubkey, listing_id: u64, lawyer: &Pubkey) -> Instruction {
    settle_fees_ix_with_mint(cranker, listing_id, tgbp_mint(), lawyer)
}

pub fn settle_fees_ix_with_mint(
    cranker: &Pubkey,
    listing_id: u64,
    mint: Pubkey,
    lawyer: &Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::SettleCancelledFees { listing_id }.data(),
        marketplace::accounts::SettleCancelledFees {
            cranker: *cranker,
            config: marketplace_config(),
            listing: listing_pda(listing_id),
            payment_mint: mint,
            listing_vault: listing_vault_pda(listing_id),
            listing_payment_account: payment_ata(&listing_vault_pda(listing_id), &mint),
            lawyer: *lawyer,
            lawyer_payment_account: payment_ata(lawyer, &mint),
            treasury: treasury(),
            treasury_payment_account: payment_ata(&treasury(), &mint),
            payment_token_program: TOKEN_PROGRAM_ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// The dead-listing exit accounts for a position paid in `mint`; the caller
/// picks which withdraw variant by overriding the data.
pub fn withdraw_exit_ix_with_mint(
    investor: &Pubkey,
    listing_id: u64,
    mint: Pubkey,
    investor_account: Pubkey,
) -> Instruction {
    let mut ix = withdraw_expired_ix(investor, listing_id);
    let accounts = marketplace::accounts::WithdrawExpired {
        investor: *investor,
        config: marketplace_config(),
        rent_collector: sponsor().pubkey(),
        listing: listing_pda(listing_id),
        property: property_pda(listing_id),
        position: position_pda(listing_id, investor),
        holding: holding_pda(listing_id, investor),
        payment_mint: mint,
        investor_payment: investor_account,
        listing_vault: listing_vault_pda(listing_id),
        listing_payment_account: payment_ata(&listing_vault_pda(listing_id), &mint),
        share_mint: share_mint_pda(listing_id),
        mint_auth: mint_auth_pda(listing_id),
        property_vault: property_vault_pda(listing_id),
        vault_share_account: vault_share_account(listing_id),
        investor_share_account: investor_share_ata(listing_id, investor),
        payment_token_program: TOKEN_PROGRAM_ID,
        share_token_program: anchor_spl::token_2022::ID,
    }
    .to_account_metas(None);
    ix.accounts = accounts;
    ix
}

pub fn confirm_docs_ix(
    lawyer: &Pubkey,
    listing_id: u64,
    approve: bool,
    documents_hash: [u8; 32],
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::LawyerConfirmDocuments {
            listing_id,
            approve,
            documents_hash,
        }
        .data(),
        marketplace::accounts::ConfirmDocuments {
            lawyer: *lawyer,
            lawyer_role: role_pda(lawyer, Role::Lawyer),
            listing: listing_pda(listing_id),
        }
        .to_account_metas(None),
    )
}

pub fn close_case_ix(cranker: &Pubkey, listing_id: u64, lawyer: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CloseCase {
            listing_id,
            lawyer: *lawyer,
        }
        .data(),
        marketplace::accounts::CloseCase {
            cranker: *cranker,
            listing: listing_pda(listing_id),
            registry: lawyer_pda(lawyer),
        }
        .to_account_metas(None),
    )
}

/// A vault or treasury associated account for one of the accepted payment
/// mints (classic-token derivation, which both test mints use).
pub fn payment_ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), TOKEN_PROGRAM_ID.as_ref(), mint.as_ref()],
        &anchor_spl::associated_token::ID,
    )
    .0
}

pub fn close_dead_listing_ix(
    cranker: &Pubkey,
    listing_id: u64,
    developer: &Pubkey,
    with_mint: bool,
) -> Instruction {
    close_dead_listing_ix_for(cranker, listing_id, developer, with_mint, &[tgbp_mint()])
}

/// The remaining-account triples must mirror `listing.collected`, so tests
/// whose listing only ever took one mint pass just that one.
pub fn close_dead_listing_ix_for(
    cranker: &Pubkey,
    listing_id: u64,
    developer: &Pubkey,
    with_mint: bool,
    mints: &[Pubkey],
) -> Instruction {
    let mut accounts = marketplace::accounts::CloseDeadListing {
        cranker: *cranker,
        config: marketplace_config(),
        rent_collector: sponsor().pubkey(),
        developer: *developer,
        listing: listing_pda(listing_id),
        property: property_pda(listing_id),
        share_mint: with_mint.then(|| share_mint_pda(listing_id)),
        mint_auth: mint_auth_pda(listing_id),
        property_vault: property_vault_pda(listing_id),
        vault_share_account: with_mint.then(|| vault_share_account(listing_id)),
        listing_vault: listing_vault_pda(listing_id),
        share_token_program: anchor_spl::token_2022::ID,
        payment_token_program: TOKEN_PROGRAM_ID,
    }
    .to_account_metas(None);
    // One (vault account, mint, treasury account) triple per collected
    // mint, in the listing's order.
    use anchor_lang::solana_program::instruction::AccountMeta;
    for &mint in mints {
        accounts.push(AccountMeta::new(
            payment_ata(&listing_vault_pda(listing_id), &mint),
            false,
        ));
        accounts.push(AccountMeta::new_readonly(mint, false));
        accounts.push(AccountMeta::new(payment_ata(&treasury(), &mint), false));
    }
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CloseDeadListing { listing_id }.data(),
        accounts,
    )
}

/// Same triple layout as the dead-listing close, for a settled listing.
pub fn close_settled_payment_accounts_ix(
    cranker: &Pubkey,
    listing_id: u64,
    mints: &[Pubkey],
) -> Instruction {
    let mut accounts = marketplace::accounts::CloseSettledPaymentAccounts {
        cranker: *cranker,
        config: marketplace_config(),
        rent_collector: sponsor().pubkey(),
        listing: listing_pda(listing_id),
        listing_vault: listing_vault_pda(listing_id),
        share_token_program: anchor_spl::token_2022::ID,
        payment_token_program: TOKEN_PROGRAM_ID,
    }
    .to_account_metas(None);
    use anchor_lang::solana_program::instruction::AccountMeta;
    for &mint in mints {
        accounts.push(AccountMeta::new(
            payment_ata(&listing_vault_pda(listing_id), &mint),
            false,
        ));
        accounts.push(AccountMeta::new_readonly(mint, false));
        accounts.push(AccountMeta::new(payment_ata(&treasury(), &mint), false));
    }
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CloseSettledPaymentAccounts { listing_id }.data(),
        accounts,
    )
}

/// One payout group per mint, in the listing's collected order. Payment
/// accounts are the deterministic test accounts for each party.
#[allow(clippy::too_many_arguments)]
pub fn execute_deal_ix(
    cranker: &Pubkey,
    listing_id: u64,
    region_id: u16,
    developer: &Pubkey,
    dev_lawyer: &Pubkey,
    spv_lawyer: &Pubkey,
    region_owner: &Pubkey,
    mints: &[Pubkey],
) -> Instruction {
    let mut accounts = marketplace::accounts::ExecuteDeal {
        cranker: *cranker,
        config: marketplace_config(),
        listing: listing_pda(listing_id),
        property: property_pda(listing_id),
        region: region_pda(region_id),
        developer_lawyer_registry: lawyer_pda(dev_lawyer),
        spv_lawyer_registry: lawyer_pda(spv_lawyer),
        listing_vault: listing_vault_pda(listing_id),
        xcav_mint: xcav_mint(),
        developer_token: token_acc(developer),
        vault: vault(),
        token_program: TOKEN_PROGRAM_ID,
        payment_token_program: TOKEN_PROGRAM_ID,
    }
    .to_account_metas(None);
    use anchor_lang::solana_program::instruction::AccountMeta;
    for mint in mints {
        let acc = |owner: &Pubkey| {
            if *mint == gbp6_mint() {
                gbp6_acc(owner)
            } else {
                tgbp_acc(owner)
            }
        };
        accounts.push(AccountMeta::new_readonly(*mint, false));
        accounts.push(AccountMeta::new(
            payment_ata(&listing_vault_pda(listing_id), mint),
            false,
        ));
        for payee in [developer, dev_lawyer, spv_lawyer, region_owner] {
            accounts.push(AccountMeta::new(acc(payee), false));
        }
        accounts.push(AccountMeta::new(payment_ata(&treasury(), mint), false));
    }
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::ExecuteDeal { listing_id }.data(),
        accounts,
    )
}

pub fn resolve_silent_ix(cranker: &Pubkey, listing_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::ResolveSilentVerdict { listing_id }.data(),
        marketplace::accounts::ResolveSilentVerdict {
            cranker: *cranker,
            listing: listing_pda(listing_id),
        }
        .to_account_metas(None),
    )
}

pub fn withdraw_deposit_ix(developer: &Pubkey, listing_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::WithdrawDepositUnsold { listing_id }.data(),
        marketplace::accounts::WithdrawDepositUnsold {
            developer: *developer,
            config: marketplace_config(),
            listing: listing_pda(listing_id),
            xcav_mint: xcav_mint(),
            developer_token: token_acc(developer),
            vault: vault(),
            treasury_token: Some(payment_ata(&treasury(), &xcav_mint())),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
    )
}

pub fn position_of(
    svm: &LiteSVM,
    listing_id: u64,
    investor: &Pubkey,
) -> marketplace::state::InvestorPosition {
    marketplace::state::InvestorPosition::try_deserialize(
        &mut &svm
            .get_account(&position_pda(listing_id, investor))
            .unwrap()
            .data[..],
    )
    .unwrap()
}

pub fn holding_of(
    svm: &LiteSVM,
    asset_id: u64,
    owner: &Pubkey,
) -> marketplace::state::ShareHolding {
    marketplace::state::ShareHolding::try_deserialize(
        &mut &svm.get_account(&holding_pda(asset_id, owner)).unwrap().data[..],
    )
    .unwrap()
}

pub fn listing_of(svm: &LiteSVM, listing_id: u64) -> marketplace::state::Listing {
    marketplace::state::Listing::try_deserialize(
        &mut &svm.get_account(&listing_pda(listing_id)).unwrap().data[..],
    )
    .unwrap()
}

pub fn property_of(svm: &LiteSVM, asset_id: u64) -> marketplace::state::PropertyAsset {
    marketplace::state::PropertyAsset::try_deserialize(
        &mut &svm.get_account(&property_pda(asset_id)).unwrap().data[..],
    )
    .unwrap()
}

pub fn register_lawyer_ix(lawyer: &Pubkey, region_id: u16) -> Instruction {
    register_lawyer_ix_capped(lawyer, region_id, u64::MAX)
}

pub fn register_lawyer_ix_capped(lawyer: &Pubkey, region_id: u16, max_deposit: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::RegisterLawyer {
            region_id,
            max_deposit,
        }
        .data(),
        marketplace::accounts::RegisterLawyer {
            lawyer: *lawyer,
            payer: sponsor().pubkey(),
            config: marketplace_config(),
            lawyer_role: role_pda(lawyer, Role::Lawyer),
            region: region_pda(region_id),
            lawyer_account: lawyer_pda(lawyer),
            xcav_mint: xcav_mint(),
            lawyer_token: token_acc(lawyer),
            vault: vault(),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn unregister_lawyer_ix(lawyer: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::UnregisterLawyer {}.data(),
        marketplace::accounts::UnregisterLawyer {
            lawyer: *lawyer,
            config: marketplace_config(),
            rent_collector: sponsor().pubkey(),
            lawyer_account: lawyer_pda(lawyer),
            xcav_mint: xcav_mint(),
            lawyer_token: token_acc(lawyer),
            vault: vault(),
            token_program: TOKEN_PROGRAM_ID,
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

pub fn lawyer_of(svm: &LiteSVM, wallet: &Pubkey) -> marketplace::state::Lawyer {
    marketplace::state::Lawyer::try_deserialize(
        &mut &svm.get_account(&lawyer_pda(wallet)).unwrap().data[..],
    )
    .unwrap()
}

/// Overwrite a registered lawyer's active-case count. The instructions that
/// assign and close cases aren't built yet, so tests poke the field directly.
pub fn set_active_cases(svm: &mut LiteSVM, wallet: &Pubkey, active_cases: u32) {
    let mut lawyer = lawyer_of(svm, wallet);
    lawyer.active_cases = active_cases;
    let mut data = marketplace::state::Lawyer::DISCRIMINATOR.to_vec();
    lawyer.serialize(&mut data).unwrap();
    let mut acc = svm.get_account(&lawyer_pda(wallet)).unwrap();
    acc.data = data;
    svm.set_account(lawyer_pda(wallet), acc).unwrap();
}

/// A SOL-funded, XCAV-holding keypair with the Lawyer role.
pub fn new_lawyer(svm: &mut LiteSVM, admin: &Keypair) -> Keypair {
    let kp = actor(svm);
    ok(
        svm,
        roles_assign_ix(&admin.pubkey(), &kp.pubkey(), Role::Lawyer),
        admin,
        &[admin],
    );
    set_compliance(svm, admin, &kp.pubkey(), true);
    kp
}

pub fn lawyer_vote_pda(listing_id: u64, round: u64, voter: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            marketplace::LAWYER_VOTE_SEED,
            &listing_id.to_le_bytes(),
            &round.to_le_bytes(),
            voter.as_ref(),
        ],
        &mid(),
    )
    .0
}

pub fn candidacy_pda(listing_id: u64, round: u64, lawyer: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            marketplace::LAWYER_CANDIDATE_SEED,
            &listing_id.to_le_bytes(),
            &round.to_le_bytes(),
            lawyer.as_ref(),
        ],
        &mid(),
    )
    .0
}

pub fn candidacy_of(
    svm: &LiteSVM,
    listing_id: u64,
    round: u64,
    lawyer: &Pubkey,
) -> marketplace::state::LawyerCandidacy {
    marketplace::state::LawyerCandidacy::try_deserialize(
        &mut &svm
            .get_account(&candidacy_pda(listing_id, round, lawyer))
            .unwrap()
            .data[..],
    )
    .unwrap()
}

/// A registered lawyer in the given region: role, XCAV for the deposit, and a
/// registry entry.
pub fn new_registered_lawyer(svm: &mut LiteSVM, admin: &Keypair, region_id: u16) -> Keypair {
    let kp = new_lawyer(svm, admin);
    ok(
        svm,
        register_lawyer_ix(&kp.pubkey(), region_id),
        &kp,
        &[&kp, &sponsor()],
    );
    kp
}

pub fn assign_dev_lawyer_ix(developer: &Pubkey, listing_id: u64, lawyer: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::AssignDeveloperLawyer {
            listing_id,
            lawyer: *lawyer,
        }
        .data(),
        marketplace::accounts::AssignDeveloperLawyer {
            config: marketplace_config(),
            developer: *developer,
            developer_role: role_pda(developer, Role::RealEstateDeveloper),
            lawyer_role: role_pda(lawyer, Role::Lawyer),
            lawyer_compliance: compliance_pda(lawyer),
            registry: lawyer_pda(lawyer),
            listing: listing_pda(listing_id),
            property: property_pda(listing_id),
        }
        .to_account_metas(None),
    )
}

pub fn claim_spv_ix(lawyer: &Pubkey, listing_id: u64, round: u64, costs: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::ClaimSpvCase {
            listing_id,
            round,
            costs,
        }
        .data(),
        marketplace::accounts::ClaimSpvCase {
            config: marketplace_config(),
            lawyer: *lawyer,
            payer: sponsor().pubkey(),
            lawyer_role: role_pda(lawyer, Role::Lawyer),
            lawyer_compliance: compliance_pda(lawyer),
            registry: lawyer_pda(lawyer),
            listing: listing_pda(listing_id),
            property: property_pda(listing_id),
            candidacy: candidacy_pda(listing_id, round, lawyer),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// Vote for a candidate; `previous` names the candidate a revote moves power
/// away from.
pub fn vote_spv_ix(
    voter: &Pubkey,
    listing_id: u64,
    round: u64,
    choice: &Pubkey,
    previous: Option<&Pubkey>,
    amount: u32,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::VoteOnSpvLawyer { listing_id, amount }.data(),
        marketplace::accounts::VoteOnSpvLawyer {
            voter: *voter,
            payer: sponsor().pubkey(),
            voter_role: role_pda(voter, Role::RealEstateInvestor),
            listing: listing_pda(listing_id),
            holding: holding_pda(listing_id, voter),
            vote_record: lawyer_vote_pda(listing_id, round, voter),
            candidacy: candidacy_pda(listing_id, round, choice),
            previous_candidacy: previous.map(|lawyer| candidacy_pda(listing_id, round, lawyer)),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn finalize_spv_ix(
    cranker: &Pubkey,
    listing_id: u64,
    round: u64,
    winner: Option<&Pubkey>,
    candidates: &[Pubkey],
) -> Instruction {
    let mut accounts = marketplace::accounts::FinalizeSpvElection {
        cranker: *cranker,
        listing: listing_pda(listing_id),
        property: property_pda(listing_id),
        winner_registry: winner.map(lawyer_pda),
    }
    .to_account_metas(None);
    for lawyer in candidates {
        accounts.push(anchor_lang::solana_program::instruction::AccountMeta::new(
            candidacy_pda(listing_id, round, lawyer),
            false,
        ));
    }
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::FinalizeSpvElection { listing_id }.data(),
        accounts,
    )
}

pub fn close_candidacy_ix(
    cranker: &Pubkey,
    listing_id: u64,
    round: u64,
    lawyer: &Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CloseCandidacy {
            listing_id,
            round,
            lawyer: *lawyer,
        }
        .data(),
        marketplace::accounts::CloseCandidacy {
            cranker: *cranker,
            rent_payer: sponsor().pubkey(),
            listing: listing_pda(listing_id),
            candidacy: candidacy_pda(listing_id, round, lawyer),
        }
        .to_account_metas(None),
    )
}

pub fn unlock_votes_ix(voter: &Pubkey, listing_id: u64, round: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::UnlockVotingShares { listing_id, round }.data(),
        marketplace::accounts::UnlockVotingShares {
            voter: *voter,
            rent_payer: sponsor().pubkey(),
            listing: listing_pda(listing_id),
            holding: holding_pda(listing_id, voter),
            vote_record: lawyer_vote_pda(listing_id, round, voter),
        }
        .to_account_metas(None),
    )
}

pub fn resign_case_ix(lawyer: &Pubkey, listing_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::ResignFromCase { listing_id }.data(),
        marketplace::accounts::ResignFromCase {
            lawyer: *lawyer,
            registry: lawyer_pda(lawyer),
            listing: listing_pda(listing_id),
        }
        .to_account_metas(None),
    )
}

// --- setup ---

pub fn config_of(svm: &LiteSVM) -> MarketplaceConfig {
    MarketplaceConfig::try_deserialize(
        &mut &svm.get_account(&marketplace_config()).unwrap().data[..],
    )
    .unwrap()
}

/// Advance the clock by `secs` seconds.
pub fn warp(svm: &mut LiteSVM, secs: i64) {
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += secs;
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

// Loads the roles and marketplace programs, seeds the XCAV mint, initializes
// both configs, and returns (svm, admin, authority). The admin can hand out
// roles.
pub fn setup() -> (LiteSVM, Keypair, Keypair) {
    let mut svm = LiteSVM::new();
    svm.add_program(roles_id(), &program_bytes("xcavate_whitelist"))
        .unwrap();
    svm.add_program(mid(), &program_bytes("marketplace"))
        .unwrap();
    set_mint(&mut svm);
    // The treasury's XCAV account, where the abandonment slash lands.
    set_token_account_for(
        &mut svm,
        xcav_mint(),
        payment_ata(&treasury(), &xcav_mint()),
        &treasury(),
        0,
    );

    let authority = funded(&mut svm);
    svm.airdrop(&sponsor().pubkey(), 100_000_000_000).unwrap();
    bind_upgrade_authority(&mut svm, &roles_id(), &authority.pubkey());
    bind_upgrade_authority(&mut svm, &mid(), &authority.pubkey());
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

    ok(
        &mut svm,
        init_ix(&authority.pubkey()),
        &authority,
        &[&authority],
    );
    (svm, admin, authority)
}

// --- secondary market ---

pub fn share_listing_pda(id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[marketplace::SHARE_LISTING_SEED, &id.to_le_bytes()],
        &mid(),
    )
    .0
}

pub fn share_listing_of(svm: &LiteSVM, id: u64) -> marketplace::state::ShareListing {
    let acc = svm.get_account(&share_listing_pda(id)).unwrap();
    marketplace::state::ShareListing::try_deserialize(&mut acc.data.as_slice()).unwrap()
}

pub fn marketplace_cpi_auth() -> Pubkey {
    Pubkey::find_program_address(&[marketplace::CPI_AUTH_SEED], &mid()).0
}

/// The property program's income and checkpoint PDAs, derived with its
/// published seeds; the settlement CPI reads them.
pub fn property_income_pda(asset_id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[b"income", &asset_id.to_le_bytes()],
        &marketplace::PROPERTY_PROGRAM,
    )
    .0
}
pub fn property_checkpoint_pda(asset_id: u64, owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"checkpoint", &asset_id.to_le_bytes(), owner.as_ref()],
        &marketplace::PROPERTY_PROGRAM,
    )
    .0
}

pub fn relist_ix(seller: &Pubkey, asset_id: u64, id: u64, amount: u32, price: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::RelistShares {
            asset_id,
            amount,
            share_price: price,
        }
        .data(),
        marketplace::accounts::RelistShares {
            seller: *seller,
            payer: *seller,
            config: marketplace_config(),
            seller_role: role_pda(seller, Role::RealEstateInvestor),
            seller_compliance: compliance_pda(seller),
            listing: listing_pda(asset_id),
            holding: holding_pda(asset_id, seller),
            share_listing: share_listing_pda(id),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn delist_ix(seller: &Pubkey, asset_id: u64, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::DelistShares {}.data(),
        marketplace::accounts::DelistShares {
            seller: *seller,
            rent_payer: *seller,
            share_listing: share_listing_pda(id),
            holding: holding_pda(asset_id, seller),
        }
        .to_account_metas(None),
    )
}

pub fn buy_relisted_ix(
    buyer: &Pubkey,
    asset_id: u64,
    id: u64,
    seller: &Pubkey,
    amount: u32,
    max_total_cost: u64,
) -> Instruction {
    buy_relisted_ix_with_mint(
        buyer,
        asset_id,
        id,
        seller,
        amount,
        max_total_cost,
        tgbp_mint(),
        tgbp_acc(buyer),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn buy_relisted_ix_with_mint(
    buyer: &Pubkey,
    asset_id: u64,
    id: u64,
    seller: &Pubkey,
    amount: u32,
    max_total_cost: u64,
    payment_mint: Pubkey,
    buyer_payment: Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::BuyRelistedShares {
            asset_id,
            id,
            amount,
            max_total_cost,
        }
        .data(),
        marketplace::accounts::BuyRelistedShares {
            buyer: *buyer,
            payer: *buyer,
            buyer_role: role_pda(buyer, Role::RealEstateInvestor),
            buyer_compliance: compliance_pda(buyer),
            config: marketplace_config(),
            listing: listing_pda(asset_id),
            property: property_pda(asset_id),
            share_listing: share_listing_pda(id),
            seller: *seller,
            rent_payer: *seller,
            seller_holding: holding_pda(asset_id, seller),
            buyer_holding: holding_pda(asset_id, buyer),
            payment_mint,
            buyer_payment,
            seller_payment: payment_ata(seller, &payment_mint),
            treasury: treasury(),
            treasury_payment: payment_ata(&treasury(), &payment_mint),
            share_mint: share_mint_pda(asset_id),
            mint_auth: mint_auth_pda(asset_id),
            seller_share_account: investor_share_ata(asset_id, seller),
            buyer_share_account: investor_share_ata(asset_id, buyer),
            cpi_auth: marketplace_cpi_auth(),
            income: property_income_pda(asset_id),
            seller_checkpoint: property_checkpoint_pda(asset_id, seller),
            buyer_checkpoint: property_checkpoint_pda(asset_id, buyer),
            property_program: marketplace::PROPERTY_PROGRAM,
            payment_token_program: TOKEN_PROGRAM_ID,
            share_token_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn close_holding_ix(cranker: &Pubkey, asset_id: u64, owner: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CloseShareHolding {}.data(),
        marketplace::accounts::CloseShareHolding {
            cranker: *cranker,
            config: marketplace_config(),
            rent_collector: sponsor().pubkey(),
            property: property_pda(asset_id),
            holding: holding_pda(asset_id, owner),
        }
        .to_account_metas(None),
    )
}

/// Raw balance of any token account address, Token-2022 extensions included.
pub fn token_balance(svm: &LiteSVM, address: &Pubkey) -> u64 {
    use anchor_spl::token_2022::spl_token_2022::{
        extension::StateWithExtensions, state::Account as Token2022Account,
    };
    let acc = svm.get_account(address).unwrap();
    StateWithExtensions::<Token2022Account>::unpack(&acc.data)
        .unwrap()
        .base
        .amount
}

// --- finalized-property fixture (secondary market) ---

pub const COSTS: u64 = 1_000_000_000;
pub const DOCS: [u8; 32] = [7u8; 32];

/// (shares, pays in gbp6) per investor; a mixed-mint sellout of 100.
pub const BUYS: [(u32, bool); 4] = [(34, false), (33, false), (24, true), (9, true)];

/// A settled, finalized property. Investors hold 34/33/24/9; the first one
/// still carries their 34-share lawyer-election lock.
pub fn finalized_property() -> (LiteSVM, Keypair, Vec<Keypair>) {
    build_property(true)
}

/// Same flow; `finalize` false stops at `Legal`, holders already claimed.
pub fn build_property(finalize: bool) -> (LiteSVM, Keypair, Vec<Keypair>) {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let spn = sponsor();
    let investors: Vec<Keypair> = (0..4).map(|_| new_investor(&mut svm, &admin)).collect();
    for (investor, (shares, gbp6)) in investors.iter().zip(BUYS) {
        let ix = if gbp6 {
            give_gbp6(&mut svm, &investor.pubkey(), 1_000_000_000);
            reserve_ix_with_mint(
                &investor.pubkey(),
                &spn.pubkey(),
                0,
                shares,
                u64::MAX,
                gbp6_mint(),
                gbp6_acc(&investor.pubkey()),
            )
        } else {
            reserve_ix(&investor.pubkey(), &spn.pubkey(), 0, shares, u64::MAX)
        };
        ok(&mut svm, ix, &spn, &[&spn, investor]);
    }
    let confirmer = new_confirmer(&mut svm, &admin);
    ok(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
    );
    for (investor, (_, gbp6)) in investors.iter().zip(BUYS) {
        let ix = if gbp6 {
            claim_ix_with_mint(
                &investor.pubkey(),
                &spn.pubkey(),
                0,
                gbp6_mint(),
                gbp6_acc(&investor.pubkey()),
                payment_ata(&listing_vault_pda(0), &gbp6_mint()),
            )
        } else {
            claim_ix(&investor.pubkey(), &spn.pubkey(), 0)
        };
        ok(&mut svm, ix, &spn, &[&spn, investor]);
    }

    let dl = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &dl.pubkey()),
        &developer,
        &[&developer],
    );
    let sl = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&sl.pubkey(), 0, 1, COSTS),
        &sl,
        &[&sl, &spn],
    );
    ok(
        &mut svm,
        vote_spv_ix(&investors[0].pubkey(), 0, 1, &sl.pubkey(), None, 34),
        &spn,
        &[&spn, &investors[0]],
    );
    warp(&mut svm, 10_001);
    ok(
        &mut svm,
        finalize_spv_ix(&operator.pubkey(), 0, 1, Some(&sl.pubkey()), &[sl.pubkey()]),
        &operator,
        &[&operator],
    );
    for lawyer in [&dl, &sl] {
        ok(
            &mut svm,
            confirm_docs_ix(&lawyer.pubkey(), 0, true, DOCS),
            lawyer,
            &[lawyer],
        );
    }

    for wallet in [
        &developer.pubkey(),
        &dl.pubkey(),
        &sl.pubkey(),
        &operator.pubkey(),
    ] {
        give_tgbp(&mut svm, wallet, 0);
        give_gbp6(&mut svm, wallet, 0);
    }
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        treasury_payment_ata(),
        &treasury(),
        0,
    );
    set_token_account_for(
        &mut svm,
        gbp6_mint(),
        payment_ata(&treasury(), &gbp6_mint()),
        &treasury(),
        0,
    );
    if finalize {
        let cranker = funded(&mut svm);
        ok(
            &mut svm,
            execute_deal_ix(
                &cranker.pubkey(),
                0,
                1,
                &developer.pubkey(),
                &dl.pubkey(),
                &sl.pubkey(),
                &operator.pubkey(),
                &[tgbp_mint(), gbp6_mint()],
            ),
            &cranker,
            &[&cranker],
        );
        assert_eq!(listing_of(&svm, 0).status, ListingStatus::Finalized);
    }
    (svm, admin, investors)
}

// --- offers ---

pub fn offer_pda(listing_id: u64, offeror: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            marketplace::OFFER_SEED,
            &listing_id.to_le_bytes(),
            offeror.as_ref(),
        ],
        &mid(),
    )
    .0
}
pub fn offer_vault_pda(listing_id: u64, offeror: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            marketplace::OFFER_VAULT_SEED,
            &listing_id.to_le_bytes(),
            offeror.as_ref(),
        ],
        &mid(),
    )
    .0
}
pub fn offer_of(svm: &LiteSVM, listing_id: u64, offeror: &Pubkey) -> marketplace::state::Offer {
    let acc = svm.get_account(&offer_pda(listing_id, offeror)).unwrap();
    marketplace::state::Offer::try_deserialize(&mut acc.data.as_slice()).unwrap()
}

pub fn make_offer_ix(
    offeror: &Pubkey,
    id: u64,
    amount: u32,
    share_price: u64,
    payment_mint: Pubkey,
    offeror_payment: Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::MakeOffer {
            id,
            amount,
            share_price,
        }
        .data(),
        marketplace::accounts::MakeOffer {
            offeror: *offeror,
            payer: *offeror,
            offeror_role: role_pda(offeror, Role::RealEstateInvestor),
            offeror_compliance: compliance_pda(offeror),
            config: marketplace_config(),
            share_listing: share_listing_pda(id),
            offer: offer_pda(id, offeror),
            offer_vault: offer_vault_pda(id, offeror),
            payment_mint,
            offeror_payment,
            vault_payment_account: payment_ata(&offer_vault_pda(id, offeror), &payment_mint),
            payment_token_program: TOKEN_PROGRAM_ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn accept_offer_ix(
    seller: &Pubkey,
    asset_id: u64,
    id: u64,
    offeror: &Pubkey,
    nonce: u64,
    payment_mint: Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::AcceptOffer { id, nonce }.data(),
        marketplace::accounts::AcceptOffer {
            seller: *seller,
            payer: *seller,
            seller_role: role_pda(seller, Role::RealEstateInvestor),
            seller_compliance: compliance_pda(seller),
            config: marketplace_config(),
            listing: listing_pda(asset_id),
            property: property_pda(asset_id),
            share_listing: share_listing_pda(id),
            listing_rent_payer: *seller,
            offeror: *offeror,
            offeror_role: role_pda(offeror, Role::RealEstateInvestor),
            offeror_compliance: compliance_pda(offeror),
            offer: offer_pda(id, offeror),
            offer_rent_payer: *offeror,
            seller_holding: holding_pda(asset_id, seller),
            offeror_holding: holding_pda(asset_id, offeror),
            payment_mint,
            offer_vault: offer_vault_pda(id, offeror),
            vault_payment_account: payment_ata(&offer_vault_pda(id, offeror), &payment_mint),
            seller_payment: payment_ata(seller, &payment_mint),
            treasury: treasury(),
            treasury_payment: payment_ata(&treasury(), &payment_mint),
            share_mint: share_mint_pda(asset_id),
            mint_auth: mint_auth_pda(asset_id),
            seller_share_account: investor_share_ata(asset_id, seller),
            offeror_share_account: investor_share_ata(asset_id, offeror),
            cpi_auth: marketplace_cpi_auth(),
            income: property_income_pda(asset_id),
            seller_checkpoint: property_checkpoint_pda(asset_id, seller),
            offeror_checkpoint: property_checkpoint_pda(asset_id, offeror),
            property_program: marketplace::PROPERTY_PROGRAM,
            payment_token_program: TOKEN_PROGRAM_ID,
            share_token_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn reject_offer_ix(
    seller: &Pubkey,
    id: u64,
    offeror: &Pubkey,
    nonce: u64,
    payment_mint: Pubkey,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::RejectOffer { id, nonce }.data(),
        marketplace::accounts::RejectOffer {
            seller: *seller,
            payer: *seller,
            share_listing: share_listing_pda(id),
            offeror: *offeror,
            offer: offer_pda(id, offeror),
            offer_rent_payer: *offeror,
            payment_mint,
            offer_vault: offer_vault_pda(id, offeror),
            vault_payment_account: payment_ata(&offer_vault_pda(id, offeror), &payment_mint),
            offeror_payment: payment_ata(offeror, &payment_mint),
            payment_token_program: TOKEN_PROGRAM_ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn cancel_offer_ix(offeror: &Pubkey, id: u64, payment_mint: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::CancelOffer {}.data(),
        marketplace::accounts::CancelOffer {
            offeror: *offeror,
            payer: *offeror,
            offer: offer_pda(id, offeror),
            offer_rent_payer: *offeror,
            payment_mint,
            offer_vault: offer_vault_pda(id, offeror),
            vault_payment_account: payment_ata(&offer_vault_pda(id, offeror), &payment_mint),
            offeror_payment: payment_ata(offeror, &payment_mint),
            payment_token_program: TOKEN_PROGRAM_ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn send_shares_ix(
    sender: &Pubkey,
    receiver: &Pubkey,
    asset_id: u64,
    amount: u32,
) -> Instruction {
    Instruction::new_with_bytes(
        mid(),
        &marketplace::instruction::SendPropertyShares { asset_id, amount }.data(),
        marketplace::accounts::SendShares {
            config: marketplace_config(),
            sender: *sender,
            payer: *sender,
            sender_role: role_pda(sender, Role::RealEstateInvestor),
            sender_compliance: compliance_pda(sender),
            receiver: *receiver,
            receiver_role: role_pda(receiver, Role::RealEstateInvestor),
            receiver_compliance: compliance_pda(receiver),
            listing: listing_pda(asset_id),
            property: property_pda(asset_id),
            sender_holding: holding_pda(asset_id, sender),
            receiver_holding: holding_pda(asset_id, receiver),
            share_mint: share_mint_pda(asset_id),
            mint_auth: mint_auth_pda(asset_id),
            sender_share_account: investor_share_ata(asset_id, sender),
            receiver_share_account: investor_share_ata(asset_id, receiver),
            cpi_auth: marketplace_cpi_auth(),
            income: property_income_pda(asset_id),
            sender_checkpoint: property_checkpoint_pda(asset_id, sender),
            receiver_checkpoint: property_checkpoint_pda(asset_id, receiver),
            property_program: marketplace::PROPERTY_PROGRAM,
            share_token_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

// --- property-program income seeding (settlement CPI tests) ---

/// Write the property program's income ledger with one tGBP stream that has
/// accrued `per_share` per share, so settlements have something to bank.
pub fn seed_income_stream(svm: &mut LiteSVM, per_share: u128) {
    let income = property::state::PropertyIncome {
        asset_id: 0,
        streams: vec![property::state::IncomeStream {
            mint: tgbp_mint(),
            per_share,
            dust: 0,
        }],
        rent_payer: Pubkey::new_unique(),
        bump: Pubkey::find_program_address(
            &[b"income", &0u64.to_le_bytes()],
            &marketplace::PROPERTY_PROGRAM,
        )
        .1,
    };
    use anchor_lang::AnchorSerialize;
    let mut data =
        <property::state::PropertyIncome as anchor_lang::Discriminator>::DISCRIMINATOR.to_vec();
    income.serialize(&mut data).unwrap();
    svm.set_account(
        property_income_pda(0),
        Account {
            lamports: 100_000_000,
            data,
            owner: marketplace::PROPERTY_PROGRAM,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

pub fn checkpoint_of(svm: &LiteSVM, owner: &Pubkey) -> property::state::IncomeCheckpoint {
    let acc = svm.get_account(&property_checkpoint_pda(0, owner)).unwrap();
    property::state::IncomeCheckpoint::try_deserialize(&mut acc.data.as_slice()).unwrap()
}
