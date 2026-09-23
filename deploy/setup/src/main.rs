//! Bootstraps a freshly deployed realXmarket cluster: initializes the four
//! program configs, hands out the team roles, screens each actor for
//! compliance, walks one region through the proposal vote so a property can
//! actually list, and registers the test lawyers. Safe to re-run: every phase
//! checks on-chain state first and skips what already exists.
//!
//! Run through `deploy/deploy.sh`, which creates the keys and mints this
//! tool expects under `deploy/keys/`.

use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_rpc_client::rpc_client::RpcClient;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;

use regions::state::Vote;
use xcavate_whitelist::state::{ComplianceStatus, Role};

const SYS: Pubkey = anchor_lang::system_program::ID;
const TOKEN: Pubkey = anchor_spl::token::ID;
const ATA_PROGRAM: Pubkey = anchor_spl::associated_token::ID;

const XCAV: u64 = 1_000_000_000; // 9 decimals

/// How long a screening clears a wallet for. Sanctions lists change, so the
/// record is dated and re-running the bootstrap renews it.
const COMPLIANCE_PERIOD: i64 = 90 * 86_400;

const REGION_ID: u16 = 1;
const REGION_NAME: &str = "England";
const POSTCODES: [&[u8]; 2] = [b"SW1A1AA", b"M11AE"];
const LISTING_DURATION: i64 = 30 * 86_400;
const REGION_TAX_BPS: u16 = 300;
// UK launch fees: 2% seller, 1% buyer.
const REGION_SELLER_FEE_BPS: u16 = 200;
const REGION_BUYER_FEE_BPS: u16 = 100;

/// Devnet-friendly windows: long enough to click through, short enough to
/// not stall a demo.
fn regions_params() -> regions::instructions::ConfigParams {
    regions::instructions::ConfigParams {
        minimum_voting_amount: XCAV,
        voting_period: 45,
        owner_change_period: 90 * 86_400,
        threshold_bps: 5_000,
        quorum: 10 * XCAV,
        notice_period: 7 * 86_400,
        min_vote_hold: 5,
        max_listing_duration: 180 * 86_400,
        max_tax_bps: 1_000,
        max_fee_bps: 1_000,
        // 0.1% of supply: 100k XCAV at the 100M devnet mint.
        operator_bond_bps: 10,
        location_deposit: 50 * XCAV,
    }
}

fn marketplace_params(
    treasury: Pubkey,
    sponsor: Pubkey,
    payment_mints: Vec<Pubkey>,
) -> marketplace::instructions::ConfigParams {
    marketplace::instructions::ConfigParams {
        treasury,
        rent_sponsor: sponsor,
        accepted_payment_mints: payment_mints,
        listing_deposit: 100 * XCAV,
        lawyer_deposit: 100 * XCAV,
        min_property_shares: 2,
        max_property_shares: 100,
        // Net sale fees, primary and secondary, split 67/33 between
        // operator and treasury.
        operator_fee_share_bps: 6_700,
        // Full range so two test investors can buy a property out; tighten
        // via update_config when the frontend tests the cap.
        max_ownership_bps: 10_000,
        claiming_time: 600,
        legal_process_time: 86_400,
        lawyer_voting_time: 600,
        min_voting_quorum_bps: 1_000,
    }
}

fn property_params(treasury: Pubkey, sponsor: Pubkey) -> property::instructions::ConfigParams {
    // Governance amounts are quote units (9 decimals, GBP).
    const GBP: u64 = 1_000_000_000;
    property::instructions::ConfigParams {
        treasury,
        rent_sponsor: sponsor,
        agent_deposit: 50 * XCAV,
        agent_voting_time: 600,
        min_voting_quorum_bps: 1_000,
        agent_notice_period: 3_600,
        proposal_voting_time: 600,
        low_proposal: 100 * GBP,
        high_proposal: 1_000 * GBP,
        high_threshold_bps: 6_700,
        auto_approval_cooldown: 600,
        challenge_deposit: 10 * XCAV,
        agent_slash_amount: 10 * XCAV,
    }
}

