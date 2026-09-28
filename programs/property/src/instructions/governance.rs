use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{
    AGENT_SEED, CHALLENGE_SEED, CHALLENGE_VOTE_SEED, CONFIG_SEED, CPI_AUTH_SEED, LETTING_SEED,
    PROPOSAL_SEED, PROPOSAL_VOTE_SEED, VAULT_SEED,
};
use crate::error::PropertyError;
use crate::instructions::election::adjust_share_lock;
use crate::state::{
    Challenge, Config, GovVote, LettingAgent, PropertyLetting, Proposal, Tally, VoteChoice,
};
use crate::vault::{lock_to_vault, release_from_vault};

use marketplace::program::Marketplace;
use marketplace::state::{LockReason, PropertyAsset, ShareHolding};
use xcavate_whitelist::state::{Role, RoleAccount};

fn side(tally: &mut Tally, choice: VoteChoice) -> &mut u32 {
    match choice {
        VoteChoice::Yes => &mut tally.yes,
        VoteChoice::No => &mut tally.no,
        VoteChoice::Abstain => &mut tally.abstain,
    }
}

/// Apply a vote to the tally: a revote first takes the old power back out of
/// its side, then the new power lands on the chosen one.
fn cast_vote(tally: &mut Tally, record: &GovVote, choice: VoteChoice, amount: u32) -> Result<()> {
    if record.power > 0 {
        let old = side(tally, record.choice);
        *old = old
            .checked_sub(record.power)
            .ok_or(PropertyError::Overflow)?;
    }
    let new = side(tally, choice);
    *new = new.checked_add(amount).ok_or(PropertyError::Overflow)?;
    Ok(())
}

/// Yes beats no, and enough of the supply showed up. Abstentions count
/// toward the quorum only.
fn vote_passed(tally: &Tally, supply: u32, quorum_bps: u16) -> bool {
    let total = tally.yes as u64 + tally.no as u64 + tally.abstain as u64;
    total * 10_000 > supply as u64 * quorum_bps as u64 && tally.yes > tally.no
}

/// The assigned agent asks the holders to sign off on spending for the
/// property. The money itself sits off chain with the SPV, so approval is
/// the product here: the `ProposalExecuted` event is the authorization the
/// off-chain payment runs on. Requests at or under the low tier execute on
/// the spot, rate limited by the cooldown; everything else opens a
/// share-weighted vote.
#[derive(Accounts)]
#[instruction(asset_id: u64, id: u64)]
pub struct Propose<'info> {
    pub agent: Signer<'info>,

    /// Whoever fronts the rent: the agent on the default path, or any
    /// willing wallet. The proposal remembers who to refund.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The caller's LettingAgent role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            agent.key().as_ref(),
            &[Role::LettingAgent.seed_byte()],
        ],
        bump = agent_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub agent_role: Box<Account<'info, RoleAccount>>,

    /// The property's letting seat; only its assigned agent proposes.
    #[account(
        mut,
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
        constraint = letting.agent == agent.key() @ PropertyError::NotAssignedAgent,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    #[account(
        init,
        payer = payer,
        space = 8 + Proposal::INIT_SPACE,
        seeds = [PROPOSAL_SEED, &asset_id.to_le_bytes(), &id.to_le_bytes()],
        bump,
    )]
    pub proposal: Box<Account<'info, Proposal>>,

    pub system_program: Program<'info, System>,
}

