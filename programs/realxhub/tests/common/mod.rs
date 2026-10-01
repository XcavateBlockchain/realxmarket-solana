//! Helpers for the hub journey. Region setup uses the existing region test
//! drivers, so role grants and region creation run through their real programs.
#![allow(dead_code, unused_imports)]

// Keep LiteSVM's failure metadata by value, matching the shared test drivers.
#[allow(clippy::result_large_err)]
#[path = "../../../regions/tests/common/mod.rs"]
pub mod region_common;

pub use anchor_lang::prelude::Pubkey;
pub use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::{clock::Clock, program_option::COption, program_pack::Pack};
pub use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token::state::{Account as SplAccount, AccountState, Mint as SplMint};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
pub use litesvm::LiteSVM;
pub use realxhub::state::{BuyerPosition, Config, Hub, HubStatus};
pub use realxhub::{ConfigParams, ProposalParams};
use realxhub::{
    BOND_VAULT_SEED, CONFIG_SEED, HUB_MINT_SEED, HUB_SEED, PAYMENT_VAULT_SEED, POSITION_SEED,
    TOKEN_VAULT_SEED,
};
use solana_account::Account;
pub use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
pub use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
pub use xcavate_whitelist::state::Role;

pub const SYS: Pubkey = anchor_lang::system_program::ID;
pub const TOKEN: Pubkey = anchor_spl::token::ID;
pub const ATA: Pubkey = anchor_spl::associated_token::ID;
pub const BOND: u64 = 100_000_000;
pub const PRICE: u64 = 200_000;
pub const SUPPLY: u64 = 10;
pub const DURATION: i64 = 100;
pub const PAYMENT_BALANCE: u64 = 10_000_000;

pub fn address(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &realxhub::id()).0
}
pub fn config() -> Pubkey {
    address(&[CONFIG_SEED])
}
pub fn hub(id: u64) -> Pubkey {
    address(&[HUB_SEED, &id.to_le_bytes()])
}
pub fn hub_mint(id: u64) -> Pubkey {
    address(&[HUB_MINT_SEED, &id.to_le_bytes()])
}
pub fn token_vault(id: u64) -> Pubkey {
    address(&[TOKEN_VAULT_SEED, &id.to_le_bytes()])
}
pub fn payment_vault(id: u64) -> Pubkey {
    address(&[PAYMENT_VAULT_SEED, &id.to_le_bytes()])
}
pub fn bond_vault(id: u64) -> Pubkey {
    address(&[BOND_VAULT_SEED, &id.to_le_bytes()])
}
pub fn position(id: u64, buyer: &Pubkey) -> Pubkey {
    address(&[POSITION_SEED, &id.to_le_bytes(), buyer.as_ref()])
}
pub fn payment_mint() -> Pubkey {
    Pubkey::new_from_array([88; 32])
}
pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    anchor_spl::associated_token::get_associated_token_address(owner, mint)
}
pub fn program_data() -> Pubkey {
    Pubkey::find_program_address(
        &[realxhub::id().as_ref()],
        &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
    )
    .0
}

pub struct Fixture {
    pub svm: LiteSVM,
    pub operator: Keypair,
    pub authority: Keypair,
    pub verifier: Keypair,
    pub admin: Keypair,
    pub buyer: Keypair,
}

pub fn params() -> ProposalParams {
    ProposalParams {
        name: "Farm cold storage".into(),
        metadata_uri: "https://example.com/hub.json".into(),
        metadata_hash: [1; 32],
        token_price: PRICE,
        token_supply: SUPPLY,
        sale_duration: DURATION,
    }
}