// --- PDAs ---

fn program_data(program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[program_id.as_ref()],
        &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
    )
    .0
}

fn roles_config() -> Pubkey {
    Pubkey::find_program_address(&[xcavate_whitelist::CONFIG_SEED], &xcavate_whitelist::ID).0
}
fn admin_pda(who: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[xcavate_whitelist::ADMIN_SEED, who.as_ref()],
        &xcavate_whitelist::ID,
    )
    .0
}
fn compliance_pda(user: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[xcavate_whitelist::COMPLIANCE_SEED, user.as_ref()],
        &xcavate_whitelist::ID,
    )
    .0
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

fn role_pda(user: &Pubkey, role: Role) -> Pubkey {
    Pubkey::find_program_address(
        &[
            xcavate_whitelist::ROLE_SEED,
            user.as_ref(),
            &[role.seed_byte()],
        ],
        &xcavate_whitelist::ID,
    )
    .0
}

fn regions_config() -> Pubkey {
    Pubkey::find_program_address(&[regions::CONFIG_SEED], &regions::ID).0
}
fn regions_vault() -> Pubkey {
    Pubkey::find_program_address(&[regions::VAULT_SEED], &regions::ID).0
}
fn region_pda(region_id: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::REGION_SEED, &region_id.to_le_bytes()],
        &regions::ID,
    )
    .0
}
fn region_state_pda(region_id: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::REGION_STATE_SEED, &region_id.to_le_bytes()],
        &regions::ID,
    )
    .0
}
fn proposal_pda(proposal_id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::PROPOSAL_SEED, &proposal_id.to_le_bytes()],
        &regions::ID,
    )
    .0
}
fn vote_pda(proposal_id: u64, voter: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            regions::VOTE_SEED,
            &proposal_id.to_le_bytes(),
            voter.as_ref(),
        ],
        &regions::ID,
    )
    .0
}
fn location_pda(region_id: u16, postcode: &[u8]) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::LOCATION_SEED, &region_id.to_le_bytes(), postcode],
        &regions::ID,
    )
    .0
}

fn marketplace_config() -> Pubkey {
    Pubkey::find_program_address(&[marketplace::CONFIG_SEED], &marketplace::ID).0
}
fn marketplace_vault() -> Pubkey {
    Pubkey::find_program_address(&[marketplace::VAULT_SEED], &marketplace::ID).0
}
fn lawyer_pda(wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[marketplace::LAWYER_SEED, wallet.as_ref()],
        &marketplace::ID,
    )
    .0
}

fn property_config() -> Pubkey {
    Pubkey::find_program_address(&[property::CONFIG_SEED], &property::ID).0
}
fn property_vault() -> Pubkey {
    Pubkey::find_program_address(&[property::VAULT_SEED], &property::ID).0
}

// --- plumbing ---

struct Ctx {
    rpc: RpcClient,
}

impl Ctx {
    fn exists(&self, address: &Pubkey) -> bool {
        self.rpc
            .get_account_with_commitment(address, CommitmentConfig::confirmed())
            .expect("rpc get_account")
            .value
            .is_some()
    }

    fn send(&self, label: &str, ix: Instruction, payer: &Keypair, signers: &[&Keypair]) {
        let blockhash = self.rpc.get_latest_blockhash().expect("blockhash");
        let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
        let tx =
            VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).expect("signing");
        let sig = self
            .rpc
            .send_and_confirm_transaction(&tx)
            .unwrap_or_else(|e| panic!("{label} failed: {e}"));
        println!("  ✓ {label} ({sig})");
    }
}

fn load_key(dir: &Path, name: &str) -> Keypair {
    solana_keypair::read_keypair_file(dir.join(format!("{name}.json")))
        .unwrap_or_else(|e| panic!("missing key {name}: {e}"))
}