pub fn propose_handler(
    ctx: Context<Propose>,
    asset_id: u64,
    id: u64,
    amount: u64,
    details_hash: [u8; 32],
) -> Result<()> {
    require!(amount > 0, PropertyError::ZeroAmount);
    require!(details_hash != [0u8; 32], PropertyError::InvalidDetailsHash);

    let config = &ctx.accounts.config;
    let gov = &mut ctx.accounts.letting.governance;
    require!(gov.active_proposal == 0, PropertyError::ProposalOngoing);
    require!(
        id == gov.proposal_count + 1,
        PropertyError::WrongGovernanceId
    );

    let now = Clock::get()?.unix_timestamp;
    let proposal = &mut ctx.accounts.proposal;
    if amount <= config.low_proposal {
        // Small enough to skip the vote, but not more often than the
        // cooldown allows. Nothing to keep around: the account closes
        // straight back to the payer and the id stays unconsumed.
        if gov.last_auto_approval_ts != 0 {
            require!(
                now > gov
                    .last_auto_approval_ts
                    .checked_add(config.auto_approval_cooldown)
                    .ok_or(PropertyError::Overflow)?,
                PropertyError::AutoApprovalTooSoon
            );
        }
        gov.last_auto_approval_ts = now;
        proposal.close(ctx.accounts.payer.to_account_info())?;

        emit!(ProposalExecuted { asset_id, amount });
        return Ok(());
    }

    gov.proposal_count = id;
    gov.active_proposal = id;

    proposal.asset_id = asset_id;
    proposal.id = id;
    proposal.proposer = ctx.accounts.agent.key();
    proposal.amount = amount;
    proposal.details_hash = details_hash;
    proposal.expiry = now
        .checked_add(config.proposal_voting_time)
        .ok_or(PropertyError::Overflow)?;
    proposal.quorum_bps = config.min_voting_quorum_bps;
    if amount >= config.high_proposal {
        proposal.threshold_bps = config.high_threshold_bps;
    }
    proposal.rent_payer = ctx.accounts.payer.key();
    proposal.bump = ctx.bumps.proposal;

    emit!(ProposalCreated {
        asset_id,
        id,
        proposer: proposal.proposer,
        amount,
        expiry: proposal.expiry,
    });
    Ok(())
}

/// Cast or change a vote on the running proposal, weighted by the shares put
/// behind it. The shares lock in the marketplace ShareHolding until
/// `unlock_proposal_votes`. Investor-role only; no compliance check, since
/// no money moves.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct VoteOnProposal<'info> {
    pub voter: Signer<'info>,

    /// Whoever fronts the vote record's rent: the sponsor on the default
    /// path, or any willing wallet, so one protocol key can never decide the
    /// vote by withholding its signature. The record remembers who to
    /// refund.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The caller's RealEstateInvestor role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            voter.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = voter_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub voter_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    /// The live proposal, pinned by the seat's active id.
    #[account(
        mut,
        seeds = [
            PROPOSAL_SEED,
            &asset_id.to_le_bytes(),
            &letting.governance.active_proposal.to_le_bytes(),
        ],
        bump = proposal.bump,
    )]
    pub proposal: Box<Account<'info, Proposal>>,

    /// The voter's share ledger, owned by the marketplace; the vote locks
    /// part of it through the CPI.
    #[account(
        mut,
        seeds = [marketplace::SHARE_SEED, &asset_id.to_le_bytes(), voter.key().as_ref()],
        bump = holding.bump,
        seeds::program = marketplace::ID,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    /// This proposal's vote record; revoting reuses it.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + GovVote::INIT_SPACE,
        seeds = [
            PROPOSAL_VOTE_SEED,
            &asset_id.to_le_bytes(),
            &letting.governance.active_proposal.to_le_bytes(),
            voter.key().as_ref(),
        ],
        bump,
    )]
    pub vote_record: Box<Account<'info, GovVote>>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs.
    #[account(seeds = [CPI_AUTH_SEED], bump)]
    pub cpi_auth: UncheckedAccount<'info>,

    pub marketplace_program: Program<'info, Marketplace>,
    pub system_program: Program<'info, System>,
}

