use anchor_lang::prelude::*;
use anchor_spl::associated_token::{create_idempotent, AssociatedToken, Create as CreateAta};
use anchor_spl::token_2022::spl_token_2022::{
    extension::StateWithExtensions, state::Mint as MintState,
};
use anchor_spl::token_2022::{freeze_account, thaw_account, FreezeAccount, ThawAccount, Token2022};
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{
    CONFIG_SEED, LISTING_SEED, LISTING_VAULT_SEED, MINT_AUTH_SEED, POSITION_SEED, PROPERTY_SEED,
    PROPERTY_VAULT_SEED, RESERVATION_SEED, SHARE_MINT_SEED, SHARE_SEED,
};
use crate::error::MarketplaceError;
use crate::state::{
    Config, InvestorPosition, Listing, ListingStatus, PropertyAsset, Reservation, ShareHolding,
    LOCK_REASONS,
};

use xcavate_whitelist::state::{Compliance, Role, RoleAccount};

/// Reserve shares of a listed property. No money moves and no shares are
/// delivered: the cost is recorded against the investor's own payment
/// account, where it stays until the claim. The platform never touches the
/// funds before the SPV exists; the balance is checked again when the claim
/// pays. Investor-role only and compliance-gated, since this commits
/// investor money. The sponsor `payer` fronts all account rent, per the
/// rent policy.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct ReserveShares<'info> {
    pub investor: Signer<'info>,

    /// The sponsor wallet fronting rent for the investor's accounts.
    #[account(mut, address = config.rent_collector @ MarketplaceError::NotRentCollector)]
    pub payer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The caller's RealEstateInvestor role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            investor.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = investor_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub investor_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, investor.key().as_ref()],
        bump = investor_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = investor_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub investor_compliance: Box<Account<'info, Compliance>>,

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

    // The same per-investor upsert as the direct purchase; a fresh init
    // zeroes `cancelled`, so the cancelled-position crank must keep waiting
    // for the listing to leave `Listed`.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + InvestorPosition::INIT_SPACE,
        seeds = [POSITION_SEED, &listing_id.to_le_bytes(), investor.key().as_ref()],
        bump,
    )]
    pub position: Box<Account<'info, InvestorPosition>>,

    /// The mint the investor will pay in; must be on the accepted list.
    pub payment_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The investor's payment account the reservation binds.
    #[account(
        mut,
        token::mint = payment_mint,
        token::authority = investor,
    )]
    pub investor_payment: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The running record of how much of the payment account is promised;
    /// shared across every listing this account reserves into.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + Reservation::INIT_SPACE,
        seeds = [RESERVATION_SEED, investor_payment.key().as_ref()],
        bump,
    )]
    pub reservation: Box<Account<'info, Reservation>>,

    pub system_program: Program<'info, System>,
}

