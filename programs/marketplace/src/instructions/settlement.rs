use anchor_lang::prelude::*;
use anchor_spl::token_2022::spl_token_2022::{
    extension::StateWithExtensions, state::Account as TokenAccountState, state::Mint as MintState,
};
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{
    CONFIG_SEED, LAWYER_SEED, LISTING_SEED, LISTING_VAULT_SEED, PROPERTY_SEED, VAULT_SEED,
};
use crate::error::MarketplaceError;
use crate::instructions::buy::{scale_from_mint, scale_to_mint};
use crate::state::{Config, Lawyer, Listing, ListingStatus, PropertyAsset};
use crate::vault::release_from_vault;

/// Settle an approved sale: everyone is paid in one transaction, per
/// collected mint. The developer takes the principal net of the seller
/// fee (and of the tax, when they cover it); the tax rides to whichever
/// lawyer remits it; the SPV lawyer draws their quoted costs from the fee
/// pot, while the developer's own lawyer is paid privately off chain; what's
/// left splits between the region's operator and the treasury at the
/// configured share, and the treasury also absorbs any rounding dust and
/// donations, so the vault drains to zero.
/// The deposit returns, both lawyers' case counts drop, and the sale is
/// final. Permissionless: every amount is fixed by the accounts.
///
/// The remaining accounts carry one (mint, vault account, developer,
/// developer's lawyer, SPV lawyer, region owner, treasury) group of token
/// accounts per collected mint, in the listing's collected order.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct ExecuteDeal<'info> {
    pub cranker: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

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

    /// The property's region, owned by the regions program; its operator
    /// takes the configured share of the leftover fees.
    #[account(
        seeds = [regions::REGION_SEED, &property.region_id.to_le_bytes()],
        bump = region.bump,
        seeds::program = regions::ID,
    )]
    pub region: Box<Account<'info, regions::state::Region>>,

    /// The developer's lawyer's registry entry; their case closes here.
    #[account(
        mut,
        seeds = [LAWYER_SEED, listing.developer_lawyer.lawyer.as_ref()],
        bump = developer_lawyer_registry.bump,
    )]
    pub developer_lawyer_registry: Box<Account<'info, Lawyer>>,

    /// The SPV lawyer's registry entry; their case closes here.
    #[account(
        mut,
        seeds = [LAWYER_SEED, listing.spv_lawyer.lawyer.as_ref()],
        bump = spv_lawyer_registry.bump,
    )]
    pub spv_lawyer_registry: Box<Account<'info, Lawyer>>,

    /// CHECK: the listing vault authority; signs every payout.
    #[account(seeds = [LISTING_VAULT_SEED, &listing_id.to_le_bytes()], bump)]
    pub listing_vault: UncheckedAccount<'info>,

    /// The XCAV mint (for the deposit release).
    #[account(address = config.xcav_mint @ MarketplaceError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The developer's XCAV account the deposit returns to.
    #[account(
        mut,
        token::mint = config.xcav_mint,
        token::authority = listing.developer,
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

    /// The XCAV mint's token program.
    pub token_program: Interface<'info, TokenInterface>,
    /// The payment mints' token program; a second one when they differ.
    pub payment_token_program: Interface<'info, TokenInterface>,
}

/// A token account's (owner, mint, amount), straight off the bytes.
fn unpack_token(info: &AccountInfo) -> Result<(Pubkey, Pubkey, u64)> {
    let data = info.try_borrow_data()?;
    let state = StateWithExtensions::<TokenAccountState>::unpack(&data)?.base;
    Ok((state.owner, state.mint, state.amount))
}

/// Draw as much of a quote-scale debt as this mint's pot can cover.
fn draw(due: &mut u64, pot: u64, decimals: u8) -> Result<u64> {
    let pay_quote = scale_from_mint(pot, decimals)?.min(*due);
    let cut = scale_to_mint(pay_quote, decimals)?;
    *due = due
        .checked_sub(pay_quote)
        .ok_or(MarketplaceError::Overflow)?;
    Ok(cut)
}

