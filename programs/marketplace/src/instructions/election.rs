use anchor_lang::prelude::*;

use crate::constants::{
    LAWYER_CANDIDATE_SEED, LAWYER_SEED, LAWYER_VOTE_SEED, LISTING_SEED, PROPERTY_SEED, SHARE_SEED,
};
use crate::error::MarketplaceError;
use crate::state::{
    DocumentStatus, Lawyer, LawyerCandidacy, LawyerVote, Listing, ListingStatus, LockReason,
    PropertyAsset, ShareHolding,
};

use xcavate_whitelist::state::{Role, RoleAccount};

use xcavate_common::election::tally_plurality;

/// Cast or change a vote in the running SPV lawyer election, weighted by the
/// shares put behind it. Every vote backs a candidacy; a round that elects
/// nobody reopens for the next one. The shares lock until
/// `unlock_voting_shares`. Investor-role only; no compliance check, since no
/// money moves.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct VoteOnSpvLawyer<'info> {
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
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    /// The voter's share ledger; the vote locks part of it.
    #[account(
        mut,
        seeds = [SHARE_SEED, &listing_id.to_le_bytes(), voter.key().as_ref()],
        bump = holding.bump,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    /// This round's vote record; revoting reuses it.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + LawyerVote::INIT_SPACE,
        seeds = [
            LAWYER_VOTE_SEED,
            &listing_id.to_le_bytes(),
            &listing.spv_election.round.to_le_bytes(),
            voter.key().as_ref(),
        ],
        bump,
    )]
    pub vote_record: Box<Account<'info, LawyerVote>>,

    /// The candidacy voted for.
    #[account(mut)]
    pub candidacy: Box<Account<'info, LawyerCandidacy>>,

    /// The candidacy a revote moves power away from; required only when the
    /// previous vote backed a different candidate.
    #[account(mut)]
    pub previous_candidacy: Option<Box<Account<'info, LawyerCandidacy>>>,

    pub system_program: Program<'info, System>,
}

pub fn vote_on_spv_lawyer_handler(
    ctx: Context<VoteOnSpvLawyer>,
    listing_id: u64,
    amount: u32,
) -> Result<()> {
    let listing = &mut ctx.accounts.listing;
    let round = listing.spv_election.round;
    require!(
        listing.spv_election.expiry != 0,
        MarketplaceError::NoLawyerProposed
    );
    require!(
        listing.status == ListingStatus::SoldOut,
        MarketplaceError::ListingNotActive
    );
    require!(
        Clock::get()?.unix_timestamp < listing.spv_election.expiry,
        MarketplaceError::VotingClosed
    );
    require!(amount > 0, MarketplaceError::InvalidVoteAmount);

    // The choice this vote backs; candidacies are unique per round, so field
    // equality pins them as tightly as their seeds do.
    let candidacy = &mut ctx.accounts.candidacy;
    require!(
        candidacy.listing_id == listing_id && candidacy.round == round,
        MarketplaceError::CandidacyMismatch
    );
    let choice = candidacy.lawyer;

    // The old candidacy rides along exactly when a revote moves power off a
    // different candidate. Anything extra is rejected: Anchor writes every
    // deserialized account back at exit, so a surplus copy of a candidacy
    // touched elsewhere in the instruction would overwrite it with stale data.
    let holding = &mut ctx.accounts.holding;
    let record = &mut ctx.accounts.vote_record;
    let needs_previous = record.power > 0 && record.choice != choice;
    require!(
        ctx.accounts.previous_candidacy.is_some() == needs_previous,
        MarketplaceError::CandidacyMismatch
    );

    let lock = LockReason::LawyerElection as usize;
    // A revote first takes the old vote back out of its tally and the lock.
    if record.power > 0 {
        holding.locks[lock] = holding.locks[lock]
            .checked_sub(record.power)
            .ok_or(MarketplaceError::Overflow)?;
        if record.choice == choice {
            // Same target: the tally adjusts through the one account.
            candidacy.vote_power = candidacy
                .vote_power
                .checked_sub(record.power)
                .ok_or(MarketplaceError::Overflow)?;
        } else {
            let previous = ctx
                .accounts
                .previous_candidacy
                .as_mut()
                .ok_or(MarketplaceError::CandidacyMismatch)?;
            require!(
                previous.listing_id == listing_id
                    && previous.round == round
                    && previous.lawyer == record.choice,
                MarketplaceError::CandidacyMismatch
            );
            previous.vote_power = previous
                .vote_power
                .checked_sub(record.power)
                .ok_or(MarketplaceError::Overflow)?;
        }
    }

    // The lock can carry leftovers from earlier rounds the voter hasn't
    // unlocked yet, so the new vote must fit next to those too.
    let locked_after = holding.locks[lock]
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;
    require!(
        locked_after <= holding.votable(),
        MarketplaceError::NotEnoughShares
    );
    holding.locks[lock] = locked_after;
    candidacy.vote_power = candidacy
        .vote_power
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;

    // A revote leaves the recorded rent payer alone: the refund belongs to
    // whoever funded the account, not whoever last touched it.
    if record.rent_payer == Pubkey::default() {
        record.rent_payer = ctx.accounts.payer.key();
    }
    record.listing_id = listing_id;
    record.round = round;
    record.voter = ctx.accounts.voter.key();
    record.choice = choice;
    record.power = amount;
    record.bump = ctx.bumps.vote_record;

    emit!(SpvLawyerVoteCast {
        listing_id,
        round,
        voter: record.voter,
        choice,
        power: amount,
    });
    Ok(())
}

