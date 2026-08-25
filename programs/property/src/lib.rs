//! The realXmarket property program: letting agents manage rented-out
//! properties, rental income is distributed to shareholders through a
//! per-mint accumulator, and holders govern the property by share-weighted
//! vote. Agent deposits are staked in XCAV; income arrives in the accepted
//! payment mints.

pub mod constants;
pub mod error;
pub mod instructions;
pub mod mint_guard;
pub mod state;
pub mod vault;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::ConfigParams;

use instructions::*;
use state::VoteChoice;

declare_id!("deCp9srk9C6P4BXJaFpjR5H6Jsm6DCq8AL2kk338dVq");

#[program]
pub mod property {
    use super::*;

    pub fn initialize_config(ctx: Context<InitializeConfig>, params: ConfigParams) -> Result<()> {
        initialize::handler(ctx, params)
    }

    pub fn update_config(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
        initialize::update_config_handler(ctx, params)
    }

    pub fn update_authority(ctx: Context<UpdateAuthority>, new_authority: Pubkey) -> Result<()> {
        initialize::update_authority_handler(ctx, new_authority)
    }

    pub fn accept_authority(ctx: Context<AcceptAuthority>) -> Result<()> {
        initialize::accept_authority_handler(ctx)
    }

    pub fn add_letting_agent(
        ctx: Context<AddLettingAgent>,
        region_id: u16,
        postcode: Vec<u8>,
        max_deposit: u64,
    ) -> Result<()> {
        agents::add_letting_agent_handler(ctx, region_id, postcode, max_deposit)
    }

    pub fn remove_letting_agent(ctx: Context<RemoveLettingAgent>, postcode: Vec<u8>) -> Result<()> {
        agents::remove_letting_agent_handler(ctx, postcode)
    }

    pub fn claim_property(ctx: Context<ClaimProperty>, asset_id: u64, round: u64) -> Result<()> {
        election::claim_property_handler(ctx, asset_id, round)
    }

    pub fn vote_on_agent(ctx: Context<VoteOnAgent>, asset_id: u64, amount: u32) -> Result<()> {
        election::vote_on_agent_handler(ctx, asset_id, amount)
    }

    pub fn finalize_agent_election<'info>(
        ctx: Context<'info, FinalizeAgentElection<'info>>,
        asset_id: u64,
    ) -> Result<()> {
        election::finalize_agent_election_handler(ctx, asset_id)
    }

    pub fn close_agent_candidacy(
        ctx: Context<CloseAgentCandidacy>,
        asset_id: u64,
        round: u64,
        agent: Pubkey,
    ) -> Result<()> {
        election::close_agent_candidacy_handler(ctx, asset_id, round, agent)
    }

    pub fn unlock_agent_votes(
        ctx: Context<UnlockAgentVotes>,
        asset_id: u64,
        round: u64,
    ) -> Result<()> {
        election::unlock_agent_votes_handler(ctx, asset_id, round)
    }

    pub fn distribute_income<'info>(
        ctx: Context<'info, DistributeIncome<'info>>,
        asset_id: u64,
        amount: u64,
    ) -> Result<()> {
        income::distribute_income_handler(ctx, asset_id, amount)
    }

    pub fn claim_income<'info>(
        ctx: Context<'info, ClaimIncome<'info>>,
        asset_id: u64,
    ) -> Result<()> {
        income::claim_income_handler(ctx, asset_id)
    }

    pub fn settle_income(ctx: Context<SettleIncome>, asset_id: u64, owner: Pubkey) -> Result<()> {
        income::settle_income_handler(ctx, asset_id, owner)
    }

    pub fn close_income_checkpoint<'info>(
        ctx: Context<'info, CloseIncomeCheckpoint<'info>>,
        asset_id: u64,
    ) -> Result<()> {
        income::close_income_checkpoint_handler(ctx, asset_id)
    }

    pub fn resign(ctx: Context<Resign>, asset_id: u64) -> Result<()> {
        resignation::resign_handler(ctx, asset_id)
    }

    pub fn finalize_resignation(ctx: Context<FinalizeResignation>, asset_id: u64) -> Result<()> {
        resignation::finalize_resignation_handler(ctx, asset_id)
    }

    pub fn propose(
        ctx: Context<Propose>,
        asset_id: u64,
        id: u64,
        amount: u64,
        details_hash: [u8; 32],
    ) -> Result<()> {
        governance::propose_handler(ctx, asset_id, id, amount, details_hash)
    }

    pub fn vote_on_proposal(
        ctx: Context<VoteOnProposal>,
        asset_id: u64,
        choice: VoteChoice,
        amount: u32,
    ) -> Result<()> {
        governance::vote_on_proposal_handler(ctx, asset_id, choice, amount)
    }

    pub fn finalize_proposal(ctx: Context<FinalizeProposal>, asset_id: u64) -> Result<()> {
        governance::finalize_proposal_handler(ctx, asset_id)
    }

    pub fn unlock_proposal_votes<'info>(
        ctx: Context<'info, UnlockProposalVotes<'info>>,
        asset_id: u64,
        id: u64,
    ) -> Result<()> {
        governance::unlock_proposal_votes_handler(ctx, asset_id, id)
    }

    pub fn challenge_agent(
        ctx: Context<ChallengeAgent>,
        asset_id: u64,
        id: u64,
        max_deposit: u64,
    ) -> Result<()> {
        governance::challenge_agent_handler(ctx, asset_id, id, max_deposit)
    }

    pub fn vote_on_challenge(
        ctx: Context<VoteOnChallenge>,
        asset_id: u64,
        choice: VoteChoice,
        amount: u32,
    ) -> Result<()> {
        governance::vote_on_challenge_handler(ctx, asset_id, choice, amount)
    }

    pub fn finalize_challenge(ctx: Context<FinalizeChallenge>, asset_id: u64) -> Result<()> {
        governance::finalize_challenge_handler(ctx, asset_id)
    }

    pub fn unlock_challenge_votes<'info>(
        ctx: Context<'info, UnlockChallengeVotes<'info>>,
        asset_id: u64,
        id: u64,
    ) -> Result<()> {
        governance::unlock_challenge_votes_handler(ctx, asset_id, id)
    }
}
