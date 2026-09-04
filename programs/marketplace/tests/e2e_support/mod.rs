//! Scaffolding for the cross-program lifecycle tests: all four programs
//! in one LiteSVM, every config initialized through its real instruction,
//! plus the regions and property builders the marketplace common lacks.
//! XCAV sits at real associated token accounts here, since the regions and
//! property refund paths recreate them at the canonical address.

use crate::common::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address;
use anchor_spl::token::spl_token::state::Account as SplAccount;
use anchor_spl::token::ID as TOKEN_PROGRAM_ID;
use property::state::VoteChoice;
use regions::state::Vote;

pub const REGION: u16 = 1;
pub const TAX_BPS: u16 = 300;
pub const SELLER_FEE_BPS: u16 = 200;
pub const BUYER_FEE_BPS: u16 = 100;
pub const REGION_VOTING_PERIOD: i64 = 1_000;
pub const LOCATION_DEPOSIT: u64 = 50_000_000;
pub const AGENT_DEPOSIT: u64 = 200_000_000;
pub const AGENT_VOTING_TIME: i64 = 3_600;
pub const PROPOSAL_VOTING_TIME: i64 = 3_600;
pub const LOW_PROPOSAL: u64 = 100_000_000_000;
pub const HIGH_PROPOSAL: u64 = 1_000_000_000_000;
/// The 0.1% region bond at the 1,000 XCAV supply `set_mint` writes.
pub const REGION_BOND: u64 = 1_000_000_000;

pub fn rid() -> Pubkey {
    regions::id()
}
pub fn pid() -> Pubkey {
    property::id()
}

// --- regions PDAs ---

pub fn regions_config() -> Pubkey {
    Pubkey::find_program_address(&[regions::CONFIG_SEED], &rid()).0
}
pub fn regions_vault() -> Pubkey {
    Pubkey::find_program_address(&[regions::VAULT_SEED], &rid()).0
}
pub fn region_state_pda(region_id: u16) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::REGION_STATE_SEED, &region_id.to_le_bytes()],
        &rid(),
    )
    .0
}
pub fn region_proposal_pda(proposal_id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[regions::PROPOSAL_SEED, &proposal_id.to_le_bytes()],
        &rid(),
    )
    .0
}
pub fn region_vote_pda(proposal_id: u64, voter: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            regions::VOTE_SEED,
            &proposal_id.to_le_bytes(),
            voter.as_ref(),
        ],
        &rid(),
    )
    .0
}

// --- property PDAs ---

pub fn property_config() -> Pubkey {
    Pubkey::find_program_address(&[property::CONFIG_SEED], &pid()).0
}
pub fn property_vault() -> Pubkey {
    Pubkey::find_program_address(&[property::VAULT_SEED], &pid()).0
}
pub fn agent_pda(wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[property::AGENT_SEED, wallet.as_ref()], &pid()).0
}
pub fn letting_pda(asset_id: u64) -> Pubkey {
    Pubkey::find_program_address(&[property::LETTING_SEED, &asset_id.to_le_bytes()], &pid()).0
}
pub fn agent_candidacy_pda(asset_id: u64, round: u64, agent: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            property::AGENT_CANDIDATE_SEED,
            &asset_id.to_le_bytes(),
            &round.to_le_bytes(),
            agent.as_ref(),
        ],
        &pid(),
    )
    .0
}
pub fn agent_vote_pda(asset_id: u64, round: u64, voter: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            property::AGENT_VOTE_SEED,
            &asset_id.to_le_bytes(),
            &round.to_le_bytes(),
            voter.as_ref(),
        ],
        &pid(),
    )
    .0
}
pub fn income_vault_pda(asset_id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[property::INCOME_VAULT_SEED, &asset_id.to_le_bytes()],
        &pid(),
    )
    .0
}
pub fn income_vault_ata(asset_id: u64, mint: &Pubkey) -> Pubkey {
    get_associated_token_address(&income_vault_pda(asset_id), mint)
}
pub fn gov_proposal_pda(asset_id: u64, id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            property::PROPOSAL_SEED,
            &asset_id.to_le_bytes(),
            &id.to_le_bytes(),
        ],
        &pid(),
    )
    .0
}
pub fn gov_vote_pda(asset_id: u64, id: u64, voter: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            property::PROPOSAL_VOTE_SEED,
            &asset_id.to_le_bytes(),
            &id.to_le_bytes(),
            voter.as_ref(),
        ],
        &pid(),
    )
    .0
}
pub fn property_cpi_auth() -> Pubkey {
    Pubkey::find_program_address(&[property::CPI_AUTH_SEED], &pid()).0
}