pub fn reserve_shares_handler(
    ctx: Context<ReserveShares>,
    listing_id: u64,
    amount: u32,
    max_total_cost: u64,
) -> Result<()> {
    let listing = &ctx.accounts.listing;
    require!(
        listing.status == ListingStatus::Listed,
        MarketplaceError::ListingNotActive
    );
    let now = Clock::get()?.unix_timestamp;
    require!(
        now < listing.listing_expiry,
        MarketplaceError::ListingExpired
    );
    // Once the claim window has run out the sale moves to direct purchases.
    require!(
        listing.claim_deadline == 0 || now < listing.claim_deadline,
        MarketplaceError::ClaimWindowClosed
    );
    let available = listing
        .listed_share_amount
        .saturating_sub(listing.sold_share_amount)
        .saturating_sub(listing.reserved_share_amount);
    require!(
        amount > 0 && amount <= available,
        MarketplaceError::InvalidShareAmount
    );
    require!(
        !ctx.accounts.position.cancelled,
        MarketplaceError::PositionCancelled
    );
    require!(
        ctx.accounts
            .config
            .accepted_payment_mints
            .contains(&ctx.accounts.payment_mint.key()),
        MarketplaceError::MintNotAccepted
    );
    let position_exists = ctx.accounts.position.investor != Pubkey::default();
    if position_exists {
        require!(
            ctx.accounts.position.payment_mint == ctx.accounts.payment_mint.key()
                && ctx.accounts.position.payment_account == ctx.accounts.investor_payment.key(),
            MarketplaceError::PaymentMintMismatch
        );
    }
    // Ownership cap against everything the position would control, held or
    // reserved.
    let owned_after = (ctx.accounts.position.share_amount as u64)
        .checked_add(ctx.accounts.position.reserved_share_amount as u64)
        .and_then(|owned| owned.checked_add(amount as u64))
        .ok_or(MarketplaceError::Overflow)?;
    listing.require_below_ownership_cap(owned_after, ctx.accounts.property.share_amount)?;

    let (funds, fee, tax) = crate::instructions::buy::price_purchase(
        listing,
        amount,
        ctx.accounts.payment_mint.decimals,
    )?;
    let tax = crate::instructions::buy::charged_tax(listing, tax);
    let total = funds
        .checked_add(fee)
        .and_then(|t| t.checked_add(tax))
        .ok_or(MarketplaceError::Overflow)?;
    require!(total <= max_total_cost, MarketplaceError::CostTooHigh);

    // The reserved money stays in the wallet, but it must actually be
    // there, on top of anything already reserved.
    let reservation = &mut ctx.accounts.reservation;
    let reserved_after = reservation
        .amount
        .checked_add(total)
        .ok_or(MarketplaceError::Overflow)?;
    require!(
        ctx.accounts.investor_payment.amount >= reserved_after,
        MarketplaceError::BalanceTooLow
    );
    if reservation.token_account == Pubkey::default() {
        reservation.token_account = ctx.accounts.investor_payment.key();
        reservation.bump = ctx.bumps.reservation;
    }
    reservation.amount = reserved_after;

    let position = &mut ctx.accounts.position;
    if !position_exists {
        position.listing_id = listing_id;
        position.investor = ctx.accounts.investor.key();
        position.payment_mint = ctx.accounts.payment_mint.key();
        position.payment_account = ctx.accounts.investor_payment.key();
        position.cancelled = false;
        position.bump = ctx.bumps.position;
    }
    position.reserved_share_amount = position
        .reserved_share_amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;
    position.reserved_funds = position
        .reserved_funds
        .checked_add(funds)
        .ok_or(MarketplaceError::Overflow)?;
    position.reserved_fee = position
        .reserved_fee
        .checked_add(fee)
        .ok_or(MarketplaceError::Overflow)?;
    position.reserved_tax = position
        .reserved_tax
        .checked_add(tax)
        .ok_or(MarketplaceError::Overflow)?;

    let listing = &mut ctx.accounts.listing;
    if !position_exists {
        listing.position_count = listing
            .position_count
            .checked_add(1)
            .ok_or(MarketplaceError::Overflow)?;
    }
    listing.reserved_share_amount = listing
        .reserved_share_amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(SharesReserved {
        listing_id,
        investor: ctx.accounts.investor.key(),
        amount,
        payment_mint: ctx.accounts.position.payment_mint,
        reserved: total,
    });
    Ok(())
}

