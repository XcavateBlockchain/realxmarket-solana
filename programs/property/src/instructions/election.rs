use anchor_lang::prelude::*;

use crate::constants::{
    AGENT_CANDIDATE_SEED, AGENT_SEED, AGENT_VOTE_SEED, CONFIG_SEED, CPI_AUTH_SEED, LETTING_SEED,
};
use crate::error::PropertyError;
use crate::state::{
    AgentCandidacy, AgentVote, Config, LettingAgent, PropertyLetting, MAX_AGENT_CANDIDATES,
};

use marketplace::program::Marketplace;
use marketplace::state::{LockReason, PropertyAsset, ShareHolding};
use xcavate_whitelist::state::{Role, RoleAccount};

use xcavate_common::election::tally_plurality;

/// Adjust the voter's share lock in the marketplace by the difference between
/// their old and new vote, under the given reason. The lock lives on the
/// marketplace ShareHolding, where transfers check it, so the mirror goes
/// through a CPI signed by this program's `cpi-auth` PDA.
#[allow(clippy::too_many_arguments)]
pub(crate) fn adjust_share_lock<'info>(
    cpi_auth: &AccountInfo<'info>,
    holding: &AccountInfo<'info>,
    cpi_auth_bump: u8,
    asset_id: u64,
    owner: Pubkey,
    reason: LockReason,
    old_power: u32,
    new_power: u32,
) -> Result<()> {
    let bump = [cpi_auth_bump];
    let seeds: &[&[u8]] = &[CPI_AUTH_SEED, &bump];
    let signer_seeds = &[seeds];
    let ctx = CpiContext::new_with_signer(
        marketplace::ID,
        marketplace::cpi::accounts::AdjustShareLock {
            property_signer: cpi_auth.clone(),
            holding: holding.clone(),
        },
        signer_seeds,
    );
    if new_power > old_power {
        marketplace::cpi::lock_shares(ctx, asset_id, owner, reason, new_power - old_power)
    } else if old_power > new_power {
        marketplace::cpi::unlock_shares(ctx, asset_id, owner, reason, old_power - new_power)
    } else {
        Ok(())
    }
}

/// A registered letting agent stands for election on a finalized property.
/// The first candidacy opens the voting window; later ones join the same
/// round while it runs. LettingAgent-role only; the agent must cover the
/// property's location.
#[derive(Accounts)]
#[instruction(asset_id: u64, round: u64)]
pub struct ClaimProperty<'info> {
    pub agent: Signer<'info>,

    /// Whoever fronts the rent: the agent on the default path, or any
    /// willing wallet. The records remember who to refund.
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

    /// The caller's registry entry; proves registration and carries the
    /// locations they cover.
    #[account(
        seeds = [AGENT_SEED, agent.key().as_ref()],
        bump = agent_entry.bump,
    )]
    pub agent_entry: Box<Account<'info, LettingAgent>>,

    /// The property, owned by the marketplace.
    #[account(
        seeds = [marketplace::PROPERTY_SEED, &asset_id.to_le_bytes()],
        bump = property.bump,
        seeds::program = marketplace::ID,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    /// The property's letting seat; the first candidacy ever creates it.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + PropertyLetting::INIT_SPACE,
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    /// The candidacy, one per agent per round; `init` rejects standing twice.
    #[account(
        init,
        payer = payer,
        space = 8 + AgentCandidacy::INIT_SPACE,
        seeds = [
            AGENT_CANDIDATE_SEED,
            &asset_id.to_le_bytes(),
            &round.to_le_bytes(),
            agent.key().as_ref(),
        ],
        bump,
    )]
    pub candidacy: Box<Account<'info, AgentCandidacy>>,

    pub system_program: Program<'info, System>,
}