// --- token plumbing ---

/// The owner's real XCAV associated token account.
pub fn xcav_ata(owner: &Pubkey) -> Pubkey {
    get_associated_token_address(owner, &xcav_mint())
}

pub fn give_xcav_ata(svm: &mut LiteSVM, owner: &Pubkey, amount: u64) {
    set_token_account_for(svm, xcav_mint(), xcav_ata(owner), owner, amount);
}

/// Balance of any token account, zero when it does not exist.
pub fn balance_at(svm: &LiteSVM, address: &Pubkey) -> u64 {
    match svm.get_account(address) {
        Some(acc) if !acc.data.is_empty() => SplAccount::unpack(&acc.data).unwrap().amount,
        _ => 0,
    }
}

// --- config params ---

pub fn regions_params() -> regions::instructions::ConfigParams {
    regions::instructions::ConfigParams {
        minimum_voting_amount: 100_000_000,
        voting_period: REGION_VOTING_PERIOD,
        owner_change_period: 90 * 86_400,
        threshold_bps: 5_000,
        quorum: 100_000_000,
        notice_period: 7 * 86_400,
        min_vote_hold: 100,
        max_listing_duration: 1_000_000,
        max_tax_bps: 1_000,
        max_fee_bps: 1_000,
        location_deposit: LOCATION_DEPOSIT,
    }
}

pub fn property_params() -> property::ConfigParams {
    property::ConfigParams {
        treasury: treasury(),
        rent_collector: sponsor().pubkey(),
        agent_deposit: AGENT_DEPOSIT,
        agent_voting_time: AGENT_VOTING_TIME,
        min_voting_quorum_bps: 2_500,
        agent_notice_period: 86_400,
        proposal_voting_time: PROPOSAL_VOTING_TIME,
        low_proposal: LOW_PROPOSAL,
        high_proposal: HIGH_PROPOSAL,
        high_threshold_bps: 6_700,
        auto_approval_cooldown: 7 * 86_400,
        challenge_deposit: 50_000_000,
        agent_slash_amount: 50_000_000,
    }
}

// --- regions builders ---