/// Claim reserved shares once the SPV exists: the reserved money finally
/// moves into the listing vault and the shares are delivered through the
/// usual airlock. All or nothing per position. Compliance-gated: this is the
/// purchase itself.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct ClaimShares<'info> {
    pub investor: Signer<'info>,

    /// The sponsor wallet fronting rent for the investor's accounts.
    #[account(mut, address = config.rent_collector @ MarketplaceError::NotRentCollector)]
    pub payer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The caller's RealEstateInvestor role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            investor.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = investor_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub investor_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, investor.key().as_ref()],
        bump = investor_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = investor_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub investor_compliance: Box<Account<'info, Compliance>>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    #[account(
        mut,
        seeds = [PROPERTY_SEED, &listing_id.to_le_bytes()],
        bump = property.bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    #[account(
        mut,
        seeds = [POSITION_SEED, &listing_id.to_le_bytes(), investor.key().as_ref()],
        bump = position.bump,
    )]
    pub position: Box<Account<'info, InvestorPosition>>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + ShareHolding::INIT_SPACE,
        seeds = [SHARE_SEED, &listing_id.to_le_bytes(), investor.key().as_ref()],
        bump,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    /// CHECK: the mint the position reserved in, pinned by address; kept
    /// untyped to spare `try_accounts` stack, the reserve already vetted it.
    #[account(address = position.payment_mint @ MarketplaceError::PaymentMintMismatch)]
    pub payment_mint: UncheckedAccount<'info>,

    /// CHECK: the reserved payment account the money finally leaves, pinned
    /// by address; the token program rules on it during the transfer.
    #[account(
        mut,
        address = position.payment_account @ MarketplaceError::PaymentMintMismatch,
    )]
    pub investor_payment: UncheckedAccount<'info>,

    /// The reservation being spent down.
    #[account(
        mut,
        seeds = [RESERVATION_SEED, investor_payment.key().as_ref()],
        bump = reservation.bump,
    )]
    pub reservation: Box<Account<'info, Reservation>>,

    /// CHECK: the listing vault authority; a bare PDA owning the vault's
    /// token accounts.
    #[account(seeds = [LISTING_VAULT_SEED, &listing_id.to_le_bytes()], bump)]
    pub listing_vault: UncheckedAccount<'info>,

    /// CHECK: the vault's associated account for the payment mint; created
    /// idempotently, so the ATA program verifies the derivation.
    #[account(mut)]
    pub listing_payment_account: UncheckedAccount<'info>,

    /// CHECK: the share mint PDA (owned by the Token-2022 program).
    #[account(seeds = [SHARE_MINT_SEED, &listing_id.to_le_bytes()], bump)]
    pub share_mint: UncheckedAccount<'info>,

    /// CHECK: the share mint's authority PDA; signs the lock-state changes.
    #[account(seeds = [MINT_AUTH_SEED, &listing_id.to_le_bytes()], bump)]
    pub mint_auth: UncheckedAccount<'info>,

    /// CHECK: the property vault authority; owner of the undistributed shares.
    #[account(seeds = [PROPERTY_VAULT_SEED, &listing_id.to_le_bytes()], bump)]
    pub property_vault: UncheckedAccount<'info>,

    /// CHECK: the vault's share account the delivery is pulled from, pinned
    /// to its derivation.
    #[account(
        mut,
        address = anchor_spl::associated_token::get_associated_token_address_with_program_id(
            &property_vault.key(),
            &share_mint.key(),
            &share_token_program.key(),
        ) @ MarketplaceError::WrongVaultAccount,
    )]
    pub vault_share_account: UncheckedAccount<'info>,

    /// CHECK: the investor's associated share account; created idempotently,
    /// so the ATA program verifies the derivation.
    #[account(mut)]
    pub investor_share_account: UncheckedAccount<'info>,

    /// The payment mint's token program (classic or Token-2022).
    pub payment_token_program: Interface<'info, TokenInterface>,
    /// The share mint's program is always Token-2022.
    pub share_token_program: Program<'info, Token2022>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn claim_shares_handler(ctx: Context<ClaimShares>, listing_id: u64) -> Result<()> {
    let listing = &ctx.accounts.listing;
    require!(
        listing.status == ListingStatus::Listed,
        MarketplaceError::ListingNotActive
    );
    let now = Clock::get()?.unix_timestamp;
    require!(
        now < listing.listing_expiry,
        MarketplaceError::ListingExpired
    );
    require!(
        listing.claim_deadline != 0 && now < listing.claim_deadline,
        MarketplaceError::ClaimWindowClosed
    );
    let amount = ctx.accounts.position.reserved_share_amount;
    require!(amount > 0, MarketplaceError::NothingReserved);
    require!(
        !ctx.accounts.position.cancelled,
        MarketplaceError::PositionCancelled
    );
    let total = ctx
        .accounts
        .position
        .reserved_funds
        .checked_add(ctx.accounts.position.reserved_fee)
        .and_then(|t| t.checked_add(ctx.accounts.position.reserved_tax))
        .ok_or(MarketplaceError::Overflow)?;

    let reservation = &mut ctx.accounts.reservation;
    reservation.amount = reservation
        .amount
        .checked_sub(total)
        .ok_or(MarketplaceError::Overflow)?;

    create_idempotent(CpiContext::new(
        ctx.accounts.associated_token_program.key(),
        CreateAta {
            payer: ctx.accounts.payer.to_account_info(),
            associated_token: ctx.accounts.listing_payment_account.to_account_info(),
            authority: ctx.accounts.listing_vault.to_account_info(),
            mint: ctx.accounts.payment_mint.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            token_program: ctx.accounts.payment_token_program.to_account_info(),
        },
    ))?;
    let mint_decimals = {
        let data = ctx.accounts.payment_mint.try_borrow_data()?;
        StateWithExtensions::<MintState>::unpack(&data)?
            .base
            .decimals
    };
    // The payment itself doubles as the check that the promised money is
    // still there: the token program rejects the transfer if it is not.
    anchor_spl::token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.payment_token_program.key(),
            anchor_spl::token_interface::TransferChecked {
                from: ctx.accounts.investor_payment.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                to: ctx.accounts.listing_payment_account.to_account_info(),
                authority: ctx.accounts.investor.to_account_info(),
            },
        ),
        total,
        mint_decimals,
    )?;

    // Shares to the investor through the usual airlock.
    let id_bytes = listing_id.to_le_bytes();
    let auth_seeds: &[&[u8]] = &[MINT_AUTH_SEED, &id_bytes, &[ctx.bumps.mint_auth]];
    let vault_seeds: &[&[u8]] = &[PROPERTY_VAULT_SEED, &id_bytes, &[ctx.bumps.property_vault]];
    create_idempotent(CpiContext::new(
        ctx.accounts.associated_token_program.key(),
        CreateAta {
            payer: ctx.accounts.payer.to_account_info(),
            associated_token: ctx.accounts.investor_share_account.to_account_info(),
            authority: ctx.accounts.investor.to_account_info(),
            mint: ctx.accounts.share_mint.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            token_program: ctx.accounts.share_token_program.to_account_info(),
        },
    ))?;
    thaw_account(CpiContext::new_with_signer(
        ctx.accounts.share_token_program.key(),
        ThawAccount {
            account: ctx.accounts.investor_share_account.to_account_info(),
            mint: ctx.accounts.share_mint.to_account_info(),
            authority: ctx.accounts.mint_auth.to_account_info(),
        },
        &[auth_seeds],
    ))?;
    anchor_spl::token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.share_token_program.key(),
            anchor_spl::token_interface::TransferChecked {
                from: ctx.accounts.vault_share_account.to_account_info(),
                mint: ctx.accounts.share_mint.to_account_info(),
                to: ctx.accounts.investor_share_account.to_account_info(),
                authority: ctx.accounts.property_vault.to_account_info(),
            },
            &[vault_seeds],
        ),
        amount as u64,
        0,
    )?;
    freeze_account(CpiContext::new_with_signer(
        ctx.accounts.share_token_program.key(),
        FreezeAccount {
            account: ctx.accounts.investor_share_account.to_account_info(),
            mint: ctx.accounts.share_mint.to_account_info(),
            authority: ctx.accounts.mint_auth.to_account_info(),
        },
        &[auth_seeds],
    ))?;

    let holding = &mut ctx.accounts.holding;
    if holding.owner == Pubkey::default() {
        holding.asset_id = listing_id;
        holding.owner = ctx.accounts.investor.key();
        holding.locks = [0; LOCK_REASONS];
        holding.listed = 0;
        holding.bump = ctx.bumps.holding;
        ctx.accounts.property.holder_count = ctx
            .accounts
            .property
            .holder_count
            .checked_add(1)
            .ok_or(MarketplaceError::Overflow)?;
    }
    holding.amount = holding
        .amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;

    // The reserved side is now the paid side: the money sits in the vault
    // and the same numbers back the refund paths.
    let position = &mut ctx.accounts.position;
    let (funds, fee, tax) = (
        position.reserved_funds,
        position.reserved_fee,
        position.reserved_tax,
    );
    position.reserved_share_amount = 0;
    position.share_amount = position
        .share_amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;
    position.paid_funds = position
        .paid_funds
        .checked_add(position.reserved_funds)
        .ok_or(MarketplaceError::Overflow)?;
    position.paid_fee = position
        .paid_fee
        .checked_add(position.reserved_fee)
        .ok_or(MarketplaceError::Overflow)?;
    position.paid_tax = position
        .paid_tax
        .checked_add(position.reserved_tax)
        .ok_or(MarketplaceError::Overflow)?;
    position.reserved_funds = 0;
    position.reserved_fee = 0;
    position.reserved_tax = 0;

    let listing = &mut ctx.accounts.listing;
    // When the developer covers the tax the buyer wasn't charged it, but the
    // obligation still accrues; settlement takes it out of their proceeds.
    // Derived from the funds this claim pays, so a price update between the
    // reservation and the claim can't skew it.
    let tax_owed = if listing.tax_paid_by_developer {
        crate::instructions::buy::bps_of(funds, listing.tax_bps)?
    } else {
        tax
    };
    let fee_quote = crate::instructions::buy::scale_from_mint(fee, mint_decimals)?;
    listing.record_collected(
        ctx.accounts.payment_mint.key(),
        funds,
        fee,
        fee_quote,
        tax_owed,
    )?;
    listing.reserved_share_amount = listing
        .reserved_share_amount
        .checked_sub(amount)
        .ok_or(MarketplaceError::Overflow)?;
    listing.sold_share_amount = listing
        .sold_share_amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;
    if listing.sold_share_amount == listing.listed_share_amount {
        listing.status = ListingStatus::SoldOut;
        listing.legal_deadline = now
            .checked_add(listing.legal_process_time)
            .ok_or(MarketplaceError::Overflow)?;
    }

    emit!(SharesClaimed {
        listing_id,
        investor: ctx.accounts.investor.key(),
        amount,
        payment_mint: ctx.accounts.position.payment_mint,
        paid: total,
        sold_out: ctx.accounts.listing.status == ListingStatus::SoldOut,
    });
    Ok(())
}