pub fn vote_on_proposal_handler(
    ctx: Context<VoteOnProposal>,
    asset_id: u64,
    choice: VoteChoice,
    amount: u32,
) -> Result<()> {
    let proposal = &mut ctx.accounts.proposal;
    require!(
        Clock::get()?.unix_timestamp < proposal.expiry,
        PropertyError::VotingClosed
    );
    require!(amount > 0, PropertyError::InvalidVoteAmount);

    let record = &mut ctx.accounts.vote_record;
    cast_vote(&mut proposal.tally, record, choice, amount)?;

    // The marketplace moves the lock by the net difference and enforces
    // that everything locked still fits inside the holding.
    adjust_share_lock(
        &ctx.accounts.cpi_auth.to_account_info(),
        &ctx.accounts.holding.to_account_info(),
        ctx.bumps.cpi_auth,
        asset_id,
        ctx.accounts.voter.key(),
        LockReason::Proposal,
        record.power,
        amount,
    )?;

    // A revote leaves the recorded rent payer alone: the refund belongs to
    // whoever funded the account, not whoever last touched it.
    if record.rent_payer == Pubkey::default() {
        record.rent_payer = ctx.accounts.payer.key();
    }
    record.asset_id = asset_id;
    record.id = proposal.id;
    record.voter = ctx.accounts.voter.key();
    record.choice = choice;
    record.power = amount;
    record.bump = ctx.bumps.vote_record;

    emit!(ProposalVoteCast {
        asset_id,
        id: proposal.id,
        voter: record.voter,
        choice,
        power: amount,
    });
    Ok(())
}

/// Settle the proposal vote once it closes. Permissionless. Approval fires
/// the `ProposalExecuted` event that authorizes the off-chain payment;
/// either way the proposal closes and frees the slot.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct FinalizeProposal<'info> {
    pub cranker: Signer<'info>,

    /// CHECK: the wallet that fronted the proposal's rent; gets it back as
    /// the proposal closes.
    #[account(mut, address = proposal.rent_payer @ PropertyError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    /// The property, owned by the marketplace; supplies the share supply the
    /// quorum is measured against.
    #[account(
        seeds = [marketplace::PROPERTY_SEED, &asset_id.to_le_bytes()],
        bump = property.bump,
        seeds::program = marketplace::ID,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    #[account(
        mut,
        seeds = [
            PROPOSAL_SEED,
            &asset_id.to_le_bytes(),
            &letting.governance.active_proposal.to_le_bytes(),
        ],
        bump = proposal.bump,
    )]
    pub proposal: Box<Account<'info, Proposal>>,
}

pub fn finalize_proposal_handler(ctx: Context<FinalizeProposal>, asset_id: u64) -> Result<()> {
    let proposal = &mut ctx.accounts.proposal;
    require!(
        Clock::get()?.unix_timestamp >= proposal.expiry,
        PropertyError::VotingStillOngoing
    );

    let id = proposal.id;
    let amount = proposal.amount;
    let tally = proposal.tally;
    let supply = ctx.accounts.property.share_amount;
    // The high tier additionally needs its snapshotted share of the decided
    // (yes + no) votes; below it the threshold is zero and always met.
    let decided = tally.yes as u64 + tally.no as u64;
    let meets_threshold = tally.yes as u64 * 10_000 >= decided * proposal.threshold_bps as u64;
    let approved = vote_passed(&tally, supply, proposal.quorum_bps) && meets_threshold;

    ctx.accounts.letting.governance.active_proposal = 0;
    proposal.close(ctx.accounts.rent_payer.to_account_info())?;

    emit!(ProposalFinalized {
        asset_id,
        id,
        approved,
        yes: tally.yes,
        no: tally.no,
        abstain: tally.abstain,
    });
    if approved {
        emit!(ProposalExecuted { asset_id, amount });
    }
    Ok(())
}

