use anchor_lang::prelude::*;

use crate::constants::{
    CONFIG_SEED, LAWYER_CANDIDATE_SEED, LAWYER_SEED, LISTING_SEED, PROPERTY_SEED,
};
use crate::error::MarketplaceError;
use crate::state::{
    Config, DocumentStatus, Lawyer, LawyerCandidacy, Listing, ListingStatus, PropertyAsset,
    MAX_SPV_CANDIDATES,
};

use xcavate_whitelist::state::{Compliance, Role, RoleAccount};

/// The gates every lawyer engagement shares: the sale is sold out, the legal
/// process still has time, and the lawyer serves the property's region.
fn check_engagement_open(
    listing: &Listing,
    property: &PropertyAsset,
    registry: &Lawyer,
) -> Result<()> {
    require!(
        listing.status == ListingStatus::SoldOut,
        MarketplaceError::ListingNotActive
    );
    require!(
        Clock::get()?.unix_timestamp <= listing.legal_deadline,
        MarketplaceError::LegalProcessExpired
    );
    require!(
        registry.region_id == property.region_id,
        MarketplaceError::WrongRegion
    );
    Ok(())
}

/// The developer engages their own lawyer directly by address. The named
/// wallet must hold a compliant Lawyer role and a registry entry in the
/// property's region. Their pay is the developer's own private arrangement,
/// off chain; the only money that ever reaches them here is the tax they
/// remit when the developer covers it. Developer-role only, and only the
/// listing's own developer.
#[derive(Accounts)]
#[instruction(listing_id: u64, lawyer: Pubkey)]
pub struct AssignDeveloperLawyer<'info> {
    pub developer: Signer<'info>,

    /// The caller's RealEstateDeveloper role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            developer.key().as_ref(),
            &[Role::RealEstateDeveloper.seed_byte()],
        ],
        bump = developer_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub developer_role: Box<Account<'info, RoleAccount>>,

    /// The named lawyer's role. They carry the legal responsibility, so their
    /// KYC is checked too even though they aren't the signer.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            lawyer.as_ref(),
            &[Role::Lawyer.seed_byte()],
        ],
        bump = lawyer_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub lawyer_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, lawyer.as_ref()],
        bump = lawyer_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = lawyer_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub lawyer_compliance: Box<Account<'info, Compliance>>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The named lawyer's registry entry; takes the case.
    #[account(
        mut,
        seeds = [LAWYER_SEED, lawyer.as_ref()],
        bump = registry.bump,
    )]
    pub registry: Box<Account<'info, Lawyer>>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
        constraint = listing.developer == developer.key() @ MarketplaceError::NotListingDeveloper,
    )]
    pub listing: Box<Account<'info, Listing>>,

    #[account(
        seeds = [PROPERTY_SEED, &listing_id.to_le_bytes()],
        bump = property.bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,
}

pub fn assign_developer_lawyer_handler(
    ctx: Context<AssignDeveloperLawyer>,
    listing_id: u64,
    lawyer: Pubkey,
) -> Result<()> {
    let listing = &ctx.accounts.listing;
    check_engagement_open(listing, &ctx.accounts.property, &ctx.accounts.registry)?;
    require!(
        listing.developer_lawyer.lawyer == Pubkey::default(),
        MarketplaceError::LawyerJobTaken
    );
    // One lawyer never acts for both sides.
    require!(
        lawyer != listing.spv_lawyer.lawyer,
        MarketplaceError::ConflictOfInterest
    );

    ctx.accounts.registry.active_cases = ctx
        .accounts
        .registry
        .active_cases
        .checked_add(1)
        .ok_or(MarketplaceError::Overflow)?;
    let listing = &mut ctx.accounts.listing;
    listing.developer_lawyer.lawyer = lawyer;
    listing.developer_lawyer.doc_status = DocumentStatus::Pending;
    listing.developer_engaged = true;

    emit!(DeveloperLawyerAssigned { listing_id, lawyer });
    Ok(())
}

