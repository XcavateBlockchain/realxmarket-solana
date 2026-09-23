use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, LISTING_SEED, PROPERTY_SEED, VAULT_SEED};
use crate::error::MarketplaceError;
use crate::state::{
    Config, LawyerAssignment, Listing, ListingStatus, PropertyAsset, SpvElection, MIN_SHARE_PRICE,
};
use crate::vault::lock_to_vault;

use xcavate_whitelist::state::{Compliance, Role, RoleAccount};

/// List a property for a primary share sale. RealEstateDeveloper-only, and one
/// of the compliance-gated calls: the role must exist AND carry the compliant
/// flag, since this starts a flow that takes investor funds. The location must
/// be registered in the region, the developer locks the listing deposit, and
/// the region's tax, fees and listing duration are snapshotted. The listing
/// starts as `PendingAssets`; `init_property_assets` creates the share mint
/// and opens it for purchases.
#[derive(Accounts)]
#[instruction(region_id: u16, postcode: Vec<u8>)]
pub struct ListProperty<'info> {
    #[account(mut)]
    pub developer: Signer<'info>,

    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

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

    /// The caller's compliance record, owned by the roles program.
    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, developer.key().as_ref()],
        bump = developer_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = developer_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub developer_compliance: Box<Account<'info, Compliance>>,

    /// The region the property sits in, owned by the regions program. Read for
    /// the tax, fee and listing-duration snapshot.
    #[account(
        seeds = [regions::REGION_SEED, &region_id.to_le_bytes()],
        bump = region.bump,
        seeds::program = regions::ID,
    )]
    pub region: Box<Account<'info, regions::state::Region>>,

    /// The property's location, owned by the regions program. Its existence is
    /// what makes the postcode valid to list under.
    #[account(
        seeds = [regions::LOCATION_SEED, &region_id.to_le_bytes(), postcode.as_slice()],
        bump = location.bump,
        seeds::program = regions::ID,
    )]
    pub location: Box<Account<'info, regions::state::Location>>,

    #[account(
        init,
        payer = developer,
        space = 8 + PropertyAsset::INIT_SPACE,
        seeds = [PROPERTY_SEED, &config.next_listing_id.to_le_bytes()],
        bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    #[account(
        init,
        payer = developer,
        space = 8 + Listing::INIT_SPACE,
        seeds = [LISTING_SEED, &config.next_listing_id.to_le_bytes()],
        bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    /// The mint the sale settles in, from the config allowlist.
    #[account(
        constraint = config.accepted_payment_mints.contains(&payment_mint.key())
            @ MarketplaceError::MintNotAccepted,
    )]
    pub payment_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ MarketplaceError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The developer's XCAV account the listing deposit is pulled from.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = developer,
    )]
    pub developer_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The protocol's XCAV vault.
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