pub fn execute_deal_handler<'info>(
    ctx: Context<'info, ExecuteDeal<'info>>,
    listing_id: u64,
) -> Result<()> {
    let listing = &ctx.accounts.listing;
    require!(
        listing.status == ListingStatus::Legal,
        MarketplaceError::ListingNotActive
    );
    // Past the deadline the timeout refunds own the vault.
    require!(
        Clock::get()?.unix_timestamp <= listing.legal_deadline,
        MarketplaceError::LegalProcessExpired
    );
    require!(
        ctx.remaining_accounts.len() == listing.collected.len() * 7,
        MarketplaceError::InvalidConfig
    );

    let collected = listing.collected.clone();
    let developer = listing.developer;
    let developer_lawyer = listing.developer_lawyer.lawyer;
    let spv_lawyer = listing.spv_lawyer.lawyer;
    let mut spv_lawyer_due = listing.spv_lawyer.costs;

    let id_bytes = listing_id.to_le_bytes();
    let vault_seeds: &[&[u8]] = &[LISTING_VAULT_SEED, &id_bytes, &[ctx.bumps.listing_vault]];
    for (entry, group) in collected.iter().zip(ctx.remaining_accounts.chunks(7)) {
        let mint = &group[0];
        require!(mint.key() == entry.mint, MarketplaceError::InvalidMint);
        let token_program = mint.owner;
        require!(
            *token_program == ctx.accounts.token_program.key()
                || *token_program == ctx.accounts.payment_token_program.key(),
            MarketplaceError::InvalidMint
        );
        let vault_account = &group[1];
        require!(
            vault_account.key()
                == anchor_spl::associated_token::get_associated_token_address_with_program_id(
                    &ctx.accounts.listing_vault.key(),
                    &entry.mint,
                    token_program,
                ),
            MarketplaceError::WrongTokenAccount
        );
        // Each payee's account must hold this mint and belong to them; any
        // account of theirs will do, since the money is theirs either way.
        let payees = [
            developer,
            developer_lawyer,
            spv_lawyer,
            ctx.accounts.region.owner,
            ctx.accounts.config.treasury,
        ];
        for (info, expected) in group[2..].iter().zip(payees) {
            let (owner, account_mint, _) = unpack_token(info)?;
            require!(
                owner == expected && account_mint == entry.mint,
                MarketplaceError::WrongPayee
            );
        }
        let decimals = {
            let data = mint.try_borrow_data()?;
            StateWithExtensions::<MintState>::unpack(&data)?
                .base
                .decimals
        };

        // The split, all in this mint's units. The developer's share is the
        // principal net of the seller fee, and net of the tax when they
        // cover it; the tax lands with the lawyer who handles it.
        let seller_fee =
            u64::try_from(entry.funds as u128 * listing.seller_fee_bps as u128 / 10_000)
                .map_err(|_| MarketplaceError::Overflow)?;
        let mut developer_amount = entry
            .funds
            .checked_sub(seller_fee)
            .ok_or(MarketplaceError::Overflow)?;
        let (developer_lawyer_amount, mut spv_lawyer_amount) = if listing.tax_paid_by_developer {
            developer_amount = developer_amount
                .checked_sub(entry.tax)
                .ok_or(MarketplaceError::Overflow)?;
            (entry.tax, 0)
        } else {
            (0, entry.tax)
        };

        // The SPV lawyer draws their costs from the fee pot; the engagement
        // cap guaranteed it covers them in quote terms. The developer's own
        // lawyer is paid privately.
        let mut pot = entry
            .fee
            .checked_add(seller_fee)
            .ok_or(MarketplaceError::Overflow)?;
        let cut = draw(&mut spv_lawyer_due, pot, decimals)?;
        spv_lawyer_amount = spv_lawyer_amount
            .checked_add(cut)
            .ok_or(MarketplaceError::Overflow)?;
        pot = pot.checked_sub(cut).ok_or(MarketplaceError::Overflow)?;
        let region_amount = u64::try_from(
            pot as u128 * ctx.accounts.config.operator_fee_share_bps as u128 / 10_000,
        )
        .map_err(|_| MarketplaceError::Overflow)?;

        // The treasury takes everything else in the vault: its share of the
        // leftover fees, the rounding dust, and any donations.
        let (_, _, vault_balance) = unpack_token(vault_account)?;
        let treasury_amount = vault_balance
            .checked_sub(developer_amount)
            .and_then(|v| v.checked_sub(developer_lawyer_amount))
            .and_then(|v| v.checked_sub(spv_lawyer_amount))
            .and_then(|v| v.checked_sub(region_amount))
            .ok_or(MarketplaceError::Overflow)?;

        let amounts = [
            developer_amount,
            developer_lawyer_amount,
            spv_lawyer_amount,
            region_amount,
            treasury_amount,
        ];
        for (info, amount) in group[2..].iter().zip(amounts) {
            if amount == 0 {
                continue;
            }
            transfer_checked(
                CpiContext::new_with_signer(
                    *token_program,
                    TransferChecked {
                        from: vault_account.clone(),
                        mint: mint.clone(),
                        to: info.clone(),
                        authority: ctx.accounts.listing_vault.to_account_info(),
                    },
                    &[vault_seeds],
                ),
                amount,
                decimals,
            )?;
        }

        emit!(DealPayout {
            listing_id,
            payment_mint: entry.mint,
            developer: developer_amount,
            developer_lawyer: developer_lawyer_amount,
            spv_lawyer: spv_lawyer_amount,
            region_owner: region_amount,
            treasury: treasury_amount,
        });
    }

    // The deposit comes home with the completed sale.
    let deposit = ctx.accounts.listing.deposit;
    if deposit > 0 {
        release_from_vault(
            &ctx.accounts.token_program.to_account_info(),
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.xcav_mint.to_account_info(),
            &ctx.accounts.developer_token.to_account_info(),
            &ctx.accounts.config.to_account_info(),
            ctx.accounts.config.bump,
            deposit,
            ctx.accounts.xcav_mint.decimals,
        )?;
    }

    // Both cases close; the assignments stay on the listing as the record
    // of who settled the sale.
    for registry in [
        &mut ctx.accounts.developer_lawyer_registry,
        &mut ctx.accounts.spv_lawyer_registry,
    ] {
        registry.active_cases = registry
            .active_cases
            .checked_sub(1)
            .ok_or(MarketplaceError::Overflow)?;
    }

    let listing = &mut ctx.accounts.listing;
    listing.deposit = 0;
    listing.status = ListingStatus::Finalized;
    ctx.accounts.property.finalized = true;

    emit!(DealExecuted {
        listing_id,
        developer,
    });
    Ok(())
}

#[event]
pub struct DealPayout {
    pub listing_id: u64,
    pub payment_mint: Pubkey,
    pub developer: u64,
    pub developer_lawyer: u64,
    pub spv_lawyer: u64,
    pub region_owner: u64,
    pub treasury: u64,
}

#[event]
pub struct DealExecuted {
    pub listing_id: u64,
    pub developer: Pubkey,
}