/// A registered lawyer stands for election as the SPV's lawyer, naming their
/// costs. The first candidacy opens the voting window; later ones join the
/// same round while it runs. Lawyer-role only and compliance-gated: engaging
/// on a live sale is where legal responsibility starts.
#[derive(Accounts)]
#[instruction(listing_id: u64, round: u64)]
pub struct ClaimSpvCase<'info> {
    pub lawyer: Signer<'info>,

    /// Whoever fronts the candidacy's rent: the sponsor on the default path,
    /// or any willing wallet, so one protocol key can never block a
    /// candidacy. The record remembers who to refund.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The caller's Lawyer role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            lawyer.key().as_ref(),
            &[Role::Lawyer.seed_byte()],
        ],
        bump = lawyer_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub lawyer_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, lawyer.key().as_ref()],
        bump = lawyer_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = lawyer_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub lawyer_compliance: Box<Account<'info, Compliance>>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The caller's registry entry; proves registration and carries the region.
    #[account(
        seeds = [LAWYER_SEED, lawyer.key().as_ref()],
        bump = registry.bump,
    )]
    pub registry: Box<Account<'info, Lawyer>>,

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

    /// The candidacy, one per lawyer per round; `init` rejects standing twice.
    #[account(
        init,
        payer = payer,
        space = 8 + LawyerCandidacy::INIT_SPACE,
        seeds = [
            LAWYER_CANDIDATE_SEED,
            &listing_id.to_le_bytes(),
            &round.to_le_bytes(),
            lawyer.key().as_ref(),
        ],
        bump,
    )]
    pub candidacy: Box<Account<'info, LawyerCandidacy>>,

    pub system_program: Program<'info, System>,
}

pub fn claim_spv_case_handler(
    ctx: Context<ClaimSpvCase>,
    listing_id: u64,
    round: u64,
    costs: u64,
) -> Result<()> {
    let listing = &ctx.accounts.listing;
    check_engagement_open(listing, &ctx.accounts.property, &ctx.accounts.registry)?;
    require!(
        ctx.accounts.property.spv_created,
        MarketplaceError::SpvNotCreated
    );
    require!(
        listing.spv_lawyer.lawyer == Pubkey::default(),
        MarketplaceError::LawyerJobTaken
    );
    let lawyer = ctx.accounts.lawyer.key();
    require!(
        lawyer != listing.developer_lawyer.lawyer,
        MarketplaceError::ConflictOfInterest
    );
    // The SPV lawyer is the only one the fee pot pays; the developer's own
    // lawyer is a private arrangement.
    require!(
        costs <= listing.collected_fee_quote,
        MarketplaceError::CostsExceedFees
    );

    let now = Clock::get()?.unix_timestamp;
    let listing = &mut ctx.accounts.listing;
    let voting_time = listing.lawyer_voting_time;
    let deadline = listing.legal_deadline;
    let election = &mut listing.spv_election;
    if election.expiry == 0 {
        // First candidacy: a new round opens and the clock starts. The
        // window never outlives the legal deadline, past which the winner
        // could not be assigned anyway.
        require!(
            round == election.round + 1,
            MarketplaceError::WrongElectionRound
        );
        election.round = round;
        election.expiry = now
            .checked_add(voting_time)
            .ok_or(MarketplaceError::Overflow)?
            .min(deadline);
        election.candidate_count = 0;
    } else {
        require!(
            round == election.round,
            MarketplaceError::WrongElectionRound
        );
        require!(now < election.expiry, MarketplaceError::VotingClosed);
    }
    election.candidate_count = election
        .candidate_count
        .checked_add(1)
        .ok_or(MarketplaceError::Overflow)?;
    require!(
        election.candidate_count <= MAX_SPV_CANDIDATES,
        MarketplaceError::TooManyCandidates
    );

    let candidacy = &mut ctx.accounts.candidacy;
    candidacy.listing_id = listing_id;
    candidacy.round = round;
    candidacy.lawyer = lawyer;
    candidacy.costs = costs;
    candidacy.vote_power = 0;
    candidacy.rent_payer = ctx.accounts.payer.key();
    candidacy.bump = ctx.bumps.candidacy;

    emit!(SpvCaseClaimed {
        listing_id,
        round,
        lawyer,
        costs,
        expiry: listing.spv_election.expiry,
    });
    Ok(())
}

/// A lawyer steps back from a case they were engaged on, reopening that side,
/// as long as they haven't confirmed documents yet. Not role-gated: stepping
/// back is an exit, and the registry PDA plus the assignment on the listing
/// prove who the caller is.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct ResignFromCase<'info> {
    pub lawyer: Signer<'info>,

    #[account(
        mut,
        seeds = [LAWYER_SEED, lawyer.key().as_ref()],
        bump = registry.bump,
    )]
    pub registry: Box<Account<'info, Lawyer>>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,
}