/// Release the shares a proposal vote locked, once that vote can no longer
/// use them. Closes the vote record, returning its rent to whoever fronted
/// it. Not role-gated: this is a pure exit.
#[derive(Accounts)]
#[instruction(asset_id: u64, id: u64)]
pub struct UnlockProposalVotes<'info> {
    pub voter: Signer<'info>,

    /// CHECK: the wallet that fronted the record's rent; gets it back as the
    /// record closes.
    #[account(mut, address = vote_record.rent_payer @ PropertyError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    /// CHECK: the proposal the vote belongs to; the handler derives the
    /// expected address and inspects it directly, since a finalized
    /// proposal is already closed.
    pub proposal: UncheckedAccount<'info>,

    /// The voter's share ledger, owned by the marketplace; the unlock CPI
    /// releases the vote's shares from it.
    #[account(
        mut,
        seeds = [marketplace::SHARE_SEED, &asset_id.to_le_bytes(), voter.key().as_ref()],
        bump = holding.bump,
        seeds::program = marketplace::ID,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [
            PROPOSAL_VOTE_SEED,
            &asset_id.to_le_bytes(),
            &id.to_le_bytes(),
            voter.key().as_ref(),
        ],
        bump = vote_record.bump,
    )]
    pub vote_record: Box<Account<'info, GovVote>>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs.
    #[account(seeds = [CPI_AUTH_SEED], bump)]
    pub cpi_auth: UncheckedAccount<'info>,

    pub marketplace_program: Program<'info, Marketplace>,
}

pub fn unlock_proposal_votes_handler<'info>(
    ctx: Context<'info, UnlockProposalVotes<'info>>,
    asset_id: u64,
    id: u64,
) -> Result<()> {
    let expected = Pubkey::find_program_address(
        &[PROPOSAL_SEED, &asset_id.to_le_bytes(), &id.to_le_bytes()],
        &crate::ID,
    )
    .0;
    require!(
        ctx.accounts.proposal.key() == expected,
        PropertyError::WrongGovernanceId
    );
    // Locked only while this exact proposal is still collecting votes. The
    // tallies live on the proposal, so releasing after expiry can't change
    // the outcome.
    if !ctx.accounts.proposal.data_is_empty() {
        let proposal: Account<Proposal> = Account::try_from(&ctx.accounts.proposal)?;
        require!(
            Clock::get()?.unix_timestamp >= proposal.expiry,
            PropertyError::VotingStillOngoing
        );
    }

    let power = ctx.accounts.vote_record.power;
    adjust_share_lock(
        &ctx.accounts.cpi_auth.to_account_info(),
        &ctx.accounts.holding.to_account_info(),
        ctx.bumps.cpi_auth,
        asset_id,
        ctx.accounts.voter.key(),
        LockReason::Proposal,
        power,
        0,
    )?;

    emit!(ProposalVotesUnlocked {
        asset_id,
        id,
        voter: ctx.accounts.voter.key(),
        power,
    });
    Ok(())
}