pub fn claim_property_handler(
    ctx: Context<ClaimProperty>,
    asset_id: u64,
    round: u64,
) -> Result<()> {
    require!(
        ctx.accounts.property.finalized,
        PropertyError::PropertyNotFinalized
    );

    let letting = &mut ctx.accounts.letting;
    if letting.rent_payer == Pubkey::default() {
        letting.asset_id = asset_id;
        letting.rent_payer = ctx.accounts.payer.key();
        letting.bump = ctx.bumps.letting;
    }
    require!(letting.agent == Pubkey::default(), PropertyError::SeatTaken);

    // The agent must cover the property's location; the entry's region and
    // location list are maintained by the registry instructions.
    let entry = &ctx.accounts.agent_entry;
    let property = &ctx.accounts.property;
    require!(
        entry.region_id == property.region_id,
        PropertyError::WrongRegion
    );
    require!(
        entry
            .locations
            .iter()
            .any(|l| l.postcode == property.location),
        PropertyError::NotInLocation
    );

    let now = Clock::get()?.unix_timestamp;
    let voting_time = ctx.accounts.config.agent_voting_time;
    let quorum_bps = ctx.accounts.config.min_voting_quorum_bps;
    let election = &mut letting.election;
    if election.expiry == 0 {
        // First candidacy: a new round opens and the clock starts. The
        // quorum is stamped now so a config change can't move it mid-vote.
        require!(
            round == election.round + 1,
            PropertyError::WrongElectionRound
        );
        election.round = round;
        election.expiry = now
            .checked_add(voting_time)
            .ok_or(PropertyError::Overflow)?;
        election.candidate_count = 0;
        election.quorum_bps = quorum_bps;
    } else {
        require!(round == election.round, PropertyError::WrongElectionRound);
        require!(now < election.expiry, PropertyError::VotingClosed);
    }
    election.candidate_count = election
        .candidate_count
        .checked_add(1)
        .ok_or(PropertyError::Overflow)?;
    require!(
        election.candidate_count <= MAX_AGENT_CANDIDATES,
        PropertyError::TooManyCandidates
    );

    let candidacy = &mut ctx.accounts.candidacy;
    candidacy.asset_id = asset_id;
    candidacy.round = round;
    candidacy.agent = ctx.accounts.agent.key();
    candidacy.vote_power = 0;
    candidacy.rent_payer = ctx.accounts.payer.key();
    candidacy.bump = ctx.bumps.candidacy;

    emit!(AgentCandidacyClaimed {
        asset_id,
        round,
        agent: candidacy.agent,
        expiry: letting.election.expiry,
    });
    Ok(())
}

/// Cast or change a vote in the running agent election, weighted by the
/// shares put behind it. Every vote backs a candidacy; a round that elects
/// nobody reopens for the next one. The shares lock in the marketplace
/// ShareHolding until `unlock_agent_votes`. Investor-role only; no
/// compliance check, since no money moves.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct VoteOnAgent<'info> {
    pub voter: Signer<'info>,

    /// Whoever fronts the vote record's rent: the sponsor on the default
    /// path, or any willing wallet, so one protocol key can never decide the
    /// election by withholding its signature. The record remembers who to
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

    /// The voter's share ledger, owned by the marketplace; the vote locks
    /// part of it through the CPI.
    #[account(
        mut,
        seeds = [marketplace::SHARE_SEED, &asset_id.to_le_bytes(), voter.key().as_ref()],
        bump = holding.bump,
        seeds::program = marketplace::ID,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    /// This round's vote record; revoting reuses it.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + AgentVote::INIT_SPACE,
        seeds = [
            AGENT_VOTE_SEED,
            &asset_id.to_le_bytes(),
            &letting.election.round.to_le_bytes(),
            voter.key().as_ref(),
        ],
        bump,
    )]
    pub vote_record: Box<Account<'info, AgentVote>>,

    /// The candidacy voted for.
    #[account(mut)]
    pub candidacy: Box<Account<'info, AgentCandidacy>>,

    /// The candidacy a revote moves power away from; required only when the
    /// previous vote backed a different candidate.
    #[account(mut)]
    pub previous_candidacy: Option<Box<Account<'info, AgentCandidacy>>>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs.
    #[account(seeds = [CPI_AUTH_SEED], bump)]
    pub cpi_auth: UncheckedAccount<'info>,

    pub marketplace_program: Program<'info, Marketplace>,
    pub system_program: Program<'info, System>,
}