pub fn resign_from_case_handler(ctx: Context<ResignFromCase>, listing_id: u64) -> Result<()> {
    let lawyer = ctx.accounts.lawyer.key();
    let listing = &mut ctx.accounts.listing;
    let side = if listing.developer_lawyer.lawyer == lawyer {
        &mut listing.developer_lawyer
    } else if listing.spv_lawyer.lawyer == lawyer {
        &mut listing.spv_lawyer
    } else {
        return err!(MarketplaceError::NotCaseLawyer);
    };
    // Once documents are confirmed the sale is relying on this lawyer.
    require!(
        side.doc_status == DocumentStatus::Pending,
        MarketplaceError::AlreadyConfirmed
    );
    side.lawyer = Pubkey::default();
    side.costs = 0;

    ctx.accounts.registry.active_cases = ctx
        .accounts
        .registry
        .active_cases
        .checked_sub(1)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(LawyerResigned { listing_id, lawyer });
    Ok(())
}

/// A case lawyer's verdict on the sale documents, bound to the hash of the
/// set they reviewed; verdicts only combine when the hashes agree. Both
/// sides approving moves the sale to settlement; both rejecting cancels it
/// and opens the refunds. A split verdict sends the documents back for
/// revision once, every verdict resetting to pending for the new set, and
/// cancels on the second split. Confirming binds the sale, so the caller
/// must still hold the Lawyer role; compliance was vetted when they engaged.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct ConfirmDocuments<'info> {
    pub lawyer: Signer<'info>,

    /// The caller's Lawyer role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            lawyer.key().as_ref(),
            &[Role::Lawyer.seed_byte()],
        ],
        bump = lawyer_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub lawyer_role: Box<Account<'info, RoleAccount>>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,
}

/// Cancellation stamps what the SPV lawyer is owed from the retained fees,
/// and who collects it, before `close_case` can clear the assignment.
fn cancel_sale(listing: &mut Listing) {
    listing.status = ListingStatus::Cancelled;
    listing.spv_costs_due = listing.spv_lawyer.costs;
    listing.spv_costs_payee = listing.spv_lawyer.lawyer;
}

pub fn confirm_documents_handler(
    ctx: Context<ConfirmDocuments>,
    listing_id: u64,
    approve: bool,
    documents_hash: [u8; 32],
) -> Result<()> {
    let listing = &mut ctx.accounts.listing;
    require!(
        listing.status == ListingStatus::SoldOut,
        MarketplaceError::ListingNotActive
    );
    // Past the deadline the timeout exit takes over; a late verdict must not
    // beat it to the status.
    require!(
        Clock::get()?.unix_timestamp <= listing.legal_deadline,
        MarketplaceError::LegalProcessExpired
    );
    require!(
        documents_hash != [0u8; 32],
        MarketplaceError::EmptyDocumentsHash
    );

    let lawyer = ctx.accounts.lawyer.key();
    let is_developer_side = if listing.developer_lawyer.lawyer == lawyer {
        true
    } else if listing.spv_lawyer.lawyer == lawyer {
        false
    } else {
        return err!(MarketplaceError::NotCaseLawyer);
    };

    // The verdicts only combine when they rule on the same document set, so
    // a second verdict must name the hash the first one recorded.
    let other = if is_developer_side {
        &listing.spv_lawyer
    } else {
        &listing.developer_lawyer
    };
    require!(
        other.doc_status == DocumentStatus::Pending || other.documents_hash == documents_hash,
        MarketplaceError::DocumentsMismatch
    );

    let side = if is_developer_side {
        &mut listing.developer_lawyer
    } else {
        &mut listing.spv_lawyer
    };
    require!(
        side.doc_status == DocumentStatus::Pending,
        MarketplaceError::AlreadyConfirmed
    );
    side.doc_status = if approve {
        DocumentStatus::Approved
    } else {
        DocumentStatus::Rejected
    };
    side.documents_hash = documents_hash;

    use DocumentStatus::{Approved, Pending, Rejected};
    match (
        listing.developer_lawyer.doc_status,
        listing.spv_lawyer.doc_status,
    ) {
        (Approved, Approved) => listing.status = ListingStatus::Legal,
        (Rejected, Rejected) => cancel_sale(listing),
        (Approved, Rejected) | (Rejected, Approved) => {
            if listing.second_attempt {
                cancel_sale(listing);
            } else {
                // A revised set is expected, so the recorded hashes go too.
                listing.developer_lawyer.doc_status = Pending;
                listing.developer_lawyer.documents_hash = [0u8; 32];
                listing.spv_lawyer.doc_status = Pending;
                listing.spv_lawyer.documents_hash = [0u8; 32];
                listing.second_attempt = true;
            }
        }
        // The other side hasn't ruled yet.
        _ => {}
    }

    emit!(DocumentsConfirmed {
        listing_id,
        lawyer,
        approve,
        documents_hash,
        status: listing.status,
    });
    Ok(())
}