fn main() {
    let url = std::env::args()
        .skip_while(|a| a != "--url")
        .nth(1)
        .unwrap_or_else(|| "https://api.devnet.solana.com".to_string());
    let keys = Path::new(env!("CARGO_MANIFEST_DIR")).join("../keys");
    let ctx = Ctx {
        rpc: RpcClient::new_with_commitment(url.clone(), CommitmentConfig::confirmed()),
    };
    println!("bootstrapping against {url}");

    let authority = load_key(&keys, "authority");
    let admin = load_key(&keys, "admin");
    let treasury = load_key(&keys, "treasury").pubkey();
    let sponsor_key = load_key(&keys, "sponsor");
    let sponsor = sponsor_key.pubkey();
    let operator = load_key(&keys, "operator");
    let developer = load_key(&keys, "developer");
    let investor1 = load_key(&keys, "investor1");
    let investor2 = load_key(&keys, "investor2");
    let lawyer1 = load_key(&keys, "lawyer1");
    let lawyer2 = load_key(&keys, "lawyer2");
    let spv_confirmer = load_key(&keys, "spv-confirmer");
    let letting_agent = load_key(&keys, "letting-agent");
    let xcav_mint = load_key(&keys, "xcav-mint").pubkey();
    let tgbp_mint = load_key(&keys, "tgbp-mint").pubkey();
    let tusdc_mint = load_key(&keys, "tusdc-mint").pubkey();

    // --- whitelist ---
    println!("whitelist");
    if ctx.exists(&roles_config()) {
        println!("  - config already initialized");
    } else {
        ctx.send(
            "initialize roles config",
            Instruction::new_with_bytes(
                xcavate_whitelist::ID,
                &xcavate_whitelist::instruction::InitializeConfig {}.data(),
                xcavate_whitelist::accounts::InitializeConfig {
                    authority: authority.pubkey(),
                    program: xcavate_whitelist::ID,
                    program_data: program_data(&xcavate_whitelist::ID),
                    config: roles_config(),
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            &authority,
            &[&authority],
        );
    }
    if ctx.exists(&admin_pda(&admin.pubkey())) {
        println!("  - admin already added");
    } else {
        ctx.send(
            "add admin",
            Instruction::new_with_bytes(
                xcavate_whitelist::ID,
                &xcavate_whitelist::instruction::AddAdmin {}.data(),
                xcavate_whitelist::accounts::AddAdmin {
                    authority: authority.pubkey(),
                    config: roles_config(),
                    new_admin: admin.pubkey(),
                    admin: admin_pda(&admin.pubkey()),
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            &authority,
            &[&authority],
        );
    }

    let assignments = [
        ("operator", operator.pubkey(), Role::RegionalOperator),
        ("developer", developer.pubkey(), Role::RealEstateDeveloper),
        ("investor1", investor1.pubkey(), Role::RealEstateInvestor),
        ("investor2", investor2.pubkey(), Role::RealEstateInvestor),
        ("lawyer1", lawyer1.pubkey(), Role::Lawyer),
        ("lawyer2", lawyer2.pubkey(), Role::Lawyer),
        (
            "spv-confirmer",
            spv_confirmer.pubkey(),
            Role::SpvConfirmation,
        ),
        ("letting-agent", letting_agent.pubkey(), Role::LettingAgent),
    ];
    for (name, user, role) in assignments {
        if ctx.exists(&role_pda(&user, role)) {
            println!("  - {name} role already assigned");
            continue;
        }
        ctx.send(
            &format!("assign {name} role"),
            Instruction::new_with_bytes(
                xcavate_whitelist::ID,
                &xcavate_whitelist::instruction::AssignRole { role }.data(),
                xcavate_whitelist::accounts::AssignRole {
                    admin_signer: admin.pubkey(),
                    admin: admin_pda(&admin.pubkey()),
                    user,
                    role_account: role_pda(&user, role),
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            &admin,
            &[&admin],
        );
    }

    // --- compliance ---
    //
    // Roles say what an actor may do, the compliance record says they are
    // cleared to move money. Every test actor gets one so a gate is never the
    // surprise, dated so devnet exercises the same re-screening the real
    // process needs.
    // Unlike the phases above this one always sends: `set_compliance` is an
    // upsert, so a re-run is how a lapsed record gets renewed.
    println!("compliance");
    let expires_at = now_unix() + COMPLIANCE_PERIOD;
    for (name, user, _) in assignments {
        ctx.send(
            &format!("clear {name}"),
            Instruction::new_with_bytes(
                xcavate_whitelist::ID,
                &xcavate_whitelist::instruction::SetCompliance {
                    status: ComplianceStatus::Cleared,
                    expires_at,
                }
                .data(),
                xcavate_whitelist::accounts::SetCompliance {
                    admin_signer: admin.pubkey(),
                    admin: admin_pda(&admin.pubkey()),
                    user,
                    compliance: compliance_pda(&user),
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            &admin,
            &[&admin],
        );
    }

    // --- program configs ---
    println!("configs");
    if ctx.exists(&regions_config()) {
        println!("  - regions config already initialized");
    } else {
        ctx.send(
            "initialize regions config",
            Instruction::new_with_bytes(
                regions::ID,
                &regions::instruction::InitializeConfig {
                    params: regions_params(),
                }
                .data(),
                regions::accounts::InitializeConfig {
                    authority: authority.pubkey(),
                    program: regions::ID,
                    program_data: program_data(&regions::ID),
                    config: regions_config(),
                    xcav_mint,
                    vault: regions_vault(),
                    token_program: TOKEN,
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            &authority,
            &[&authority],
        );
    }

    if ctx.exists(&marketplace_config()) {
        println!("  - marketplace config already initialized");
    } else {
        let params = marketplace_params(treasury, sponsor, vec![tgbp_mint, tusdc_mint]);
        let mut accounts = marketplace::accounts::InitializeConfig {
            authority: authority.pubkey(),
            program: marketplace::ID,
            program_data: program_data(&marketplace::ID),
            config: marketplace_config(),
            xcav_mint,
            vault: marketplace_vault(),
            token_program: TOKEN,
            system_program: SYS,
        }
        .to_account_metas(None);
        // Every accepted payment mint rides along for validation.
        for mint in &params.accepted_payment_mints {
            accounts.push(AccountMeta::new_readonly(*mint, false));
        }
        ctx.send(
            "initialize marketplace config",
            Instruction::new_with_bytes(
                marketplace::ID,
                &marketplace::instruction::InitializeConfig { params }.data(),
                accounts,
            ),
            &authority,
            &[&authority],
        );
    }

    if ctx.exists(&property_config()) {
        println!("  - property config already initialized");
    } else {
        ctx.send(
            "initialize property config",
            Instruction::new_with_bytes(
                property::ID,
                &property::instruction::InitializeConfig {
                    params: property_params(treasury, sponsor),
                }
                .data(),
                property::accounts::InitializeConfig {
                    authority: authority.pubkey(),
                    program: property::ID,
                    program_data: program_data(&property::ID),
                    config: property_config(),
                    xcav_mint,
                    vault: property_vault(),
                    token_program: TOKEN,
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            &authority,
            &[&authority],
        );
    }

    // --- region 1, proposed and voted through by the operator ---
    println!("region {REGION_ID}");
    if ctx.exists(&region_pda(REGION_ID)) {
        println!("  - region already live");
    } else {
        let operator_xcav = get_associated_token_address(&operator.pubkey(), &xcav_mint);
        if !ctx.exists(&region_state_pda(REGION_ID)) {
            ctx.send(
                "propose region",
                Instruction::new_with_bytes(
                    regions::ID,
                    &regions::instruction::ProposeNewRegion {
                        region_id: REGION_ID,
                        name: REGION_NAME.to_string(),
                        max_deposit: u64::MAX,
                    }
                    .data(),
                    regions::accounts::ProposeNewRegion {
                        proposer: operator.pubkey(),
                        config: regions_config(),
                        xcav_mint,
                        proposer_token: operator_xcav,
                        vault: regions_vault(),
                        operator_role: role_pda(&operator.pubkey(), Role::RegionalOperator),
                        region: region_pda(REGION_ID),
                        region_state: region_state_pda(REGION_ID),
                        proposal: {
                            let acc = ctx
                                .rpc
                                .get_account(&regions_config())
                                .expect("regions config");
                            let config =
                                regions::state::Config::try_deserialize(&mut acc.data.as_slice())
                                    .expect("regions config deser");
                            proposal_pda(config.proposal_counter)
                        },
                        token_program: TOKEN,
                        system_program: SYS,
                    }
                    .to_account_metas(None),
                ),
                &operator,
                &[&operator],
            );
        }

        let state_acc = ctx
            .rpc
            .get_account(&region_state_pda(REGION_ID))
            .expect("region state");
        let state = regions::state::RegionState::try_deserialize(&mut state_acc.data.as_slice())
            .expect("region state deser");
        assert!(
            state.status != regions::state::RegionStatus::Rejected,
            "region {REGION_ID} proposal was rejected; run clear_region_state and re-run"
        );
        let proposal_id = state.proposal_id;

        if state.status == regions::state::RegionStatus::Proposing {
            if !ctx.exists(&vote_pda(proposal_id, &operator.pubkey())) {
                ctx.send(
                    "vote yes",
                    Instruction::new_with_bytes(
                        regions::ID,
                        &regions::instruction::VoteOnRegionProposal {
                            region_id: REGION_ID,
                            vote: Vote::Yes,
                            amount: 100 * XCAV,
                        }
                        .data(),
                        regions::accounts::VoteOnRegionProposal {
                            voter: operator.pubkey(),
                            config: regions_config(),
                            xcav_mint,
                            voter_token: operator_xcav,
                            vault: regions_vault(),
                            region_state: region_state_pda(REGION_ID),
                            proposal: proposal_pda(proposal_id),
                            vote_record: vote_pda(proposal_id, &operator.pubkey()),
                            token_program: TOKEN,
                            system_program: SYS,
                        }
                        .to_account_metas(None),
                    ),
                    &operator,
                    &[&operator],
                );
            }

            let wait = regions_params().voting_period + 5;
            println!("  ... waiting {wait}s for the voting window");
            sleep(Duration::from_secs(wait as u64));
            ctx.send(
                "finalize proposal",
                Instruction::new_with_bytes(
                    regions::ID,
                    &regions::instruction::FinalizeRegionProposal {
                        region_id: REGION_ID,
                    }
                    .data(),
                    regions::accounts::FinalizeRegionProposal {
                        cranker: authority.pubkey(),
                        config: regions_config(),
                        xcav_mint,
                        vault: regions_vault(),
                        region_state: region_state_pda(REGION_ID),
                        proposal: proposal_pda(proposal_id),
                        proposer: operator.pubkey(),
                        proposer_token: operator_xcav,
                        token_program: TOKEN,
                        associated_token_program: ATA_PROGRAM,
                        system_program: SYS,
                    }
                    .to_account_metas(None),
                ),
                &authority,
                &[&authority],
            );
        }

        ctx.send(
            "claim region",
            Instruction::new_with_bytes(
                regions::ID,
                &regions::instruction::CreateRegion {
                    region_id: REGION_ID,
                    listing_duration: LISTING_DURATION,
                    tax_bps: REGION_TAX_BPS,
                    seller_fee_bps: REGION_SELLER_FEE_BPS,
                    buyer_fee_bps: REGION_BUYER_FEE_BPS,
                }
                .data(),
                regions::accounts::CreateRegion {
                    creator: operator.pubkey(),
                    config: regions_config(),
                    creator_role: role_pda(&operator.pubkey(), Role::RegionalOperator),
                    region_state: region_state_pda(REGION_ID),
                    region: region_pda(REGION_ID),
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            &operator,
            &[&operator],
        );
    }

    for postcode in POSTCODES {
        let label = String::from_utf8_lossy(postcode).to_string();
        if ctx.exists(&location_pda(REGION_ID, postcode)) {
            println!("  - location {label} already registered");
            continue;
        }
        ctx.send(
            &format!("register location {label}"),
            Instruction::new_with_bytes(
                regions::ID,
                &regions::instruction::CreateNewLocation {
                    region_id: REGION_ID,
                    postcode: postcode.to_vec(),
                    max_deposit: u64::MAX,
                }
                .data(),
                regions::accounts::CreateNewLocation {
                    operator: operator.pubkey(),
                    config: regions_config(),
                    operator_role: role_pda(&operator.pubkey(), Role::RegionalOperator),
                    xcav_mint,
                    operator_token: get_associated_token_address(&operator.pubkey(), &xcav_mint),
                    vault: regions_vault(),
                    region: region_pda(REGION_ID),
                    location: location_pda(REGION_ID, postcode),
                    token_program: TOKEN,
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            &operator,
            &[&operator],
        );
    }

    // --- lawyers join the registry ---
    println!("lawyers");
    for (name, lawyer) in [("lawyer1", &lawyer1), ("lawyer2", &lawyer2)] {
        if ctx.exists(&lawyer_pda(&lawyer.pubkey())) {
            println!("  - {name} already registered");
            continue;
        }
        // The sponsor fronts lawyer account rent; register_lawyer insists.
        ctx.send(
            &format!("register {name}"),
            Instruction::new_with_bytes(
                marketplace::ID,
                &marketplace::instruction::RegisterLawyer {
                    region_id: REGION_ID,
                    max_deposit: u64::MAX,
                }
                .data(),
                marketplace::accounts::RegisterLawyer {
                    lawyer: lawyer.pubkey(),
                    payer: sponsor,
                    config: marketplace_config(),
                    lawyer_role: role_pda(&lawyer.pubkey(), Role::Lawyer),
                    region: region_pda(REGION_ID),
                    lawyer_account: lawyer_pda(&lawyer.pubkey()),
                    xcav_mint,
                    lawyer_token: get_associated_token_address(&lawyer.pubkey(), &xcav_mint),
                    vault: marketplace_vault(),
                    token_program: TOKEN,
                    system_program: SYS,
                }
                .to_account_metas(None),
            ),
            lawyer,
            &[lawyer, &sponsor_key],
        );
    }

    // --- summary for the frontend team ---
    let addresses = format!(
        r#"{{
  "cluster": "{url}",
  "programs": {{
    "xcavate_whitelist": "{}",
    "regions": "{}",
    "marketplace": "{}",
    "property": "{}"
  }},
  "mints": {{
    "xcav": "{xcav_mint}",
    "tgbp": "{tgbp_mint}",
    "tusdc": "{tusdc_mint}"
  }},
  "configs": {{
    "whitelist": "{}",
    "regions": "{}",
    "marketplace": "{}",
    "property": "{}"
  }},
  "region": {{ "id": {REGION_ID}, "address": "{}", "postcodes": ["SW1A1AA", "M11AE"] }},
  "wallets": {{
    "authority": "{}",
    "admin": "{}",
    "treasury": "{treasury}",
    "sponsor": "{sponsor}",
    "operator": "{}",
    "developer": "{}",
    "investor1": "{}",
    "investor2": "{}",
    "lawyer1": "{}",
    "lawyer2": "{}",
    "spv_confirmer": "{}",
    "letting_agent": "{}"
  }}
}}"#,
        xcavate_whitelist::ID,
        regions::ID,
        marketplace::ID,
        property::ID,
        roles_config(),
        regions_config(),
        marketplace_config(),
        property_config(),
        region_pda(REGION_ID),
        authority.pubkey(),
        admin.pubkey(),
        operator.pubkey(),
        developer.pubkey(),
        investor1.pubkey(),
        investor2.pubkey(),
        lawyer1.pubkey(),
        lawyer2.pubkey(),
        spv_confirmer.pubkey(),
        letting_agent.pubkey(),
    );
    let out = keys.parent().unwrap().join("addresses.json");
    std::fs::write(&out, addresses).expect("write addresses.json");
    println!("done, addresses written to {}", out.display());
}