/// A holder moves to strike the sitting agent, staking XCAV on the outcome.
/// Passing costs the agent a slice of their location deposit and a strike;
/// failing costs the challenger the stake. Investor-role only.
#[derive(Accounts)]
#[instruction(asset_id: u64, id: u64)]
pub struct ChallengeAgent<'info> {
    pub challenger: Signer<'info>,

    /// Whoever fronts the rent; the challenger on the default path. The
    /// challenge remembers who to refund.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The caller's RealEstateInvestor role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            challenger.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = challenger_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub challenger_role: Box<Account<'info, RoleAccount>>,

    /// The caller's share ledger, owned by the marketplace; only holders
    /// challenge.
    #[account(
        seeds = [marketplace::SHARE_SEED, &asset_id.to_le_bytes(), challenger.key().as_ref()],
        bump = holding.bump,
        seeds::program = marketplace::ID,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    #[account(
        mut,
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    #[account(
        init,
        payer = payer,
        space = 8 + Challenge::INIT_SPACE,
        seeds = [CHALLENGE_SEED, &asset_id.to_le_bytes(), &id.to_le_bytes()],
        bump,
    )]
    pub challenge: Box<Account<'info, Challenge>>,

    #[account(address = config.xcav_mint @ PropertyError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The challenger's XCAV account the stake leaves.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = challenger,
    )]
    pub challenger_token: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        token::mint = config.xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn challenge_agent_handler(
    ctx: Context<ChallengeAgent>,
    asset_id: u64,
    id: u64,
    max_deposit: u64,
) -> Result<()> {
    require!(ctx.accounts.holding.amount > 0, PropertyError::NotAHolder);
    let agent = ctx.accounts.letting.agent;
    require!(agent != Pubkey::default(), PropertyError::SeatVacant);

    let gov = &mut ctx.accounts.letting.governance;
    require!(gov.active_challenge == 0, PropertyError::ChallengeOngoing);
    require!(
        id == gov.challenge_count + 1,
        PropertyError::WrongGovernanceId
    );
    gov.challenge_count = id;
    gov.active_challenge = id;

    // The stake is read from live config; the caller caps what they are
    // willing to pay so an update can't reprice their signed transaction.
    let config = &ctx.accounts.config;
    let deposit = config.challenge_deposit;
    require!(deposit <= max_deposit, PropertyError::DepositTooHigh);
    lock_to_vault(
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.challenger_token.to_account_info(),
        &ctx.accounts.xcav_mint.to_account_info(),
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.challenger.to_account_info(),
        deposit,
        ctx.accounts.xcav_mint.decimals,
    )?;

    let challenge = &mut ctx.accounts.challenge;
    challenge.asset_id = asset_id;
    challenge.id = id;
    challenge.challenger = ctx.accounts.challenger.key();
    challenge.agent = agent;
    challenge.deposit = deposit;
    challenge.expiry = Clock::get()?
        .unix_timestamp
        .checked_add(config.proposal_voting_time)
        .ok_or(PropertyError::Overflow)?;
    challenge.quorum_bps = config.min_voting_quorum_bps;
    challenge.rent_payer = ctx.accounts.payer.key();
    challenge.bump = ctx.bumps.challenge;

    emit!(ChallengeOpened {
        asset_id,
        id,
        challenger: challenge.challenger,
        agent,
        deposit,
        expiry: challenge.expiry,
    });
    Ok(())
}

/// Cast or change a vote on the running challenge, weighted by the shares
/// put behind it. Yes backs the challenger; the shares lock until
/// `unlock_challenge_votes`. Investor-role only.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct VoteOnChallenge<'info> {
    pub voter: Signer<'info>,

    /// Whoever fronts the vote record's rent: the sponsor on the default
    /// path, or any willing wallet. The record remembers who to refund.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The caller's RealEstateInvestor role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            voter.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = voter_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub voter_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    /// The live challenge, pinned by the seat's active id.
    #[account(
        mut,
        seeds = [
            CHALLENGE_SEED,
            &asset_id.to_le_bytes(),
            &letting.governance.active_challenge.to_le_bytes(),
        ],
        bump = challenge.bump,
    )]
    pub challenge: Box<Account<'info, Challenge>>,

    /// The voter's share ledger, owned by the marketplace; the vote locks
    /// part of it through the CPI.
    #[account(
        mut,
        seeds = [marketplace::SHARE_SEED, &asset_id.to_le_bytes(), voter.key().as_ref()],
        bump = holding.bump,
        seeds::program = marketplace::ID,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    /// This challenge's vote record; revoting reuses it.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + GovVote::INIT_SPACE,
        seeds = [
            CHALLENGE_VOTE_SEED,
            &asset_id.to_le_bytes(),
            &letting.governance.active_challenge.to_le_bytes(),
            voter.key().as_ref(),
        ],
        bump,
    )]
    pub vote_record: Box<Account<'info, GovVote>>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs.
    #[account(seeds = [CPI_AUTH_SEED], bump)]
    pub cpi_auth: UncheckedAccount<'info>,

    pub marketplace_program: Program<'info, Marketplace>,
    pub system_program: Program<'info, System>,
}