pub fn vote_on_agent_handler(ctx: Context<VoteOnAgent>, asset_id: u64, amount: u32) -> Result<()> {
    let election = &ctx.accounts.letting.election;
    let round = election.round;
    require!(election.expiry != 0, PropertyError::NoElectionRunning);
    require!(
        Clock::get()?.unix_timestamp < election.expiry,
        PropertyError::VotingClosed
    );
    require!(amount > 0, PropertyError::InvalidVoteAmount);

    // The choice this vote backs; candidacies are unique per round, so field
    // equality pins them as tightly as their seeds do.
    let candidacy = &mut ctx.accounts.candidacy;
    require!(
        candidacy.asset_id == asset_id && candidacy.round == round,
        PropertyError::CandidacyMismatch
    );
    let choice = candidacy.agent;

    // The old candidacy rides along exactly when a revote moves power off a
    // different candidate. Anything extra is rejected: Anchor writes every
    // deserialized account back at exit, so a surplus copy of a candidacy
    // touched elsewhere in the instruction would overwrite it with stale data.
    let record = &mut ctx.accounts.vote_record;
    let needs_previous = record.power > 0 && record.choice != choice;
    require!(
        ctx.accounts.previous_candidacy.is_some() == needs_previous,
        PropertyError::CandidacyMismatch
    );

    // A revote first takes the old vote back out of its tally.
    if record.power > 0 {
        if record.choice == choice {
            // Same target: the tally adjusts through the one account.
            candidacy.vote_power = candidacy
                .vote_power
                .checked_sub(record.power)
                .ok_or(PropertyError::Overflow)?;
        } else {
            let previous = ctx
                .accounts
                .previous_candidacy
                .as_mut()
                .ok_or(PropertyError::CandidacyMismatch)?;
            require!(
                previous.asset_id == asset_id
                    && previous.round == round
                    && previous.agent == record.choice,
                PropertyError::CandidacyMismatch
            );
            previous.vote_power = previous
                .vote_power
                .checked_sub(record.power)
                .ok_or(PropertyError::Overflow)?;
        }
    }
    candidacy.vote_power = candidacy
        .vote_power
        .checked_add(amount)
        .ok_or(PropertyError::Overflow)?;

    // The marketplace moves the lock by the net difference and enforces
    // that everything locked still fits inside the holding, including
    // leftovers from earlier rounds the voter hasn't unlocked yet.
    adjust_share_lock(
        &ctx.accounts.cpi_auth.to_account_info(),
        &ctx.accounts.holding.to_account_info(),
        ctx.bumps.cpi_auth,
        asset_id,
        ctx.accounts.voter.key(),
        LockReason::AgentElection,
        record.power,
        amount,
    )?;

    // A revote leaves the recorded rent payer alone: the refund belongs to
    // whoever funded the account, not whoever last touched it.
    if record.rent_payer == Pubkey::default() {
        record.rent_payer = ctx.accounts.payer.key();
    }
    record.asset_id = asset_id;
    record.round = round;
    record.voter = ctx.accounts.voter.key();
    record.choice = choice;
    record.power = amount;
    record.bump = ctx.bumps.vote_record;

    emit!(AgentVoteCast {
        asset_id,
        round,
        voter: record.voter,
        choice,
        power: amount,
    });
    Ok(())
}

/// Settle the agent election once voting closes. Permissionless; every
/// candidacy of the round rides along as a remaining account so the
/// plurality is computed over the full field. A unique leading candidate
/// with quorum who still covers the property's location is assigned;
/// anything else reopens the seat for a fresh round.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct FinalizeAgentElection<'info> {
    pub cranker: Signer<'info>,

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

    /// CHECK: the winning agent's registry entry; the handler derives the
    /// expected address once the winner is known and inspects it directly, so
    /// a vanished registry fails the election instead of the transaction.
    #[account(mut)]
    pub winner_entry: Option<UncheckedAccount<'info>>,
}