/// Settle the SPV lawyer election once voting closes. Permissionless; every
/// candidacy of the round rides along as a remaining account so the
/// plurality is computed over the full field. A unique leading candidate
/// with quorum who still passes the engagement checks is assigned; anything
/// else reopens the slot for a fresh round, until a lawyer is engaged or the
/// legal process runs out.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct FinalizeSpvElection<'info> {
    pub cranker: Signer<'info>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    #[account(
        seeds = [PROPERTY_SEED, &listing_id.to_le_bytes()],
        bump = property.bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    /// CHECK: the winning candidate's registry entry; the handler derives the
    /// expected address once the winner is known and inspects it directly, so
    /// a vanished registry fails the election instead of the transaction.
    #[account(mut)]
    pub winner_registry: Option<UncheckedAccount<'info>>,
}

pub fn finalize_spv_election_handler<'info>(
    ctx: Context<'info, FinalizeSpvElection<'info>>,
    listing_id: u64,
) -> Result<()> {
    let listing = &ctx.accounts.listing;
    let election = listing.spv_election;
    require!(election.expiry != 0, MarketplaceError::NoLawyerProposed);
    let now = Clock::get()?.unix_timestamp;
    require!(now >= election.expiry, MarketplaceError::VotingStillOngoing);

    // The full candidate field, complete and duplicate-free: the count pins
    // how many exist, and candidacy PDAs are unique per (round, lawyer).
    require!(
        ctx.remaining_accounts.len() == election.candidate_count as usize,
        MarketplaceError::CandidacyMismatch
    );
    let mut powers = Vec::with_capacity(ctx.remaining_accounts.len());
    let mut candidates = Vec::with_capacity(ctx.remaining_accounts.len());
    for (i, info) in ctx.remaining_accounts.iter().enumerate() {
        require!(
            ctx.remaining_accounts[..i]
                .iter()
                .all(|other| other.key != info.key),
            MarketplaceError::CandidacyMismatch
        );
        let candidacy: Account<LawyerCandidacy> = Account::try_from(info)?;
        require!(
            candidacy.listing_id == listing_id && candidacy.round == election.round,
            MarketplaceError::CandidacyMismatch
        );
        powers.push(candidacy.vote_power as u64);
        candidates.push((candidacy.lawyer, candidacy.costs, candidacy.vote_power));
    }
    let tally = tally_plurality(powers).ok_or(MarketplaceError::Overflow)?;
    let quorum_met = tally.total * 10_000
        > ctx.accounts.property.share_amount as u64 * listing.min_voting_quorum_bps as u64;
    let tied = tally.tied;

    let mut assigned = false;
    let (winner, winner_costs, top_power) = tally.leader.map(|i| candidates[i]).unwrap_or_default();
    if quorum_met
        && listing.status == ListingStatus::SoldOut
        && !tied
        && top_power > 0
        && now <= listing.legal_deadline
        && winner != listing.developer_lawyer.lawyer
        && winner_costs <= listing.collected_fee_quote
    {
        // The win only sticks if the engagement checks still hold; a
        // vanished or conflicted winner fails the election instead of
        // wedging it.
        let expected = Pubkey::find_program_address(&[LAWYER_SEED, winner.as_ref()], &crate::ID).0;
        if let Some(registry_info) = &ctx.accounts.winner_registry {
            require!(
                registry_info.key() == expected,
                MarketplaceError::WrongLawyer
            );
            if !registry_info.data_is_empty() {
                let mut registry: Account<Lawyer> = Account::try_from(registry_info)?;
                if registry.region_id == ctx.accounts.property.region_id {
                    registry.active_cases = registry
                        .active_cases
                        .checked_add(1)
                        .ok_or(MarketplaceError::Overflow)?;
                    registry.exit(&crate::ID)?;
                    assigned = true;
                }
            }
        } else {
            // A winner exists, so the cranker must supply the registry
            // for inspection; erroring here just means resubmitting.
            return err!(MarketplaceError::WrongLawyer);
        }
    }

    let listing = &mut ctx.accounts.listing;
    if assigned {
        listing.spv_lawyer.lawyer = winner;
        listing.spv_lawyer.costs = winner_costs;
        listing.spv_lawyer.doc_status = DocumentStatus::Pending;
    }
    // The round number stays for the vote records and candidacies still
    // keyed to it; the cleared window is what lets them close.
    listing.spv_election.expiry = 0;
    listing.spv_election.candidate_count = 0;

    emit!(SpvElectionFinalized {
        listing_id,
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
#[instruction(listing_id: u64, round: u64, lawyer: Pubkey)]
pub struct CloseCandidacy<'info> {
    pub cranker: Signer<'info>,

    /// CHECK: the wallet that fronted the candidacy's rent; gets it back as
    /// the candidacy closes.
    #[account(mut, address = candidacy.rent_payer @ MarketplaceError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    /// CHECK: the listing PDA, pinned by seeds. Unchecked because it may
    /// already be torn down; a candidacy must stay closable after that.
    #[account(seeds = [LISTING_SEED, &listing_id.to_le_bytes()], bump)]
    pub listing: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [
            LAWYER_CANDIDATE_SEED,
            &listing_id.to_le_bytes(),
            &round.to_le_bytes(),
            lawyer.as_ref(),
        ],
        bump = candidacy.bump,
    )]
    pub candidacy: Box<Account<'info, LawyerCandidacy>>,
}