pub fn vote_on_challenge_handler(
    ctx: Context<VoteOnChallenge>,
    asset_id: u64,
    choice: VoteChoice,
    amount: u32,
) -> Result<()> {
    let challenge = &mut ctx.accounts.challenge;
    require!(
        Clock::get()?.unix_timestamp < challenge.expiry,
        PropertyError::VotingClosed
    );
    require!(amount > 0, PropertyError::InvalidVoteAmount);

    let record = &mut ctx.accounts.vote_record;
    cast_vote(&mut challenge.tally, record, choice, amount)?;

    adjust_share_lock(
        &ctx.accounts.cpi_auth.to_account_info(),
        &ctx.accounts.holding.to_account_info(),
        ctx.bumps.cpi_auth,
        asset_id,
        ctx.accounts.voter.key(),
        LockReason::Challenge,
        record.power,
        amount,
    )?;

    if record.rent_payer == Pubkey::default() {
        record.rent_payer = ctx.accounts.payer.key();
    }
    record.asset_id = asset_id;
    record.id = challenge.id;
    record.voter = ctx.accounts.voter.key();
    record.choice = choice;
    record.power = amount;
    record.bump = ctx.bumps.vote_record;

    emit!(ChallengeVoteCast {
        asset_id,
        id: challenge.id,
        voter: record.voter,
        choice,
        power: amount,
    });
    Ok(())
}

/// Settle the challenge once voting closes. Permissionless. A passed
/// challenge slashes the agent's location deposit to the treasury, adds a
/// strike (three remove them from the seat), and returns the challenger's
/// stake; a failed one forfeits the stake to the treasury. An agent who
/// already left the seat is past punishing, but the challenger still gets
/// their stake back: the challenge got what it wanted.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct FinalizeChallenge<'info> {
    #[account(mut)]
    pub cranker: Signer<'info>,

    /// CHECK: the wallet that fronted the challenge's rent; gets it back as
    /// the challenge closes.
    #[account(mut, address = challenge.rent_payer @ PropertyError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    /// The property, owned by the marketplace; supplies the share supply the
    /// quorum is measured against and the location whose deposit is slashed.
    #[account(
        seeds = [marketplace::PROPERTY_SEED, &asset_id.to_le_bytes()],
        bump = property.bump,
        seeds::program = marketplace::ID,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    #[account(
        mut,
        seeds = [
            CHALLENGE_SEED,
            &asset_id.to_le_bytes(),
            &letting.governance.active_challenge.to_le_bytes(),
        ],
        bump = challenge.bump,
    )]
    pub challenge: Box<Account<'info, Challenge>>,

    /// The struck agent's registry entry, carrying the deposit to slash.
    /// Required while they still hold the seat; an agent already gone is
    /// left alone. The handler derives the expected address and checks it,
    /// since the account is only needed on that path.
    #[account(mut)]
    pub agent_entry: Option<Box<Account<'info, LettingAgent>>>,

    #[account(address = config.xcav_mint @ PropertyError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        token::mint = config.xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: the treasury owner key from config.
    #[account(address = config.treasury @ PropertyError::InvalidConfig)]
    pub treasury: UncheckedAccount<'info>,

    /// Any XCAV account the treasury owns.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = treasury,
    )]
    pub treasury_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: the challenger, from the challenge record.
    #[account(address = challenge.challenger @ PropertyError::WrongRentPayer)]
    pub challenger: UncheckedAccount<'info>,

    /// Any XCAV account the challenger owns. Not pinned to the ATA, so a
    /// re-owned one can't strand the refund and freeze the seat's governance.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = challenger,
    )]
    pub challenger_token: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> FinalizeChallenge<'info> {
    /// Pay `amount` XCAV from the vault into `token`.
    fn pay_out(&self, token: &InterfaceAccount<'info, TokenAccount>, amount: u64) -> Result<()> {
        release_from_vault(
            &self.token_program.to_account_info(),
            &self.vault.to_account_info(),
            &self.xcav_mint.to_account_info(),
            &token.to_account_info(),
            &self.config.to_account_info(),
            self.config.bump,
            amount,
            self.xcav_mint.decimals,
        )
    }
}