/// Walk away from a reservation. The money never moved, so there is nothing
/// to refund; the reservation just lets go of it. Only while the sale is
/// still filling: once every share is reserved the sale locks in and the SPV
/// incorporates against it, so from there a reservation is claimed or
/// expires. One-way: the position stays open with `cancelled` set, and this
/// investor can never buy into this listing again. Not role-gated: exits
/// never are.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct UnreserveShares<'info> {
    pub investor: Signer<'info>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    #[account(
        mut,
        seeds = [POSITION_SEED, &listing_id.to_le_bytes(), investor.key().as_ref()],
        bump = position.bump,
    )]
    pub position: Box<Account<'info, InvestorPosition>>,

    #[account(
        mut,
        seeds = [RESERVATION_SEED, position.payment_account.as_ref()],
        bump = reservation.bump,
    )]
    pub reservation: Box<Account<'info, Reservation>>,
}

pub fn unreserve_shares_handler(ctx: Context<UnreserveShares>, listing_id: u64) -> Result<()> {
    // Both checks matter: the deadline stays set after missed reservations
    // are swept, and the sale can be fully reserved before the SPV attests.
    let listing = &ctx.accounts.listing;
    let committed = listing
        .reserved_share_amount
        .checked_add(listing.sold_share_amount)
        .ok_or(MarketplaceError::Overflow)?;
    require!(
        listing.claim_deadline == 0 && committed < listing.listed_share_amount,
        MarketplaceError::SaleLocked
    );
    let amount = ctx.accounts.position.reserved_share_amount;
    require!(amount > 0, MarketplaceError::NothingReserved);
    release_position_reservation(
        &mut ctx.accounts.listing,
        &mut ctx.accounts.position,
        &mut ctx.accounts.reservation,
    )?;
    ctx.accounts.position.cancelled = true;

    emit!(SharesUnreserved {
        listing_id,
        investor: ctx.accounts.investor.key(),
        amount,
    });
    Ok(())
}