/// Cancel a sale whose standing SPV verdict the developer's side is
/// sitting out, so the SPV lawyer's pay never depends on the counterparty
/// choosing to rule. One-directional on purpose: with the SPV side silent
/// nobody's pay is at stake, and cancelling would keep buyer fees the
/// timeout exit refunds, so that case just rides to the deadline.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct ResolveSilentVerdict<'info> {
    pub cranker: Signer<'info>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,
}

pub fn resolve_silent_verdict_handler(
    ctx: Context<ResolveSilentVerdict>,
    listing_id: u64,
) -> Result<()> {
    let listing = &mut ctx.accounts.listing;
    require!(
        listing.status == ListingStatus::SoldOut,
        MarketplaceError::ListingNotActive
    );
    let now = Clock::get()?.unix_timestamp;
    require!(
        now <= listing.legal_deadline,
        MarketplaceError::LegalProcessExpired
    );
    let final_stretch = listing
        .legal_deadline
        .checked_sub(listing.legal_process_time / 5)
        .ok_or(MarketplaceError::Overflow)?;
    require!(now >= final_stretch, MarketplaceError::VerdictWindowOpen);

    let dev_ruled = listing.developer_lawyer.doc_status != DocumentStatus::Pending;
    let spv_ruled = listing.spv_lawyer.doc_status != DocumentStatus::Pending;
    require!(spv_ruled && !dev_ruled, MarketplaceError::NoVerdictPassed);

    let silent_lawyer = listing.developer_lawyer.lawyer;
    listing.developer_lawyer.doc_status = DocumentStatus::Rejected;
    cancel_sale(listing);

    emit!(SilentVerdictResolved {
        listing_id,
        silent_lawyer,
    });
    Ok(())
}

/// Release a lawyer from a sale that died: their side clears and their case
/// count drops, so they can unregister. Permissionless, once per side:
/// clearing the assignment is what makes a second call fail.
#[derive(Accounts)]
#[instruction(listing_id: u64, lawyer: Pubkey)]
pub struct CloseCase<'info> {
    pub cranker: Signer<'info>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    /// The released lawyer's registry entry.
    #[account(
        mut,
        seeds = [LAWYER_SEED, lawyer.as_ref()],
        bump = registry.bump,
    )]
    pub registry: Box<Account<'info, Lawyer>>,
}

pub fn close_case_handler(ctx: Context<CloseCase>, listing_id: u64, lawyer: Pubkey) -> Result<()> {
    let listing = &mut ctx.accounts.listing;
    require!(
        matches!(
            listing.status,
            ListingStatus::Cancelled | ListingStatus::Refunding | ListingStatus::Expired
        ),
        MarketplaceError::CaseStillOpen
    );
    let side = if listing.developer_lawyer.lawyer == lawyer {
        &mut listing.developer_lawyer
    } else if listing.spv_lawyer.lawyer == lawyer {
        &mut listing.spv_lawyer
    } else {
        return err!(MarketplaceError::NotCaseLawyer);
    };
    side.lawyer = Pubkey::default();
    side.costs = 0;

    ctx.accounts.registry.active_cases = ctx
        .accounts
        .registry
        .active_cases
        .checked_sub(1)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(CaseClosed { listing_id, lawyer });
    Ok(())
}

#[event]
pub struct DeveloperLawyerAssigned {
    pub listing_id: u64,
    pub lawyer: Pubkey,
}

#[event]
pub struct SpvCaseClaimed {
    pub listing_id: u64,
    pub round: u64,
    pub lawyer: Pubkey,
    pub costs: u64,
    pub expiry: i64,
}

#[event]
pub struct LawyerResigned {
    pub listing_id: u64,
    pub lawyer: Pubkey,
}

#[event]
pub struct DocumentsConfirmed {
    pub listing_id: u64,
    pub lawyer: Pubkey,
    pub approve: bool,
    /// The document set the verdict rules on.
    pub documents_hash: [u8; 32],
    /// Where the verdict left the sale.
    pub status: ListingStatus,
}

#[event]
pub struct CaseClosed {
    pub listing_id: u64,
    pub lawyer: Pubkey,
}

#[event]
pub struct SilentVerdictResolved {
    pub listing_id: u64,
    /// The lawyer who never ruled; default if that side was never engaged.
    pub silent_lawyer: Pubkey,
}
