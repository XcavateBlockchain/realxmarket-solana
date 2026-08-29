use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{CONFIG_SEED, LOCATION_SEED, REGION_SEED, VAULT_SEED};
use crate::error::RegionsError;
use crate::state::{Config, Location, Region, POSTCODE_MAX_LEN};
use crate::vault::{lock_to_vault, release_from_vault};

use xcavate_whitelist::state::{Role, RoleAccount};

/// Registers a postcode as a valid property location in a region. Operator-only.
/// The operator locks a location deposit on top of their bond; it counts toward
/// the region's collateral, so a replacement operator has to match it. The PDA
/// existing is what the marketplace checks when a property is listed.
#[derive(Accounts)]
#[instruction(region_id: u16, postcode: Vec<u8>)]
pub struct CreateNewLocation<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            operator.key().as_ref(),
            &[Role::RegionalOperator.seed_byte()],
        ],
        bump = operator_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub operator_role: Box<Account<'info, RoleAccount>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ RegionsError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The operator's XCAV account the location deposit is pulled from.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = operator,
    )]
    pub operator_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The protocol's XCAV vault.
    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        token::mint = config.xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [REGION_SEED, &region_id.to_le_bytes()],
        bump = region.bump,
        constraint = region.owner == operator.key() @ RegionsError::NotRegionOwner,
    )]
    pub region: Box<Account<'info, Region>>,

    /// `init` doubles as the duplicate check: registering the same postcode
    /// twice fails on the second create.
    #[account(
        init,
        payer = operator,
        space = 8 + Location::INIT_SPACE,
        seeds = [LOCATION_SEED, &region_id.to_le_bytes(), postcode.as_slice()],
        bump,
    )]
    pub location: Box<Account<'info, Location>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn create_new_location_handler(
    ctx: Context<CreateNewLocation>,
    region_id: u16,
    postcode: Vec<u8>,
    max_deposit: u64,
) -> Result<()> {
    // Uppercase alphanumeric ASCII only, so "sw1a1aa" and "SW1A1AA" can't
    // register as two different locations. The frontend strips spaces.
    require!(
        !postcode.is_empty()
            && postcode.len() <= POSTCODE_MAX_LEN
            && postcode
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
        RegionsError::InvalidPostcode
    );
    // Once the seat is open the incumbent is a lame duck: letting them add
    // locations then would reprice the takeover for every challenger.
    require!(
        Clock::get()?.unix_timestamp < ctx.accounts.region.next_owner_change,
        RegionsError::SeatOpen
    );

    let deposit = ctx.accounts.config.location_deposit;
    require!(deposit <= max_deposit, RegionsError::DepositTooHigh);
    lock_to_vault(
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.operator_token.to_account_info(),
        &ctx.accounts.xcav_mint.to_account_info(),
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.operator.to_account_info(),
        deposit,
        ctx.accounts.xcav_mint.decimals,
    )?;

    let region = &mut ctx.accounts.region;
    region.collateral = region
        .collateral
        .checked_add(deposit)
        .ok_or(RegionsError::Overflow)?;
    region.location_collateral = region
        .location_collateral
        .checked_add(deposit)
        .ok_or(RegionsError::Overflow)?;
    region.location_count = region
        .location_count
        .checked_add(1)
        .ok_or(RegionsError::Overflow)?;

    let location = &mut ctx.accounts.location;
    location.region_id = region_id;
    location.postcode = postcode.clone();
    location.deposit = deposit;
    location.bump = ctx.bumps.location;

    emit!(LocationCreated {
        region_id,
        postcode,
        new_collateral: region.collateral,
        location_count: region.location_count,
    });
    Ok(())
}