pub fn finalize_agent_election_handler<'info>(
    ctx: Context<'info, FinalizeAgentElection<'info>>,
    asset_id: u64,
) -> Result<()> {
    let election = ctx.accounts.letting.election;
    require!(election.expiry != 0, PropertyError::NoElectionRunning);
    let now = Clock::get()?.unix_timestamp;
    require!(now >= election.expiry, PropertyError::VotingStillOngoing);

    // The full candidate field, complete and duplicate-free: the count pins
    // how many exist, and candidacy PDAs are unique per (round, agent).
    require!(
        ctx.remaining_accounts.len() == election.candidate_count as usize,
        PropertyError::CandidacyMismatch
    );
    let mut powers = Vec::with_capacity(ctx.remaining_accounts.len());
    let mut candidates = Vec::with_capacity(ctx.remaining_accounts.len());
    for (i, info) in ctx.remaining_accounts.iter().enumerate() {
        require!(
            ctx.remaining_accounts[..i]
                .iter()
                .all(|other| other.key != info.key),
            PropertyError::CandidacyMismatch
        );
        let candidacy: Account<AgentCandidacy> = Account::try_from(info)?;
        require!(
            candidacy.asset_id == asset_id && candidacy.round == election.round,
            PropertyError::CandidacyMismatch
        );
        powers.push(candidacy.vote_power as u64);
        candidates.push((candidacy.agent, candidacy.vote_power));
    }
    let tally = tally_plurality(powers).ok_or(PropertyError::Overflow)?;
    let quorum_met = tally.total * 10_000
        > ctx.accounts.property.share_amount as u64 * election.quorum_bps as u64;
    let tied = tally.tied;

    let mut assigned = false;
    let (winner, top_power) = tally.leader.map(|i| candidates[i]).unwrap_or_default();
    if quorum_met && !tied && top_power > 0 && ctx.accounts.letting.agent == Pubkey::default() {
        // The win only sticks if the winner still covers the location; an
        // agent who left mid-election fails the round instead of wedging it.
        let expected = Pubkey::find_program_address(&[AGENT_SEED, winner.as_ref()], &crate::ID).0;
        if let Some(entry_info) = &ctx.accounts.winner_entry {
            require!(entry_info.key() == expected, PropertyError::WrongAgent);
            if !entry_info.data_is_empty() {
                let mut entry: Account<LettingAgent> = Account::try_from(entry_info)?;
                let property = &ctx.accounts.property;
                if entry.region_id == property.region_id {
                    if let Some(location) = entry
                        .locations
                        .iter_mut()
                        .find(|l| l.postcode == property.location)
                    {
                        location.assigned_count = location
                            .assigned_count
                            .checked_add(1)
                            .ok_or(PropertyError::Overflow)?;
                        entry.exit(&crate::ID)?;
                        assigned = true;
                    }
                }
            }
        } else {
            // A winner exists, so the cranker must supply the entry for
            // inspection; erroring here just means resubmitting.
            return err!(PropertyError::WrongAgent);
        }
    }

    let letting = &mut ctx.accounts.letting;
    if assigned {
        letting.agent = winner;
        // A new agent starts with a clean record.
        letting.governance.strikes = 0;
    }
    // The round number stays for the vote records and candidacies still
    // keyed to it; the cleared window is what lets them close.
    letting.election.expiry = 0;
    letting.election.candidate_count = 0;

    emit!(AgentElectionFinalized {
        asset_id,
        round: election.round,
        winner,
        assigned,
        top_power,
    });
    Ok(())
}