pub fn finalize_challenge_handler(ctx: Context<FinalizeChallenge>, asset_id: u64) -> Result<()> {
    let challenge = &ctx.accounts.challenge;
    require!(
        Clock::get()?.unix_timestamp >= challenge.expiry,
        PropertyError::VotingStillOngoing
    );
    let id = challenge.id;
    let deposit = challenge.deposit;
    let struck_agent = challenge.agent;
    let passed = vote_passed(
        &challenge.tally,
        ctx.accounts.property.share_amount,
        challenge.quorum_bps,
    );
    let seated = ctx.accounts.letting.agent == struck_agent;

    let mut slashed = 0u64;
    let mut removed = false;
    if passed && seated {
        let slash_cap = ctx.accounts.config.agent_slash_amount;
        let strikes = ctx
            .accounts
            .letting
            .governance
            .strikes
            .checked_add(1)
            .ok_or(PropertyError::Overflow)?;
        let expected =
            Pubkey::find_program_address(&[AGENT_SEED, struck_agent.as_ref()], &crate::ID).0;
        let entry = ctx
            .accounts
            .agent_entry
            .as_mut()
            .ok_or(PropertyError::WrongAgent)?;
        require!(entry.key() == expected, PropertyError::WrongAgent);
        let location = entry
            .locations
            .iter_mut()
            .find(|l| l.postcode == ctx.accounts.property.location)
            .ok_or(PropertyError::NotInLocation)?;
        // The slash comes out of the recorded location deposit, so a later
        // removal refunds exactly what is left.
        slashed = slash_cap.min(location.deposit);
        location.deposit = location
            .deposit
            .checked_sub(slashed)
            .ok_or(PropertyError::Overflow)?;
        if strikes >= 3 {
            location.assigned_count = location
                .assigned_count
                .checked_sub(1)
                .ok_or(PropertyError::Overflow)?;
            removed = true;
        }

        if removed {
            ctx.accounts.letting.agent = Pubkey::default();
            ctx.accounts.letting.governance.strikes = 0;
        } else {
            ctx.accounts.letting.governance.strikes = strikes;
        }
        if slashed > 0 {
            ctx.accounts
                .pay_out(&ctx.accounts.treasury_token, slashed)?;
        }
    }
    if passed {
        // The stake goes back whether the agent was punished or had already
        // left the seat.
        ctx.accounts
            .pay_out(&ctx.accounts.challenger_token, deposit)?;
    } else {
        ctx.accounts
            .pay_out(&ctx.accounts.treasury_token, deposit)?;
    }

    ctx.accounts.letting.governance.active_challenge = 0;
    ctx.accounts
        .challenge
        .close(ctx.accounts.rent_payer.to_account_info())?;

    emit!(ChallengeFinalized {
        asset_id,
        id,
        passed,
        slashed,
        removed,
    });
    Ok(())
}

/// Release the shares a challenge vote locked, once that vote can no longer
/// use them. Closes the vote record, returning its rent to whoever fronted
/// it. Not role-gated: this is a pure exit.
#[derive(Accounts)]
#[instruction(asset_id: u64, id: u64)]
pub struct UnlockChallengeVotes<'info> {
    pub voter: Signer<'info>,

    /// CHECK: the wallet that fronted the record's rent; gets it back as the
    /// record closes.
    #[account(mut, address = vote_record.rent_payer @ PropertyError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    /// CHECK: the challenge the vote belongs to; the handler derives the
    /// expected address and inspects it directly, since a settled challenge
    /// is already closed.
    pub challenge: UncheckedAccount<'info>,

    /// The voter's share ledger, owned by the marketplace; the unlock CPI
    /// releases the vote's shares from it.
    #[account(
        mut,
        seeds = [marketplace::SHARE_SEED, &asset_id.to_le_bytes(), voter.key().as_ref()],
        bump = holding.bump,
        seeds::program = marketplace::ID,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [
            CHALLENGE_VOTE_SEED,
            &asset_id.to_le_bytes(),
            &id.to_le_bytes(),
            voter.key().as_ref(),
        ],
        bump = vote_record.bump,
    )]
    pub vote_record: Box<Account<'info, GovVote>>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs.
    #[account(seeds = [CPI_AUTH_SEED], bump)]
    pub cpi_auth: UncheckedAccount<'info>,

    pub marketplace_program: Program<'info, Marketplace>,
}

