use anchor_lang::prelude::*;
use anchor_spl::associated_token::{create_idempotent, AssociatedToken, Create as CreateAta};
use anchor_spl::token_2022::{freeze_account, thaw_account, FreezeAccount, ThawAccount, Token2022};
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{
    CONFIG_SEED, LISTING_SEED, LISTING_VAULT_SEED, MINT_AUTH_SEED, POSITION_SEED, PROPERTY_SEED,
    PROPERTY_VAULT_SEED, SHARE_MINT_SEED, SHARE_SEED,
};
use crate::error::MarketplaceError;
use crate::state::{
    Config, InvestorPosition, Listing, ListingStatus, PropertyAsset, ShareHolding, LOCK_REASONS,
    PRICE_DECIMALS,
};

use xcavate_whitelist::state::{Compliance, Role, RoleAccount};

/// Rescale a value quoted at `PRICE_DECIMALS` to the payment mint's own
/// decimals, flooring so any dust favours the investor.
pub(crate) fn scale_to_mint(value: u64, mint_decimals: u8) -> Result<u64> {
    if mint_decimals >= PRICE_DECIMALS {
        let factor = 10u128.pow((mint_decimals - PRICE_DECIMALS) as u32);
        u64::try_from(value as u128 * factor).map_err(|_| MarketplaceError::Overflow.into())
    } else {
        let factor = 10u64.pow((PRICE_DECIMALS - mint_decimals) as u32);
        Ok(value / factor)
    }
}

/// The inverse: mint units back to the `PRICE_DECIMALS` quote, flooring.
pub(crate) fn scale_from_mint(value: u64, mint_decimals: u8) -> Result<u64> {
    if mint_decimals >= PRICE_DECIMALS {
        let factor = 10u64.pow((mint_decimals - PRICE_DECIMALS) as u32);
        Ok(value / factor)
    } else {
        let factor = 10u128.pow((PRICE_DECIMALS - mint_decimals) as u32);
        u64::try_from(value as u128 * factor).map_err(|_| MarketplaceError::Overflow.into())
    }
}

pub(crate) fn bps_of(value: u64, bps: u16) -> Result<u64> {
    u64::try_from(value as u128 * bps as u128 / 10_000)
        .map_err(|_| MarketplaceError::Overflow.into())
}

/// Price a purchase of `amount` shares off the listing snapshots, rescaled
/// to the payment mint: the funds, the investor fee, and the tax.
pub(crate) fn price_purchase(
    listing: &Listing,
    amount: u32,
    mint_decimals: u8,
) -> Result<(u64, u64, u64)> {
    let price_total = listing
        .share_price
        .checked_mul(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    let funds = scale_to_mint(price_total, mint_decimals)?;
    let fee = scale_to_mint(
        bps_of(price_total, listing.investor_fee_bps)?,
        mint_decimals,
    )?;
    // The tax is always part of the price: it becomes the buyer's surcharge
    // or the developer's obligation, depending on who covers it.
    let tax = scale_to_mint(bps_of(price_total, listing.tax_bps)?, mint_decimals)?;
    Ok((funds, fee, tax))
}

/// What the buyer is actually charged of the tax.
pub(crate) fn charged_tax(listing: &Listing, tax: u64) -> u64 {
    if listing.tax_paid_by_developer {
        0
    } else {
        tax
    }
}

/// Buy shares directly, the post-claim-window market: once the window has
/// run out, the funds land in the listing vault and the shares are delivered
/// to the investor in the same instruction. RealEstateInvestor-only and
/// compliance-gated, since this is investor money moving. The sponsor
/// `payer` fronts all account rent, per the rent policy; the investor
/// themselves never needs SOL.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct BuyPropertyShares<'info> {
    pub investor: Signer<'info>,

    /// The sponsor wallet fronting rent for the investor's accounts. Fixed to
    /// the configured rent collector, so the wallet paying the rent is also
    /// the one refunded when the accounts close.
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

    // init_if_needed is the natural fit for a per-investor upsert: the PDA is
    // seeded by the investor, so only they can target it, and a fresh init
    // zeroes `cancelled`. That's also why close_cancelled_position must only
    // run once the listing leaves Listed; closing earlier would let a
    // cancelled position re-init here with the flag wiped.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + InvestorPosition::INIT_SPACE,
        seeds = [POSITION_SEED, &listing_id.to_le_bytes(), investor.key().as_ref()],
        bump,
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

    /// The mint the investor pays in; must be on the accepted list.
    pub payment_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The investor's payment account the funds are pulled from.
    #[account(
        mut,
        token::mint = payment_mint,
        token::authority = investor,
    )]
    pub investor_payment: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: the listing vault authority; a bare PDA owning the vault's token accounts.
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

    /// The vault's share account the delivery is pulled from.
    #[account(
        mut,
        associated_token::mint = share_mint,
        associated_token::authority = property_vault,
        associated_token::token_program = share_token_program,
    )]
    pub vault_share_account: Box<InterfaceAccount<'info, TokenAccount>>,

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