/// Sweep a reservation whose claim can no longer happen: the window closed,
/// the listing expired, or the sale left `Listed`. Permissionless; the
/// position closes and the sponsor takes its rent back. No fault, no flag:
/// the investor may buy directly later.
#[derive(Accounts)]
#[instruction(listing_id: u64, investor: Pubkey)]
pub struct ReleaseReservation<'info> {
    pub cranker: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// CHECK: the sponsor wallet that fronted the position's rent.
    #[account(mut, address = config.rent_collector @ MarketplaceError::NotRentCollector)]
    pub rent_collector: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    #[account(
        mut,
        close = rent_collector,
        seeds = [POSITION_SEED, &listing_id.to_le_bytes(), investor.as_ref()],
        bump = position.bump,
    )]
    pub position: Box<Account<'info, InvestorPosition>>,

    #[account(
        mut,
        seeds = [RESERVATION_SEED, position.payment_account.as_ref()],
        bump = reservation.bump,
    )]
    pub reservation: Box<Account<'info, Reservation>>,
}

pub fn release_reservation_handler(
    ctx: Context<ReleaseReservation>,
    listing_id: u64,
    investor: Pubkey,
) -> Result<()> {
    let listing = &ctx.accounts.listing;
    let now = Clock::get()?.unix_timestamp;
    let claim_possible = listing.status == ListingStatus::Listed
        && now < listing.listing_expiry
        && (listing.claim_deadline == 0 || now < listing.claim_deadline);
    require!(!claim_possible, MarketplaceError::ListingStillActive);
    let amount = ctx.accounts.position.reserved_share_amount;
    require!(amount > 0, MarketplaceError::NothingReserved);
    // A paid position must exit through its own withdraw. Paired with the
    // withdraw-side guard this would trap a paid-and-reserved position;
    // `create_spv`'s full-reservation rule is what makes that unreachable.
    require!(
        ctx.accounts.position.share_amount == 0,
        MarketplaceError::SharesOutstanding
    );
    release_position_reservation(
        &mut ctx.accounts.listing,
        &mut ctx.accounts.position,
        &mut ctx.accounts.reservation,
    )?;
    ctx.accounts.listing.position_count = ctx
        .accounts
        .listing
        .position_count
        .checked_sub(1)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(ReservationReleased {
        listing_id,
        investor,
        amount,
    });
    Ok(())
}