pub fn close_candidacy_handler(
    ctx: Context<CloseCandidacy>,
    listing_id: u64,
    round: u64,
    lawyer: Pubkey,
) -> Result<()> {
    // The finalizer walks the full candidate field, so candidacies must hold
    // still until the round is settled or superseded. A torn-down listing
    // settled everything by definition.
    if !ctx.accounts.listing.data_is_empty() {
        let data = ctx.accounts.listing.try_borrow_data()?;
        let listing = Listing::try_deserialize(&mut &data[..])?;
        let election = &listing.spv_election;
        require!(
            round != election.round || election.expiry == 0,
            MarketplaceError::VotingStillOngoing
        );
    }

    emit!(CandidacyClosed {
        listing_id,
        round,
        lawyer,
    });
    Ok(())
}

/// Release the shares a vote locked, once that round can no longer use them:
/// the election was settled or superseded, or the sale left `SoldOut`
/// entirely. Closes the vote record, returning its rent to whoever fronted
/// it. Not role-gated: this is a pure exit.
#[derive(Accounts)]
#[instruction(listing_id: u64, round: u64)]
pub struct UnlockVotingShares<'info> {
    pub voter: Signer<'info>,

    /// CHECK: the wallet that fronted the record's rent; gets it back as the
    /// record closes.
    #[account(mut, address = vote_record.rent_payer @ MarketplaceError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    /// Typed, unlike `close_candidacy`'s tolerant listing: teardown requires
    /// `holder_count == 0`, holdings only empty with `locked() == 0`, so
    /// every lock is provably released while the listing still exists.
    /// Relaxing that teardown guard would make this a permanent lock.
    #[account(
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    #[account(
        mut,
        seeds = [SHARE_SEED, &listing_id.to_le_bytes(), voter.key().as_ref()],
        bump = holding.bump,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [
            LAWYER_VOTE_SEED,
            &listing_id.to_le_bytes(),
            &round.to_le_bytes(),
            voter.key().as_ref(),
        ],
        bump = vote_record.bump,
    )]
    pub vote_record: Box<Account<'info, LawyerVote>>,
}

pub fn unlock_voting_shares_handler(
    ctx: Context<UnlockVotingShares>,
    listing_id: u64,
    round: u64,
) -> Result<()> {
    let listing = &ctx.accounts.listing;
    let election = &listing.spv_election;
    // Locked only while this exact round's election is still live.
    let election_live = round == election.round
        && election.expiry != 0
        && listing.status == ListingStatus::SoldOut
        && Clock::get()?.unix_timestamp < election.expiry;
    require!(!election_live, MarketplaceError::VotingStillOngoing);

    let power = ctx.accounts.vote_record.power;
    let lock = LockReason::LawyerElection as usize;
    ctx.accounts.holding.locks[lock] = ctx.accounts.holding.locks[lock]
        .checked_sub(power)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(VotingSharesUnlocked {
        listing_id,
        round,
        voter: ctx.accounts.voter.key(),
        power,
    });
    Ok(())
}

#[event]
pub struct SpvLawyerVoteCast {
    pub listing_id: u64,
    pub round: u64,
    pub voter: Pubkey,
    pub choice: Pubkey,
    pub power: u32,
}

#[event]
pub struct SpvElectionFinalized {
    pub listing_id: u64,
    pub round: u64,
    pub winner: Pubkey,
    pub assigned: bool,
    pub top_power: u32,
}

#[event]
pub struct CandidacyClosed {
    pub listing_id: u64,
    pub round: u64,
    pub lawyer: Pubkey,
}

#[event]
pub struct VotingSharesUnlocked {
    pub listing_id: u64,
    pub round: u64,
    pub voter: Pubkey,
    pub power: u32,
}