/// Reclaim a candidacy's rent once its round is over (settled by the
/// finalizer, or superseded by a later round). Permissionless; the rent goes
/// back to whoever fronted the candidacy.
#[derive(Accounts)]
#[instruction(asset_id: u64, round: u64, agent: Pubkey)]
pub struct CloseAgentCandidacy<'info> {
    pub cranker: Signer<'info>,

    /// CHECK: the wallet that fronted the candidacy's rent; gets it back as
    /// the candidacy closes.
    #[account(mut, address = candidacy.rent_payer @ PropertyError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [
            AGENT_CANDIDATE_SEED,
            &asset_id.to_le_bytes(),
            &round.to_le_bytes(),
            agent.as_ref(),
        ],
        bump = candidacy.bump,
    )]
    pub candidacy: Box<Account<'info, AgentCandidacy>>,
}

pub fn close_agent_candidacy_handler(
    ctx: Context<CloseAgentCandidacy>,
    asset_id: u64,
    round: u64,
    agent: Pubkey,
) -> Result<()> {
    // The finalizer walks the full candidate field, so candidacies must hold
    // still until the round is settled or superseded.
    let election = &ctx.accounts.letting.election;
    require!(
        round != election.round || election.expiry == 0,
        PropertyError::VotingStillOngoing
    );

    emit!(AgentCandidacyClosed {
        asset_id,
        round,
        agent,
    });
    Ok(())
}

/// Release the shares a vote locked, once that round can no longer use them.
/// Closes the vote record, returning its rent to whoever fronted it. Not
/// role-gated: this is a pure exit.
#[derive(Accounts)]
#[instruction(asset_id: u64, round: u64)]
pub struct UnlockAgentVotes<'info> {
    pub voter: Signer<'info>,

    /// CHECK: the wallet that fronted the record's rent; gets it back as the
    /// record closes.
    #[account(mut, address = vote_record.rent_payer @ PropertyError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

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
            AGENT_VOTE_SEED,
            &asset_id.to_le_bytes(),
            &round.to_le_bytes(),
            voter.key().as_ref(),
        ],
        bump = vote_record.bump,
    )]
    pub vote_record: Box<Account<'info, AgentVote>>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs.
    #[account(seeds = [CPI_AUTH_SEED], bump)]
    pub cpi_auth: UncheckedAccount<'info>,

    pub marketplace_program: Program<'info, Marketplace>,
}

pub fn unlock_agent_votes_handler(
    ctx: Context<UnlockAgentVotes>,
    asset_id: u64,
    round: u64,
) -> Result<()> {
    let election = &ctx.accounts.letting.election;
    // Locked only while this exact round's election is still live.
    let election_live = round == election.round
        && election.expiry != 0
        && Clock::get()?.unix_timestamp < election.expiry;
    require!(!election_live, PropertyError::VotingStillOngoing);

    let power = ctx.accounts.vote_record.power;
    adjust_share_lock(
        &ctx.accounts.cpi_auth.to_account_info(),
        &ctx.accounts.holding.to_account_info(),
        ctx.bumps.cpi_auth,
        asset_id,
        ctx.accounts.voter.key(),
        LockReason::AgentElection,
        power,
        0,
    )?;

    emit!(AgentVotesUnlocked {
        asset_id,
        round,
        voter: ctx.accounts.voter.key(),
        power,
    });
    Ok(())
}

#[event]
pub struct AgentCandidacyClaimed {
    pub asset_id: u64,
    pub round: u64,
    pub agent: Pubkey,
    pub expiry: i64,
}

#[event]
pub struct AgentVoteCast {
    pub asset_id: u64,
    pub round: u64,
    pub voter: Pubkey,
    pub choice: Pubkey,
    pub power: u32,
}

#[event]
pub struct AgentElectionFinalized {
    pub asset_id: u64,
    pub round: u64,
    pub winner: Pubkey,
    pub assigned: bool,
    pub top_power: u32,
}

#[event]
pub struct AgentCandidacyClosed {
    pub asset_id: u64,
    pub round: u64,
    pub agent: Pubkey,
}

#[event]
pub struct AgentVotesUnlocked {
    pub asset_id: u64,
    pub round: u64,
    pub voter: Pubkey,
    pub power: u32,
}