/// Take a position's reserved cost back out of the reservation and the
/// listing counter, zeroing the position's reserved side.
fn release_position_reservation(
    listing: &mut Listing,
    position: &mut InvestorPosition,
    reservation: &mut Reservation,
) -> Result<()> {
    let total = position
        .reserved_funds
        .checked_add(position.reserved_fee)
        .and_then(|t| t.checked_add(position.reserved_tax))
        .ok_or(MarketplaceError::Overflow)?;
    reservation.amount = reservation
        .amount
        .checked_sub(total)
        .ok_or(MarketplaceError::Overflow)?;
    listing.reserved_share_amount = listing
        .reserved_share_amount
        .checked_sub(position.reserved_share_amount)
        .ok_or(MarketplaceError::Overflow)?;
    position.reserved_share_amount = 0;
    position.reserved_funds = 0;
    position.reserved_fee = 0;
    position.reserved_tax = 0;
    Ok(())
}

/// Reclaim the rent of a reservation that has wound down to zero.
/// Permissionless; the rent goes back to the sponsor that fronted it.
#[derive(Accounts)]
pub struct CloseReservation<'info> {
    pub cranker: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// CHECK: the sponsor wallet that fronted the reservation's rent.
    #[account(mut, address = config.rent_collector @ MarketplaceError::NotRentCollector)]
    pub rent_collector: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_collector,
        seeds = [RESERVATION_SEED, reservation.token_account.as_ref()],
        bump = reservation.bump,
        constraint = reservation.amount == 0 @ MarketplaceError::NothingReserved,
    )]
    pub reservation: Box<Account<'info, Reservation>>,
}