/// Deregister a postcode and release its recorded deposit back to the
/// operator. Operator-only, and blocked while the seat is open, mirroring
/// create. New listings can no longer cite the postcode; live listings keep
/// the copy they snapshotted.
#[derive(Accounts)]
#[instruction(region_id: u16, postcode: Vec<u8>)]
pub struct RemoveLocation<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            operator.key().as_ref(),
            &[Role::RegionalOperator.seed_byte()],
        ],
        bump = operator_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub operator_role: Box<Account<'info, RoleAccount>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ RegionsError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The operator's XCAV account the deposit is returned to.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = operator,
    )]
    pub operator_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The protocol's XCAV vault.
    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
        token::mint = config.xcav_mint,
        token::authority = config,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [REGION_SEED, &region_id.to_le_bytes()],
        bump = region.bump,
        constraint = region.owner == operator.key() @ RegionsError::NotRegionOwner,
    )]
    pub region: Box<Account<'info, Region>>,

    #[account(
        mut,
        close = operator,
        seeds = [LOCATION_SEED, &region_id.to_le_bytes(), postcode.as_slice()],
        bump = location.bump,
    )]
    pub location: Box<Account<'info, Location>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn remove_location_handler(
    ctx: Context<RemoveLocation>,
    region_id: u16,
    postcode: Vec<u8>,
) -> Result<()> {
    // Same lame-duck rule as create: an open seat's collateral is spoken for.
    require!(
        Clock::get()?.unix_timestamp < ctx.accounts.region.next_owner_change,
        RegionsError::SeatOpen
    );

    let deposit = ctx.accounts.location.deposit;
    release_from_vault(
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.xcav_mint.to_account_info(),
        &ctx.accounts.operator_token.to_account_info(),
        &ctx.accounts.config.to_account_info(),
        ctx.accounts.config.bump,
        deposit,
        ctx.accounts.xcav_mint.decimals,
    )?;

    let region = &mut ctx.accounts.region;
    region.collateral = region
        .collateral
        .checked_sub(deposit)
        .ok_or(RegionsError::Overflow)?;
    region.location_collateral = region
        .location_collateral
        .checked_sub(deposit)
        .ok_or(RegionsError::Overflow)?;
    region.location_count = region
        .location_count
        .checked_sub(1)
        .ok_or(RegionsError::Overflow)?;

    emit!(LocationRemoved {
        region_id,
        postcode,
        new_collateral: region.collateral,
        location_count: region.location_count,
    });
    Ok(())
}

/// Operator-only setters for a region's marketplace parameters. One accounts
/// struct serves both, since they gate identically.
#[derive(Accounts)]
#[instruction(region_id: u16)]
pub struct AdjustRegion<'info> {
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            operator.key().as_ref(),
            &[Role::RegionalOperator.seed_byte()],
        ],
        bump = operator_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub operator_role: Box<Account<'info, RoleAccount>>,

    #[account(
        mut,
        seeds = [REGION_SEED, &region_id.to_le_bytes()],
        bump = region.bump,
        constraint = region.owner == operator.key() @ RegionsError::NotRegionOwner,
    )]
    pub region: Box<Account<'info, Region>>,
}

/// Change how long new property listings in the region stay active. Only
/// affects listings created after the change; live listings keep the duration
/// they were listed under.
pub fn adjust_listing_duration_handler(
    ctx: Context<AdjustRegion>,
    region_id: u16,
    listing_duration: i64,
) -> Result<()> {
    require!(
        listing_duration > 0 && listing_duration <= ctx.accounts.config.max_listing_duration,
        RegionsError::InvalidListingDuration
    );
    ctx.accounts.region.listing_duration = listing_duration;

    emit!(ListingDurationChanged {
        region_id,
        listing_duration,
    });
    Ok(())
}

/// Change the region's property sale tax. Listings snapshot the tax at listing
/// time, so a change never reprices a sale already underway.
pub fn adjust_region_tax_handler(
    ctx: Context<AdjustRegion>,
    region_id: u16,
    tax_bps: u16,
) -> Result<()> {
    require!(
        tax_bps <= ctx.accounts.config.max_tax_bps,
        RegionsError::TaxTooHigh
    );
    ctx.accounts.region.tax_bps = tax_bps;

    emit!(RegionTaxChanged { region_id, tax_bps });
    Ok(())
}

/// Change the region's seller and buyer fees. Listings snapshot both at
/// listing time, so a change never reprices a sale already underway.
pub fn adjust_region_fees_handler(
    ctx: Context<AdjustRegion>,
    region_id: u16,
    seller_fee_bps: u16,
    buyer_fee_bps: u16,
) -> Result<()> {
    require!(
        seller_fee_bps <= ctx.accounts.config.max_fee_bps
            && buyer_fee_bps <= ctx.accounts.config.max_fee_bps,
        RegionsError::FeeTooHigh
    );
    ctx.accounts.region.seller_fee_bps = seller_fee_bps;
    ctx.accounts.region.buyer_fee_bps = buyer_fee_bps;

    emit!(RegionFeesChanged {
        region_id,
        seller_fee_bps,
        buyer_fee_bps,
    });
    Ok(())
}

#[event]
pub struct LocationCreated {
    pub region_id: u16,
    pub postcode: Vec<u8>,
    pub new_collateral: u64,
    pub location_count: u32,
}

#[event]
pub struct LocationRemoved {
    pub region_id: u16,
    pub postcode: Vec<u8>,
    pub new_collateral: u64,
    pub location_count: u32,
}

#[event]
pub struct ListingDurationChanged {
    pub region_id: u16,
    pub listing_duration: i64,
}

#[event]
pub struct RegionTaxChanged {
    pub region_id: u16,
    pub tax_bps: u16,
}

#[event]
pub struct RegionFeesChanged {
    pub region_id: u16,
    pub seller_fee_bps: u16,
    pub buyer_fee_bps: u16,
}