pub fn unlock_challenge_votes_handler<'info>(
    ctx: Context<'info, UnlockChallengeVotes<'info>>,
    asset_id: u64,
    id: u64,
) -> Result<()> {
    let expected = Pubkey::find_program_address(
        &[CHALLENGE_SEED, &asset_id.to_le_bytes(), &id.to_le_bytes()],
        &crate::ID,
    )
    .0;
    require!(
        ctx.accounts.challenge.key() == expected,
        PropertyError::WrongGovernanceId
    );
    // Locked only while this exact challenge is still collecting votes.
    if !ctx.accounts.challenge.data_is_empty() {
        let challenge: Account<Challenge> = Account::try_from(&ctx.accounts.challenge)?;
        require!(
            Clock::get()?.unix_timestamp >= challenge.expiry,
            PropertyError::VotingStillOngoing
        );
    }

    let power = ctx.accounts.vote_record.power;
    adjust_share_lock(
        &ctx.accounts.cpi_auth.to_account_info(),
        &ctx.accounts.holding.to_account_info(),
        ctx.bumps.cpi_auth,
        asset_id,
        ctx.accounts.voter.key(),
        LockReason::Challenge,
        power,
        0,
    )?;

    emit!(ChallengeVotesUnlocked {
        asset_id,
        id,
        voter: ctx.accounts.voter.key(),
        power,
    });
    Ok(())
}

#[event]
pub struct ProposalCreated {
    pub asset_id: u64,
    pub id: u64,
    pub proposer: Pubkey,
    pub amount: u64,
    pub expiry: i64,
}

#[event]
pub struct ProposalVoteCast {
    pub asset_id: u64,
    pub id: u64,
    pub voter: Pubkey,
    pub choice: VoteChoice,
    pub power: u32,
}

#[event]
pub struct ProposalFinalized {
    pub asset_id: u64,
    pub id: u64,
    pub approved: bool,
    pub yes: u32,
    pub no: u32,
    pub abstain: u32,
}

/// The authorization the off-chain payment runs on: the SPV pays this
/// amount from the property's bank account.
#[event]
pub struct ProposalExecuted {
    pub asset_id: u64,
    pub amount: u64,
}

#[event]
pub struct ProposalVotesUnlocked {
    pub asset_id: u64,
    pub id: u64,
    pub voter: Pubkey,
    pub power: u32,
}

#[event]
pub struct ChallengeOpened {
    pub asset_id: u64,
    pub id: u64,
    pub challenger: Pubkey,
    pub agent: Pubkey,
    pub deposit: u64,
    pub expiry: i64,
}

#[event]
pub struct ChallengeVoteCast {
    pub asset_id: u64,
    pub id: u64,
    pub voter: Pubkey,
    pub choice: VoteChoice,
    pub power: u32,
}

#[event]
pub struct ChallengeFinalized {
    pub asset_id: u64,
    pub id: u64,
    pub passed: bool,
    pub slashed: u64,
    pub removed: bool,
}

#[event]
pub struct ChallengeVotesUnlocked {
    pub asset_id: u64,
    pub id: u64,
    pub voter: Pubkey,
    pub power: u32,
}