pub fn close_reservation_handler(_ctx: Context<CloseReservation>) -> Result<()> {
    Ok(())
}

/// Reclaim the rent of a cancelled position once the listing has left
/// `Listed`. Permissionless: cancelled investors have nothing locked and no
/// reason to come back, so a crank sweeps the accounts and the sponsor gets
/// its rent back. Never earlier, because the open position is what bars a
/// cancelled investor from re-buying.
#[derive(Accounts)]
#[instruction(listing_id: u64, investor: Pubkey)]
pub struct CloseCancelledPosition<'info> {
    pub cranker: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// CHECK: the sponsor wallet that fronted the position's rent.
    #[account(mut, address = config.rent_collector @ MarketplaceError::NotRentCollector)]
    pub rent_collector: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    #[account(
        mut,
        close = rent_collector,
        seeds = [POSITION_SEED, &listing_id.to_le_bytes(), investor.as_ref()],
        bump = position.bump,
        constraint = position.cancelled @ MarketplaceError::PositionNotCancelled,
        constraint = position.reserved_share_amount == 0
            @ MarketplaceError::ReservationOutstanding,
    )]
    pub position: Box<Account<'info, InvestorPosition>>,
}

pub fn close_cancelled_position_handler(
    ctx: Context<CloseCancelledPosition>,
    listing_id: u64,
    investor: Pubkey,
) -> Result<()> {
    // Past expiry the flag is moot even if nobody flipped the status: buy
    // refuses expired listings on its own, so the windows still can't overlap.
    require!(
        ctx.accounts.listing.status != ListingStatus::Listed
            || Clock::get()?.unix_timestamp >= ctx.accounts.listing.listing_expiry,
        MarketplaceError::ListingStillActive
    );
    let listing = &mut ctx.accounts.listing;
    listing.position_count = listing
        .position_count
        .checked_sub(1)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(CancelledPositionClosed {
        listing_id,
        investor,
    });
    Ok(())
}

#[event]
pub struct SharesReserved {
    pub listing_id: u64,
    pub investor: Pubkey,
    pub amount: u32,
    pub payment_mint: Pubkey,
    pub reserved: u64,
}

#[event]
pub struct SharesClaimed {
    pub listing_id: u64,
    pub investor: Pubkey,
    pub amount: u32,
    pub payment_mint: Pubkey,
    pub paid: u64,
    pub sold_out: bool,
}

#[event]
pub struct SharesUnreserved {
    pub listing_id: u64,
    pub investor: Pubkey,
    pub amount: u32,
}

#[event]
pub struct ReservationReleased {
    pub listing_id: u64,
    pub investor: Pubkey,
    pub amount: u32,
}

#[event]
pub struct CancelledPositionClosed {
    pub listing_id: u64,
    pub investor: Pubkey,
}