pub fn buy_property_shares_handler(
    ctx: Context<BuyPropertyShares>,
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
    // Direct purchase is the post-claim-window market: before the SPV exists
    // money may only be reserved in place, and while claims run they have
    // first call on the shares.
    require!(
        listing.claim_deadline != 0 && now >= listing.claim_deadline,
        MarketplaceError::DirectBuyNotOpen
    );
    let available = listing
        .listed_share_amount
        .saturating_sub(listing.sold_share_amount)
        .saturating_sub(listing.reserved_share_amount);
    require!(
        amount > 0 && amount <= available,
        MarketplaceError::InvalidShareAmount
    );
    // The one-way unreserve bar: a cancelled position never buys again.
    require!(
        !ctx.accounts.position.cancelled,
        MarketplaceError::PositionCancelled
    );
    // A missed reservation must be released before this wallet buys.
    require!(
        ctx.accounts.position.reserved_share_amount == 0,
        MarketplaceError::ReservationOutstanding
    );
    require!(
        ctx.accounts
            .config
            .accepted_payment_mints
            .contains(&ctx.accounts.payment_mint.key()),
        MarketplaceError::MintNotAccepted
    );
    // One payment mint per position, so refunds are a single transfer.
    let position_exists = ctx.accounts.position.investor != Pubkey::default();
    if position_exists {
        require!(
            ctx.accounts.position.payment_mint == ctx.accounts.payment_mint.key(),
            MarketplaceError::PaymentMintMismatch
        );
    }
    // Ownership cap, against the snapshot taken at listing time. Holdings
    // must stay strictly below the cap (floored to whole shares), so at 50%
    // of 100 shares an investor tops out at 49.
    let owned_after = (ctx.accounts.holding.amount as u64)
        .checked_add(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    let max_shares = (listing.max_ownership_bps as u64)
        .checked_mul(ctx.accounts.property.share_amount as u64)
        .ok_or(MarketplaceError::Overflow)?
        / 10_000;
    require!(
        owned_after < max_shares,
        MarketplaceError::MaxOwnershipExceeded
    );

    // Price the purchase off the listing snapshots, then rescale to the
    // payment mint. The caller caps the total, so neither a price update nor
    // a config change can charge more than they signed for.
    let mint_decimals = ctx.accounts.payment_mint.decimals;
    let (funds, fee, tax) = price_purchase(listing, amount, mint_decimals)?;
    let buyer_tax = charged_tax(listing, tax);
    let total = funds
        .checked_add(fee)
        .and_then(|t| t.checked_add(buyer_tax))
        .ok_or(MarketplaceError::Overflow)?;
    require!(total <= max_total_cost, MarketplaceError::CostTooHigh);

    // Funds into the listing vault.
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
    transfer_checked(
        CpiContext::new(
            ctx.accounts.payment_token_program.key(),
            TransferChecked {
                from: ctx.accounts.investor_payment.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                to: ctx.accounts.listing_payment_account.to_account_info(),
                authority: ctx.accounts.investor.to_account_info(),
            },
        ),
        total,
        mint_decimals,
    )?;

    // Shares to the investor: their account starts non-transferable, so the
    // program opens it, delivers, and locks it again.
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
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.share_token_program.key(),
            TransferChecked {
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

    // Ledger updates.
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

    let position = &mut ctx.accounts.position;
    if !position_exists {
        position.listing_id = listing_id;
        position.investor = ctx.accounts.investor.key();
        position.payment_mint = ctx.accounts.payment_mint.key();
        position.payment_account = ctx.accounts.investor_payment.key();
        position.reserved_share_amount = 0;
        position.reserved_funds = 0;
        position.reserved_fee = 0;
        position.reserved_tax = 0;
        position.cancelled = false;
        position.bump = ctx.bumps.position;
    }
    position.share_amount = position
        .share_amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;
    position.paid_funds = position
        .paid_funds
        .checked_add(funds)
        .ok_or(MarketplaceError::Overflow)?;
    position.paid_fee = position
        .paid_fee
        .checked_add(fee)
        .ok_or(MarketplaceError::Overflow)?;
    position.paid_tax = position
        .paid_tax
        .checked_add(buyer_tax)
        .ok_or(MarketplaceError::Overflow)?;

    let listing = &mut ctx.accounts.listing;
    let fee_quote = bps_of(
        listing
            .share_price
            .checked_mul(amount as u64)
            .ok_or(MarketplaceError::Overflow)?,
        listing.investor_fee_bps,
    )?;
    listing.record_collected(ctx.accounts.payment_mint.key(), funds, fee, fee_quote, tax)?;
    if !position_exists {
        listing.position_count = listing
            .position_count
            .checked_add(1)
            .ok_or(MarketplaceError::Overflow)?;
    }
    listing.sold_share_amount = listing
        .sold_share_amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;
    // The last share locks the sale in; the legal process takes over from
    // here and unreserving is no longer possible. The timeout exit opens if
    // the lawyers haven't settled by the deadline.
    if listing.sold_share_amount == listing.listed_share_amount {
        listing.status = ListingStatus::SoldOut;
        listing.legal_deadline = Clock::get()?
            .unix_timestamp
            .checked_add(listing.legal_process_time)
            .ok_or(MarketplaceError::Overflow)?;
    }

    emit!(PropertySharesBought {
        listing_id,
        investor: ctx.accounts.investor.key(),
        amount,
        payment_mint: ctx.accounts.payment_mint.key(),
        paid_funds: funds,
        paid_fee: fee,
        paid_tax: buyer_tax,
        sold_out: ctx.accounts.listing.status == ListingStatus::SoldOut,
    });
    Ok(())
}

#[event]
pub struct PropertySharesBought {
    pub listing_id: u64,
    pub investor: Pubkey,
    pub amount: u32,
    pub payment_mint: Pubkey,
    pub paid_funds: u64,
    pub paid_fee: u64,
    pub paid_tax: u64,
    pub sold_out: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_is_identity_at_price_decimals() {
        assert_eq!(scale_to_mint(52_000_000_000, 9).unwrap(), 52_000_000_000);
    }

    #[test]
    fn scale_down_floors() {
        // 9 -> 6 decimals divides by 1_000, flooring the remainder away.
        assert_eq!(scale_to_mint(52_000_000_000, 6).unwrap(), 52_000_000);
        assert_eq!(scale_to_mint(1_999, 6).unwrap(), 1);
        assert_eq!(scale_to_mint(999, 6).unwrap(), 0);
    }

    #[test]
    fn scale_up_multiplies() {
        // 9 -> 12 decimals multiplies by 1_000.
        assert_eq!(scale_to_mint(5, 12).unwrap(), 5_000);
    }
}