pub fn regions_init_ix(authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::InitializeConfig {
            params: regions_params(),
        }
        .data(),
        regions::accounts::InitializeConfig {
            authority: *authority,
            program: rid(),
            program_data: program_data_pda(&rid()),
            config: regions_config(),
            xcav_mint: xcav_mint(),
            vault: regions_vault(),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn propose_region_ix(proposer: &Pubkey, region_id: u16, proposal_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::ProposeNewRegion {
            region_id,
            max_deposit: u64::MAX,
        }
        .data(),
        regions::accounts::ProposeNewRegion {
            proposer: *proposer,
            config: regions_config(),
            xcav_mint: xcav_mint(),
            proposer_token: xcav_ata(proposer),
            vault: regions_vault(),
            operator_role: role_pda(proposer, Role::RegionalOperator),
            region: region_pda(region_id),
            region_state: region_state_pda(region_id),
            proposal: region_proposal_pda(proposal_id),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn vote_region_ix(
    voter: &Pubkey,
    region_id: u16,
    proposal_id: u64,
    amount: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::VoteOnRegionProposal {
            region_id,
            vote: Vote::Yes,
            amount,
        }
        .data(),
        regions::accounts::VoteOnRegionProposal {
            voter: *voter,
            config: regions_config(),
            xcav_mint: xcav_mint(),
            voter_token: xcav_ata(voter),
            vault: regions_vault(),
            region_state: region_state_pda(region_id),
            proposal: region_proposal_pda(proposal_id),
            vote_record: region_vote_pda(proposal_id, voter),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn finalize_region_ix(
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
            vault: regions_vault(),
            region_state: region_state_pda(region_id),
            proposal: region_proposal_pda(proposal_id),
            proposer: *proposer,
            proposer_token: xcav_ata(proposer),
            token_program: TOKEN_PROGRAM_ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn create_region_ix(creator: &Pubkey, region_id: u16) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::CreateRegion {
            region_id,
            listing_duration: LISTING_DURATION,
            tax_bps: TAX_BPS,
            seller_fee_bps: SELLER_FEE_BPS,
            buyer_fee_bps: BUYER_FEE_BPS,
        }
        .data(),
        regions::accounts::CreateRegion {
            creator: *creator,
            config: regions_config(),
            creator_role: role_pda(creator, Role::RegionalOperator),
            region_state: region_state_pda(region_id),
            region: region_pda(region_id),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn unlock_region_vote_ix(voter: &Pubkey, proposal_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::UnlockVotingToken { proposal_id }.data(),
        regions::accounts::UnlockVotingToken {
            voter: *voter,
            config: regions_config(),
            xcav_mint: xcav_mint(),
            voter_token: xcav_ata(voter),
            vault: regions_vault(),
            vote_record: region_vote_pda(proposal_id, voter),
            token_program: TOKEN_PROGRAM_ID,
        }
        .to_account_metas(None),
    )
}

pub fn create_location_ix(operator: &Pubkey, region_id: u16, postcode: &[u8]) -> Instruction {
    Instruction::new_with_bytes(
        rid(),
        &regions::instruction::CreateNewLocation {
            region_id,
            postcode: postcode.to_vec(),
            max_deposit: u64::MAX,
        }
        .data(),
        regions::accounts::CreateNewLocation {
            operator: *operator,
            config: regions_config(),
            operator_role: role_pda(operator, Role::RegionalOperator),
            xcav_mint: xcav_mint(),
            operator_token: xcav_ata(operator),
            vault: regions_vault(),
            region: region_pda(region_id),
            location: location_pda(region_id, postcode),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

// --- property builders ---

pub fn property_init_ix(authority: &Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::InitializeConfig {
            params: property_params(),
        }
        .data(),
        property::accounts::InitializeConfig {
            authority: *authority,
            program: pid(),
            program_data: program_data_pda(&pid()),
            config: property_config(),
            xcav_mint: xcav_mint(),
            vault: property_vault(),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn add_agent_ix(agent: &Pubkey, region_id: u16, postcode: &[u8]) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::AddLettingAgent {
            region_id,
            postcode: postcode.to_vec(),
            max_deposit: u64::MAX,
        }
        .data(),
        property::accounts::AddLettingAgent {
            agent: *agent,
            config: property_config(),
            agent_role: role_pda(agent, Role::LettingAgent),
            location: location_pda(region_id, postcode),
            agent_entry: agent_pda(agent),
            xcav_mint: xcav_mint(),
            agent_token: xcav_ata(agent),
            vault: property_vault(),
            token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn claim_property_ix(agent: &Pubkey, asset_id: u64, round: u64) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::ClaimProperty { asset_id, round }.data(),
        property::accounts::ClaimProperty {
            agent: *agent,
            payer: *agent,
            config: property_config(),
            agent_role: role_pda(agent, Role::LettingAgent),
            agent_entry: agent_pda(agent),
            property: property_pda(asset_id),
            letting: letting_pda(asset_id),
            candidacy: agent_candidacy_pda(asset_id, round, agent),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn vote_agent_ix(
    voter: &Pubkey,
    asset_id: u64,
    round: u64,
    agent: &Pubkey,
    amount: u32,
) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::VoteOnAgent { asset_id, amount }.data(),
        property::accounts::VoteOnAgent {
            voter: *voter,
            payer: sponsor().pubkey(),
            voter_role: role_pda(voter, Role::RealEstateInvestor),
            letting: letting_pda(asset_id),
            holding: holding_pda(asset_id, voter),
            vote_record: agent_vote_pda(asset_id, round, voter),
            candidacy: agent_candidacy_pda(asset_id, round, agent),
            previous_candidacy: None,
            cpi_auth: property_cpi_auth(),
            marketplace_program: mid(),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn finalize_election_ix(
    cranker: &Pubkey,
    asset_id: u64,
    round: u64,
    winner: &Pubkey,
) -> Instruction {
    let mut accounts = property::accounts::FinalizeAgentElection {
        cranker: *cranker,
        letting: letting_pda(asset_id),
        property: property_pda(asset_id),
        winner_entry: Some(agent_pda(winner)),
    }
    .to_account_metas(None);
    accounts.push(AccountMeta::new(
        agent_candidacy_pda(asset_id, round, winner),
        false,
    ));
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::FinalizeAgentElection { asset_id }.data(),
        accounts,
    )
}

pub fn unlock_agent_votes_ix(voter: &Pubkey, asset_id: u64, round: u64) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::UnlockAgentVotes { asset_id, round }.data(),
        property::accounts::UnlockAgentVotes {
            voter: *voter,
            rent_payer: sponsor().pubkey(),
            letting: letting_pda(asset_id),
            holding: holding_pda(asset_id, voter),
            vote_record: agent_vote_pda(asset_id, round, voter),
            cpi_auth: property_cpi_auth(),
            marketplace_program: mid(),
        }
        .to_account_metas(None),
    )
}

/// Rent arrives in tGBP from the agent's deterministic marketplace-style
/// account, which `give_tgbp` funds.
pub fn distribute_ix(agent: &Pubkey, asset_id: u64, amount: u64) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::DistributeIncome { asset_id, amount }.data(),
        property::accounts::DistributeIncome {
            agent: *agent,
            payer: *agent,
            agent_role: role_pda(agent, Role::LettingAgent),
            letting: letting_pda(asset_id),
            property: property_pda(asset_id),
            market_config: marketplace_config(),
            income: property_income_pda(asset_id),
            payment_mint: tgbp_mint(),
            agent_payment: tgbp_acc(agent),
            income_vault: income_vault_pda(asset_id),
            vault_payment_account: income_vault_ata(asset_id, &tgbp_mint()),
            payment_token_program: TOKEN_PROGRAM_ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn claim_income_ix(holder: &Pubkey, asset_id: u64) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::ClaimIncome { asset_id }.data(),
        property::accounts::ClaimIncome {
            holder: *holder,
            payer: sponsor().pubkey(),
            income: property_income_pda(asset_id),
            checkpoint: property_checkpoint_pda(asset_id, holder),
            holding: holding_pda(asset_id, holder),
            payment_mint: tgbp_mint(),
            income_vault: income_vault_pda(asset_id),
            vault_payment_account: income_vault_ata(asset_id, &tgbp_mint()),
            holder_payment: tgbp_acc(holder),
            payment_token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn propose_ix(agent: &Pubkey, asset_id: u64, id: u64, amount: u64) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::Propose {
            asset_id,
            id,
            amount,
            details_hash: [1u8; 32],
        }
        .data(),
        property::accounts::Propose {
            agent: *agent,
            payer: *agent,
            config: property_config(),
            agent_role: role_pda(agent, Role::LettingAgent),
            letting: letting_pda(asset_id),
            proposal: gov_proposal_pda(asset_id, id),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn vote_proposal_ix(voter: &Pubkey, asset_id: u64, id: u64, amount: u32) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::VoteOnProposal {
            asset_id,
            choice: VoteChoice::Yes,
            amount,
        }
        .data(),
        property::accounts::VoteOnProposal {
            voter: *voter,
            payer: sponsor().pubkey(),
            voter_role: role_pda(voter, Role::RealEstateInvestor),
            letting: letting_pda(asset_id),
            proposal: gov_proposal_pda(asset_id, id),
            holding: holding_pda(asset_id, voter),
            vote_record: gov_vote_pda(asset_id, id, voter),
            cpi_auth: property_cpi_auth(),
            marketplace_program: mid(),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

pub fn finalize_proposal_ix(
    cranker: &Pubkey,
    agent: &Pubkey,
    asset_id: u64,
    id: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::FinalizeProposal { asset_id }.data(),
        property::accounts::FinalizeProposal {
            cranker: *cranker,
            rent_payer: *agent,
            letting: letting_pda(asset_id),
            property: property_pda(asset_id),
            proposal: gov_proposal_pda(asset_id, id),
        }
        .to_account_metas(None),
    )
}

pub fn unlock_proposal_votes_ix(voter: &Pubkey, asset_id: u64, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        pid(),
        &property::instruction::UnlockProposalVotes { asset_id, id }.data(),
        property::accounts::UnlockProposalVotes {
            voter: *voter,
            rent_payer: sponsor().pubkey(),
            proposal: gov_proposal_pda(asset_id, id),
            holding: holding_pda(asset_id, voter),
            vote_record: gov_vote_pda(asset_id, id, voter),
            cpi_auth: property_cpi_auth(),
            marketplace_program: mid(),
        }
        .to_account_metas(None),
    )
}

// --- readers ---

fn read<T: AccountDeserialize>(svm: &LiteSVM, address: &Pubkey) -> T {
    let acc = svm
        .get_account(address)
        .unwrap_or_else(|| panic!("{address} missing"));
    T::try_deserialize(&mut acc.data.as_slice()).unwrap()
}

pub fn next_region_proposal_id(svm: &LiteSVM) -> u64 {
    read::<regions::state::Config>(svm, &regions_config()).proposal_counter
}
pub fn region_of(svm: &LiteSVM, region_id: u16) -> regions::state::Region {
    read(svm, &region_pda(region_id))
}
pub fn letting_of(svm: &LiteSVM, asset_id: u64) -> property::state::PropertyLetting {
    read(svm, &letting_pda(asset_id))
}
pub fn agent_of(svm: &LiteSVM, wallet: &Pubkey) -> property::state::LettingAgent {
    read(svm, &agent_pda(wallet))
}
pub fn income_of(svm: &LiteSVM, asset_id: u64) -> property::state::PropertyIncome {
    read(svm, &property_income_pda(asset_id))
}

/// Whether a transaction's logs carry the event with this discriminator.
/// Event logs are base64; the first ten characters encode the eight bytes.
pub fn emitted(logs: &[String], discriminator: &[u8]) -> bool {
    const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u128;
    for &b in discriminator {
        bits = (bits << 8) | b as u128;
    }
    bits <<= 8;
    let prefix: String = (0..10)
        .map(|i| B64[((bits >> (66 - 6 * i)) & 63) as usize] as char)
        .collect();
    logs.iter().any(|l| {
        l.strip_prefix("Program data: ")
            .is_some_and(|d| d.starts_with(&prefix))
    })
}

/// Send and return the logs, panicking with the program error on failure.
pub fn logs_of(
    svm: &mut LiteSVM,
    ix: Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
) -> Vec<String> {
    match process(svm, ix, payer, signers) {
        Ok(meta) => meta.logs,
        Err(failed) => panic!("expected success, failed with: {:?}", failed.err),
    }
}

// --- the world ---

/// All four programs loaded, every config initialized through its real
/// instruction, the roles program bootstrapped with one admin. Returns
/// (svm, admin, authority). The clock starts at a real timestamp so the
/// programs' zero-means-never sentinels behave as on a cluster.
pub fn setup_all() -> (LiteSVM, Keypair, Keypair) {
    assert_eq!(
        marketplace::PROPERTY_PROGRAM,
        pid(),
        "marketplace::PROPERTY_PROGRAM is stale, update it after `anchor keys sync`"
    );
    let mut svm = LiteSVM::new();
    for (id, name) in [
        (roles_id(), "xcavate_whitelist"),
        (rid(), "regions"),
        (mid(), "marketplace"),
        (pid(), "property"),
    ] {
        svm.add_program(id, &program_bytes(name)).unwrap();
    }
    warp(&mut svm, 1_755_000_000);
    set_mint(&mut svm);
    set_token_account_for(
        &mut svm,
        xcav_mint(),
        payment_ata(&treasury(), &xcav_mint()),
        &treasury(),
        0,
    );

    let authority = funded(&mut svm);
    svm.airdrop(&sponsor().pubkey(), 100_000_000_000).unwrap();
    for id in [roles_id(), rid(), mid(), pid()] {
        bind_upgrade_authority(&mut svm, &id, &authority.pubkey());
    }
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
    for ix in [
        regions_init_ix(&authority.pubkey()),
        init_ix(&authority.pubkey()),
        property_init_ix(&authority.pubkey()),
    ] {
        ok(&mut svm, ix, &authority, &[&authority]);
    }
    (svm, admin, authority)
}

/// A SOL-funded wallet holding XCAV at its real ATA with the given role.
pub fn role_holder(svm: &mut LiteSVM, admin: &Keypair, role: Role) -> Keypair {
    let kp = funded(svm);
    give_xcav_ata(svm, &kp.pubkey(), FUND_XCAV);
    ok(
        svm,
        roles_assign_ix(&admin.pubkey(), &kp.pubkey(), role),
        admin,
        &[admin],
    );
    kp
}

/// Drives region 1 through propose, vote, finalize and create, owned by
/// the deterministic `region_operator` the marketplace builders name as
/// the fee payee, and registers `POSTCODE`. Returns the voter, whose
/// stake is still locked, and the proposal id.
pub fn live_region(svm: &mut LiteSVM, admin: &Keypair) -> (Keypair, u64) {
    let operator = region_operator();
    svm.airdrop(&operator.pubkey(), 100_000_000_000).unwrap();
    give_xcav_ata(svm, &operator.pubkey(), FUND_XCAV);
    ok(
        svm,
        roles_assign_ix(&admin.pubkey(), &operator.pubkey(), Role::RegionalOperator),
        admin,
        &[admin],
    );
    let proposal_id = next_region_proposal_id(svm);
    ok(
        svm,
        propose_region_ix(&operator.pubkey(), REGION, proposal_id),
        &operator,
        &[&operator],
    );
    let voter = funded(svm);
    give_xcav_ata(svm, &voter.pubkey(), FUND_XCAV);
    ok(
        svm,
        vote_region_ix(&voter.pubkey(), REGION, proposal_id, 200_000_000),
        &voter,
        &[&voter],
    );
    warp(svm, REGION_VOTING_PERIOD + 1);
    let cranker = funded(svm);
    ok(
        svm,
        finalize_region_ix(&cranker.pubkey(), REGION, proposal_id, &operator.pubkey()),
        &cranker,
        &[&cranker],
    );
    ok(
        svm,
        create_region_ix(&operator.pubkey(), REGION),
        &operator,
        &[&operator],
    );
    ok(
        svm,
        create_location_ix(&operator.pubkey(), REGION, POSTCODE),
        &operator,
        &[&operator],
    );
    (voter, proposal_id)
}