pub fn set_mint(svm: &mut LiteSVM, freeze_authority: COption<Pubkey>) {
    let mint = SplMint {
        mint_authority: COption::None,
        supply: 1_000_000_000,
        decimals: 6,
        is_initialized: true,
        freeze_authority,
    };
    let mut data = vec![0; SplMint::LEN];
    mint.pack_into_slice(&mut data);
    svm.set_account(
        payment_mint(),
        Account {
            lamports: 100_000_000,
            data,
            owner: TOKEN,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

pub fn set_payment(svm: &mut LiteSVM, owner: &Pubkey, amount: u64) {
    let token = SplAccount {
        mint: payment_mint(),
        owner: *owner,
        amount,
        delegate: COption::None,
        state: AccountState::Initialized,
        is_native: COption::None,
        delegated_amount: 0,
        close_authority: COption::None,
    };
    let mut data = vec![0; SplAccount::LEN];
    token.pack_into_slice(&mut data);
    svm.set_account(
        ata(owner, &payment_mint()),
        Account {
            lamports: 100_000_000,
            data,
            owner: TOKEN,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

pub fn setup_uninitialized() -> Fixture {
    let (mut svm, operator, authority) = region_common::setup();
    region_common::reach_created(&mut svm, &operator, &authority);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/realxhub.so");
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{} missing, run anchor build first", path.display()));
    svm.add_program(realxhub::id(), &bytes).unwrap();
    let mut pd = svm.get_account(&program_data()).unwrap();
    pd.data[12] = 1;
    pd.data[13..45].copy_from_slice(authority.pubkey().as_ref());
    svm.set_account(program_data(), pd).unwrap();
    set_mint(&mut svm, COption::None);
    let verifier = region_common::funded(&mut svm);
    let admin = region_common::funded(&mut svm);
    let buyer = region_common::funded(&mut svm);
    region_common::ok(
        &mut svm,
        region_common::roles_add_admin_ix(&authority.pubkey(), &admin.pubkey()),
        &authority,
        &[&authority],
    );
    region_common::ok(
        &mut svm,
        region_common::roles_assign_ix(&admin.pubkey(), &buyer.pubkey(), Role::RealEstateInvestor),
        &admin,
        &[&admin],
    );
    set_payment(&mut svm, &buyer.pubkey(), PAYMENT_BALANCE);
    Fixture {
        svm,
        operator,
        authority,
        verifier,
        admin,
        buyer,
    }
}

pub fn setup() -> Fixture {
    let mut f = setup_uninitialized();
    let ix = initialize_ix(&f.authority.pubkey(), f.verifier.pubkey());
    ok(&mut f.svm, ix, &f.authority);
    f
}

#[allow(clippy::result_large_err)]
pub fn process(
    svm: &mut LiteSVM,
    ix: Instruction,
    signer: &Keypair,
) -> Result<TransactionMetadata, FailedTransactionMetadata> {
    svm.expire_blockhash();
    let message =
        Message::new_with_blockhash(&[ix], Some(&signer.pubkey()), &svm.latest_blockhash());
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[signer]).unwrap();
    svm.send_transaction(tx)
}
pub fn ok(svm: &mut LiteSVM, ix: Instruction, signer: &Keypair) {
    if let Err(failed) = process(svm, ix, signer) {
        panic!(
            "expected success: {:?}\n{}",
            failed.err,
            failed.meta.logs.join("\n")
        );
    }
}
pub fn fails_with(svm: &mut LiteSVM, ix: Instruction, signer: &Keypair, expected: &str) {
    match process(svm, ix, signer) {
        Ok(_) => panic!("expected failure {expected}"),
        Err(failed) => {
            let detail = format!("{:?}\n{}", failed.err, failed.meta.logs.join("\n"));
            assert!(
                detail.contains(expected),
                "expected {expected}, got {detail}"
            );
        }
    }
}
pub fn hub_of(svm: &LiteSVM, id: u64) -> Hub {
    Hub::try_deserialize(&mut &svm.get_account(&hub(id)).unwrap().data[..]).unwrap()
}
pub fn position_of(svm: &LiteSVM, id: u64, buyer: &Pubkey) -> BuyerPosition {
    BuyerPosition::try_deserialize(&mut &svm.get_account(&position(id, buyer)).unwrap().data[..])
        .unwrap()
}
pub fn balance(svm: &LiteSVM, account: Pubkey) -> u64 {
    SplAccount::unpack(&svm.get_account(&account).unwrap().data)
        .unwrap()
        .amount
}
pub fn warp(svm: &mut LiteSVM, secs: i64) {
    region_common::warp(svm, secs);
}

pub fn initialize_ix(authority: &Pubkey, verifier: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::InitializeConfig {
            params: ConfigParams {
                verifier,
                bond_amount: BOND,
            },
        }
        .data(),
        realxhub::accounts::InitializeConfig {
            authority: *authority,
            program: realxhub::id(),
            program_data: program_data(),
            config: config(),
            xcav_mint: region_common::xcav_mint(),
            payment_mint: payment_mint(),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}
pub fn create_ix(operator: &Pubkey, id: u64, params: ProposalParams) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::CreateProposal {
            region_id: 1,
            params,
        }
        .data(),
        realxhub::accounts::CreateProposal {
            operator: *operator,
            config: config(),
            operator_role: region_common::role_pda(operator, Role::RegionalOperator),
            region: region_common::region_pda(1),
            hub: hub(id),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}
fn operator_accounts(operator: &Pubkey, id: u64) -> realxhub::accounts::OperatorProposal {
    realxhub::accounts::OperatorProposal {
        operator: *operator,
        hub: hub(id),
        operator_role: region_common::role_pda(operator, Role::RegionalOperator),
        region: region_common::region_pda(1),
    }
}
pub fn edit_ix(operator: &Pubkey, id: u64, params: ProposalParams) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::UpdateProposal { hub_id: id, params }.data(),
        operator_accounts(operator, id).to_account_metas(None),
    )
}
pub fn submit_ix(operator: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::SubmitProposal { hub_id: id }.data(),
        operator_accounts(operator, id).to_account_metas(None),
    )
}
pub fn review_ix(
    verifier: &Pubkey,
    id: u64,
    revision: u32,
    hash: [u8; 32],
    approve: bool,
) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::ReviewProposal {
            hub_id: id,
            revision,
            metadata_hash: hash,
            approve,
        }
        .data(),
        realxhub::accounts::ReviewProposal {
            verifier: *verifier,
            config: config(),
            hub: hub(id),
        }
        .to_account_metas(None),
    )
}
pub fn activate_ix(operator: &Pubkey, id: u64, max_bond: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::ActivateHub {
            hub_id: id,
            max_bond,
        }
        .data(),
        realxhub::accounts::ActivateHub {
            operator: *operator,
            config: config(),
            hub: hub(id),
            operator_role: region_common::role_pda(operator, Role::RegionalOperator),
            region: region_common::region_pda(1),
            xcav_mint: region_common::xcav_mint(),
            payment_mint: payment_mint(),
            operator_token: region_common::token_acc(operator),
            hub_mint: hub_mint(id),
            token_vault: token_vault(id),
            payment_vault: payment_vault(id),
            bond_vault: bond_vault(id),
            token_program: TOKEN,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}
pub fn buy_ix(buyer: &Pubkey, id: u64, amount: u64, max_total_cost: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::BuyTokens {
            hub_id: id,
            amount,
            max_total_cost,
        }
        .data(),
        realxhub::accounts::BuyTokens {
            buyer: *buyer,
            config: config(),
            hub: hub(id),
            buyer_role: region_common::role_pda(buyer, Role::RealEstateInvestor),
            payment_mint: payment_mint(),
            buyer_token: ata(buyer, &payment_mint()),
            payment_vault: payment_vault(id),
            position: position(id, buyer),
            token_program: TOKEN,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}
pub fn finalize_ix(cranker: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::FinalizeSale { hub_id: id }.data(),
        realxhub::accounts::FinalizeSale {
            cranker: *cranker,
            hub: hub(id),
        }
        .to_account_metas(None),
    )
}
pub fn claim_ix(buyer: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::ClaimTokens { hub_id: id }.data(),
        realxhub::accounts::ClaimTokens {
            buyer: *buyer,
            hub: hub(id),
            position: position(id, buyer),
            hub_mint: hub_mint(id),
            token_vault: token_vault(id),
            buyer_token: ata(buyer, &hub_mint(id)),
            token_program: TOKEN,
            associated_token_program: ATA,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}
pub fn refund_ix(buyer: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::RefundPurchase { hub_id: id }.data(),
        realxhub::accounts::RefundPurchase {
            buyer: *buyer,
            config: config(),
            hub: hub(id),
            position: position(id, buyer),
            payment_mint: payment_mint(),
            payment_vault: payment_vault(id),
            buyer_token: ata(buyer, &payment_mint()),
            token_program: TOKEN,
            associated_token_program: ATA,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}
pub fn bond_refund_ix(operator: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::RefundBond { hub_id: id }.data(),
        realxhub::accounts::RefundBond {
            operator: *operator,
            config: config(),
            hub: hub(id),
            xcav_mint: region_common::xcav_mint(),
            bond_vault: bond_vault(id),
            operator_token: region_common::token_acc(operator),
            token_program: TOKEN,
            associated_token_program: ATA,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}
pub fn reach_submitted(f: &mut Fixture, id: u64) {
    ok(
        &mut f.svm,
        create_ix(&f.operator.pubkey(), id, params()),
        &f.operator,
    );
    ok(&mut f.svm, submit_ix(&f.operator.pubkey(), id), &f.operator);
}
pub fn reach_approved(f: &mut Fixture, id: u64) {
    reach_submitted(f, id);
    ok(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), id, 1, [1; 32], true),
        &f.verifier,
    );
}
pub fn reach_listed(f: &mut Fixture, id: u64) {
    reach_approved(f, id);
    ok(
        &mut f.svm,
        activate_ix(&f.operator.pubkey(), id, BOND),
        &f.operator,
    );
}