pub fn list_property_handler(
    ctx: Context<ListProperty>,
    region_id: u16,
    postcode: Vec<u8>,
    share_price: u64,
    share_amount: u32,
    tax_paid_by_developer: bool,
    max_deposit: u64,
) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(
        share_amount >= config.min_property_shares && share_amount <= config.max_property_shares,
        MarketplaceError::InvalidShareAmount
    );
    require!(
        share_price >= MIN_SHARE_PRICE,
        MarketplaceError::InvalidSharePrice
    );
    // The full property must stay priceable in u64 for settlement math.
    share_price
        .checked_mul(share_amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    // The strict-below ownership cap must admit at least a one-share buy, or
    // the listing could never sell a single share.
    require!(
        (config.max_ownership_bps as u64)
            .checked_mul(share_amount as u64)
            .ok_or(MarketplaceError::Overflow)?
            / 10_000
            > 1,
        MarketplaceError::OwnershipCapTooTight
    );

    let listing_id = config.next_listing_id;
    let deposit = config.listing_deposit;
    // The deposit is read from live config; the caller caps what they are
    // willing to pay so an update can't reprice their signed transaction.
    require!(deposit <= max_deposit, MarketplaceError::DepositTooHigh);
    let now = Clock::get()?.unix_timestamp;
    let listing_expiry = now
        .checked_add(ctx.accounts.region.listing_duration)
        .ok_or(MarketplaceError::Overflow)?;
    // A developer-covered tax comes out of the sale proceeds next to the
    // seller fee; together they must fit inside the price, or the
    // settlement subtraction could never pay the developer out.
    if tax_paid_by_developer {
        require!(
            ctx.accounts.region.tax_bps as u32 + ctx.accounts.region.seller_fee_bps as u32
                <= 10_000,
            MarketplaceError::TaxExceedsProceeds
        );
    }

    lock_to_vault(
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.developer_token.to_account_info(),
        &ctx.accounts.xcav_mint.to_account_info(),
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.developer.to_account_info(),
        deposit,
        ctx.accounts.xcav_mint.decimals,
    )?;

    let property = &mut ctx.accounts.property;
    property.asset_id = listing_id;
    property.share_mint = Pubkey::default();
    property.region_id = region_id;
    property.location = postcode;
    property.share_amount = share_amount;
    property.spv_created = false;
    property.finalized = false;
    property.holder_count = 0;
    property.bump = ctx.bumps.property;

    let listing = &mut ctx.accounts.listing;
    listing.listing_id = listing_id;
    listing.developer = ctx.accounts.developer.key();
    listing.asset_id = listing_id;
    listing.share_price = share_price;
    listing.payment_mint = ctx.accounts.payment_mint.key();
    listing.listed_share_amount = share_amount;
    listing.sold_share_amount = 0;
    listing.reserved_share_amount = 0;
    listing.tax_paid_by_developer = tax_paid_by_developer;
    listing.tax_bps = ctx.accounts.region.tax_bps;
    listing.seller_fee_bps = ctx.accounts.region.seller_fee_bps;
    listing.buyer_fee_bps = ctx.accounts.region.buyer_fee_bps;
    listing.max_ownership_bps = config.max_ownership_bps;
    listing.listing_expiry = listing_expiry;
    listing.claiming_time = config.claiming_time;
    listing.claim_deadline = 0;
    listing.legal_process_time = config.legal_process_time;
    listing.lawyer_voting_time = config.lawyer_voting_time;
    listing.min_voting_quorum_bps = config.min_voting_quorum_bps;
    listing.legal_deadline = 0;
    listing.position_count = 0;
    listing.deposit = deposit;
    listing.developer_lawyer = LawyerAssignment::default();
    listing.spv_lawyer = LawyerAssignment::default();
    listing.second_attempt = false;
    listing.developer_engaged = false;
    listing.spv_costs_due = 0;
    listing.spv_costs_payee = Pubkey::default();
    listing.collected = Vec::new();
    listing.spv_election = SpvElection::default();
    listing.status = ListingStatus::PendingAssets;
    listing.bump = ctx.bumps.listing;

    ctx.accounts.config.next_listing_id = listing_id
        .checked_add(1)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(PropertyListed {
        listing_id,
        developer: listing.developer,
        region_id,
        payment_mint: listing.payment_mint,
        share_price,
        share_amount,
        listing_expiry,
    });
    Ok(())
}

/// Update a listing's share price. RealEstateDeveloper role and the listing's
/// developer only; allowed while the listing is open and at least one share
/// remains, so a price change can never touch a completed sale.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct UpgradeObject<'info> {
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

    /// Repricing steers investor money, so the developer's KYC is checked too.
    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, developer.key().as_ref()],
        bump = developer_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = developer_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub developer_compliance: Box<Account<'info, Compliance>>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
        constraint = listing.developer == developer.key() @ MarketplaceError::NotListingDeveloper,
    )]
    pub listing: Box<Account<'info, Listing>>,
}

pub fn upgrade_object_handler(
    ctx: Context<UpgradeObject>,
    listing_id: u64,
    new_price: u64,
) -> Result<()> {
    require!(
        new_price >= MIN_SHARE_PRICE,
        MarketplaceError::InvalidSharePrice
    );

    let listing = &mut ctx.accounts.listing;
    require!(
        matches!(
            listing.status,
            ListingStatus::PendingAssets | ListingStatus::Listed
        ),
        MarketplaceError::ListingNotActive
    );
    require!(
        Clock::get()?.unix_timestamp < listing.listing_expiry,
        MarketplaceError::ListingExpired
    );
    require!(
        listing.sold_share_amount < listing.listed_share_amount,
        MarketplaceError::PropertyAlreadySold
    );
    listing.share_price = new_price;

    emit!(ObjectUpdated {
        listing_id,
        new_price,
    });
    Ok(())
}

#[event]
pub struct PropertyListed {
    pub listing_id: u64,
    pub developer: Pubkey,
    pub region_id: u16,
    pub payment_mint: Pubkey,
    pub share_price: u64,
    pub share_amount: u32,
    pub listing_expiry: i64,
}

#[event]
pub struct ObjectUpdated {
    pub listing_id: u64,
    pub new_price: u64,
}
