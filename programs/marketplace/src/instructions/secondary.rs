use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::AccountsExit;
use anchor_spl::associated_token::{
    create_idempotent, get_associated_token_address_with_program_id, AssociatedToken,
    Create as CreateAta,
};
use anchor_spl::token_2022::spl_token_2022::{
    extension::StateWithExtensions, state::Mint as MintState,
};
use anchor_spl::token_2022::{freeze_account, thaw_account, FreezeAccount, ThawAccount, Token2022};
use anchor_spl::token_interface::{transfer_checked, TokenInterface, TransferChecked};

use crate::constants::{
    CONFIG_SEED, CPI_AUTH_SEED, INCOME_SEED, LISTING_SEED, MINT_AUTH_SEED, PROPERTY_PROGRAM,
    PROPERTY_SEED, SHARE_LISTING_SEED, SHARE_SEED,
};
use crate::error::MarketplaceError;
use crate::instructions::buy::{bps_of, scale_to_mint};
use crate::state::{
    Config, Listing, ListingStatus, PropertyAsset, ShareHolding, ShareListing, LOCK_REASONS,
    MIN_SHARE_PRICE,
};

use crate::compliance_guard::require_compliant;
use xcavate_whitelist::state::{Compliance, Role, RoleAccount};

/// Anchor discriminator of the property program's `settle_income`. Built by
/// hand because the crate dependency runs the other way (property depends
/// on this program), so the CPI can't use the generated client.
pub const SETTLE_INCOME_DISC: [u8; 8] = [228, 147, 202, 250, 235, 75, 170, 119];

/// Checkpoint a holder's accrued income at their current balance, before
/// the transfer changes it. The property program verifies every account
/// against its own seeds; this side only signs with the `cpi-auth` PDA that
/// gates the instruction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn settle_income<'info>(
    property_program: &AccountInfo<'info>,
    cpi_auth: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    income: &AccountInfo<'info>,
    holding: &AccountInfo<'info>,
    checkpoint: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    cpi_auth_bump: u8,
    asset_id: u64,
    owner: Pubkey,
) -> Result<()> {
    let mut data = SETTLE_INCOME_DISC.to_vec();
    data.extend_from_slice(&asset_id.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    let ix = Instruction {
        program_id: PROPERTY_PROGRAM,
        accounts: vec![
            AccountMeta::new_readonly(cpi_auth.key(), true),
            AccountMeta::new(payer.key(), true),
            AccountMeta::new_readonly(income.key(), false),
            AccountMeta::new_readonly(holding.key(), false),
            AccountMeta::new(checkpoint.key(), false),
            AccountMeta::new_readonly(system_program.key(), false),
        ],
        data,
    };
    let bump = [cpi_auth_bump];
    let seeds: &[&[u8]] = &[CPI_AUTH_SEED, &bump];
    invoke_signed(
        &ix,
        &[
            cpi_auth.clone(),
            payer.clone(),
            income.clone(),
            holding.clone(),
            checkpoint.clone(),
            system_program.clone(),
            property_program.clone(),
        ],
        &[seeds],
    )
    .map_err(Into::into)
}

/// Pin an account to its PDA through a stored bump: one fixed-cost
/// derivation, where `find_program_address` costs vary with the keys.
pub(crate) fn require_pda(
    actual: &Pubkey,
    seeds: &[&[u8]],
    program: &Pubkey,
    err: MarketplaceError,
) -> Result<()> {
    let expected = Pubkey::create_program_address(seeds, program).map_err(|_| err)?;
    require_keys_eq!(*actual, expected, err);
    Ok(())
}

/// Move shares between two wallets' Token-2022 accounts: create the
/// destination if needed, thaw both ends, transfer with the permanent
/// delegate signing, and freeze both again.
#[allow(clippy::too_many_arguments)]
pub(crate) fn move_shares<'info>(
    share_token_program: &AccountInfo<'info>,
    associated_token_program: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    share_mint: &AccountInfo<'info>,
    mint_auth: &AccountInfo<'info>,
    mint_auth_bump: u8,
    asset_id: u64,
    from_account: &AccountInfo<'info>,
    to_account: &AccountInfo<'info>,
    to_authority: &AccountInfo<'info>,
    amount: u32,
) -> Result<()> {
    let id_bytes = asset_id.to_le_bytes();
    let auth_seeds: &[&[u8]] = &[MINT_AUTH_SEED, &id_bytes, &[mint_auth_bump]];
    create_idempotent(CpiContext::new(
        associated_token_program.key(),
        CreateAta {
            payer: payer.clone(),
            associated_token: to_account.clone(),
            authority: to_authority.clone(),
            mint: share_mint.clone(),
            system_program: system_program.clone(),
            token_program: share_token_program.clone(),
        },
    ))?;
    for account in [from_account, to_account] {
        thaw_account(CpiContext::new_with_signer(
            share_token_program.key(),
            ThawAccount {
                account: account.clone(),
                mint: share_mint.clone(),
                authority: mint_auth.clone(),
            },
            &[auth_seeds],
        ))?;
    }
    transfer_checked(
        CpiContext::new_with_signer(
            share_token_program.key(),
            TransferChecked {
                from: from_account.clone(),
                mint: share_mint.clone(),
                to: to_account.clone(),
                authority: mint_auth.clone(),
            },
            &[auth_seeds],
        ),
        amount as u64,
        0,
    )?;
    for account in [from_account, to_account] {
        freeze_account(CpiContext::new_with_signer(
            share_token_program.key(),
            FreezeAccount {
                account: account.clone(),
                mint: share_mint.clone(),
                authority: mint_auth.clone(),
            },
            &[auth_seeds],
        ))?;
    }
    Ok(())
}

/// Put part of a holding up for sale on the secondary market. The shares
/// stay on the seller's ledger, still earning income, but are reserved
/// against voting and other transfers until sold or delisted.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct RelistShares<'info> {
    pub seller: Signer<'info>,

    /// Whoever fronts the rent: the seller on the default path, or any
    /// willing wallet. The listing remembers who to refund.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// The seller's investor role, owned by the roles program; must be
    /// compliant, since this opens a sale.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            seller.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = seller_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub seller_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, seller.key().as_ref()],
        bump = seller_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = seller_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub seller_compliance: Box<Account<'info, Compliance>>,

    /// The primary listing; carries the property's lifecycle status.
    #[account(
        seeds = [LISTING_SEED, &asset_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    /// The property; names the region whose fees get snapshotted.
    #[account(
        seeds = [PROPERTY_SEED, &asset_id.to_le_bytes()],
        bump = property.bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    /// The property's region, owned by the regions program. Read for the
    /// fee snapshot, same as the primary listing.
    #[account(
        seeds = [regions::REGION_SEED, &property.region_id.to_le_bytes()],
        bump = region.bump,
        seeds::program = regions::ID,
    )]
    pub region: Box<Account<'info, regions::state::Region>>,

    #[account(
        mut,
        seeds = [SHARE_SEED, &asset_id.to_le_bytes(), seller.key().as_ref()],
        bump = holding.bump,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    #[account(
        init,
        payer = payer,
        space = 8 + ShareListing::INIT_SPACE,
        seeds = [SHARE_LISTING_SEED, &config.next_share_listing_id.to_le_bytes()],
        bump,
    )]
    pub share_listing: Box<Account<'info, ShareListing>>,

    pub system_program: Program<'info, System>,
}

pub fn relist_shares_handler(
    ctx: Context<RelistShares>,
    asset_id: u64,
    amount: u32,
    share_price: u64,
    payment_mint: Pubkey,
) -> Result<()> {
    require!(
        ctx.accounts.listing.status == ListingStatus::Finalized,
        MarketplaceError::PropertyNotFinalized
    );
    // The allowlist was validated as real mints when the config was set.
    require!(
        ctx.accounts
            .config
            .accepted_payment_mints
            .contains(&payment_mint),
        MarketplaceError::MintNotAccepted
    );
    require!(amount > 0, MarketplaceError::InvalidShareAmount);
    require!(
        share_price >= MIN_SHARE_PRICE,
        MarketplaceError::InvalidSharePrice
    );
    // The full lot must stay priceable in u64.
    share_price
        .checked_mul(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;

    let holding = &mut ctx.accounts.holding;
    require!(
        amount <= holding.transferable(),
        MarketplaceError::NotEnoughShares
    );
    holding.listed = holding
        .listed
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;

    let config = &mut ctx.accounts.config;
    let id = config.next_share_listing_id;
    config.next_share_listing_id = id.checked_add(1).ok_or(MarketplaceError::Overflow)?;

    let share_listing = &mut ctx.accounts.share_listing;
    share_listing.id = id;
    share_listing.asset_id = asset_id;
    share_listing.seller = ctx.accounts.seller.key();
    share_listing.share_price = share_price;
    share_listing.payment_mint = payment_mint;
    share_listing.amount = amount;
    share_listing.seller_fee_bps = ctx.accounts.region.seller_fee_bps;
    share_listing.buyer_fee_bps = ctx.accounts.region.buyer_fee_bps;
    share_listing.rent_payer = ctx.accounts.payer.key();
    share_listing.bump = ctx.bumps.share_listing;

    emit!(SharesRelisted {
        id,
        asset_id,
        seller: share_listing.seller,
        payment_mint,
        share_price,
        amount,
    });
    Ok(())
}

/// Take an unsold listing off the market. Seller only, role-free: this is a
/// pure exit.
#[derive(Accounts)]
pub struct DelistShares<'info> {
    pub seller: Signer<'info>,

    /// CHECK: the wallet that fronted the listing's rent; gets it back as
    /// the listing closes.
    #[account(mut, address = share_listing.rent_payer @ MarketplaceError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [SHARE_LISTING_SEED, &share_listing.id.to_le_bytes()],
        bump = share_listing.bump,
        constraint = share_listing.seller == seller.key() @ MarketplaceError::WrongSeller,
    )]
    pub share_listing: Box<Account<'info, ShareListing>>,

    #[account(
        mut,
        seeds = [
            SHARE_SEED,
            &share_listing.asset_id.to_le_bytes(),
            seller.key().as_ref(),
        ],
        bump = holding.bump,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,
}

pub fn delist_shares_handler(ctx: Context<DelistShares>) -> Result<()> {
    let holding = &mut ctx.accounts.holding;
    holding.listed = holding
        .listed
        .checked_sub(ctx.accounts.share_listing.amount)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(SharesDelisted {
        id: ctx.accounts.share_listing.id,
        asset_id: ctx.accounts.share_listing.asset_id,
        amount: ctx.accounts.share_listing.amount,
    });
    Ok(())
}

/// Buy from a secondary listing. Both parties' income settles at their
/// pre-trade balances first, then the buyer pays the price plus the buyer
/// fee: the price minus the seller fee goes to the seller (both rates
/// snapshotted on the listing), the net fees split between the region's
/// operator and the treasury, and the shares move ledger and token side.
/// A partial buy leaves the listing open for the rest.
#[derive(Accounts)]
#[instruction(asset_id: u64, id: u64)]
pub struct BuyRelistedShares<'info> {
    pub buyer: Signer<'info>,

    /// Whoever fronts rent for accounts created along the way: the sponsor
    /// on the default path, or any willing wallet.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The buyer's investor role, owned by the roles program; must be
    /// compliant, since investor money moves.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            buyer.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = buyer_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub buyer_role: Box<Account<'info, RoleAccount>>,

    /// CHECK: the buyer's compliance record; address derived and verdict read
    /// in the handler, to keep `try_accounts` inside the BPF stack frame.
    pub buyer_compliance: UncheckedAccount<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// CHECK: the primary listing (status gate and the ownership-cap
    /// snapshot); the handler deserializes it and matches its stored asset
    /// id, all out of `try_accounts` for stack room.
    pub listing: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [PROPERTY_SEED, &asset_id.to_le_bytes()],
        bump = property.bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    #[account(
        mut,
        seeds = [SHARE_LISTING_SEED, &id.to_le_bytes()],
        bump = share_listing.bump,
        constraint = share_listing.asset_id == asset_id @ MarketplaceError::LedgerMismatch,
    )]
    pub share_listing: Box<Account<'info, ShareListing>>,

    /// CHECK: the seller, from the listing; authority of the payout and
    /// share accounts below.
    #[account(address = share_listing.seller @ MarketplaceError::WrongSeller)]
    pub seller: UncheckedAccount<'info>,

    /// CHECK: the wallet that fronted the listing's rent; gets it back if
    /// this buy empties the listing.
    #[account(mut, address = share_listing.rent_payer @ MarketplaceError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [SHARE_SEED, &asset_id.to_le_bytes(), seller.key().as_ref()],
        bump = seller_holding.bump,
    )]
    pub seller_holding: Box<Account<'info, ShareHolding>>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + ShareHolding::INIT_SPACE,
        seeds = [SHARE_SEED, &asset_id.to_le_bytes(), buyer.key().as_ref()],
        bump,
    )]
    pub buyer_holding: Box<Account<'info, ShareHolding>>,

    /// CHECK: the share listing's settlement mint; the transfers fail on any
    /// account that doesn't match it.
    #[account(address = share_listing.payment_mint @ MarketplaceError::PaymentMintMismatch)]
    pub payment_mint: UncheckedAccount<'info>,

    /// CHECK: the buyer's token account the money leaves; the token program
    /// rules on it during the transfers.
    #[account(mut)]
    pub buyer_payment: UncheckedAccount<'info>,

    /// CHECK: the seller's associated account for the paid mint; created
    /// idempotently, so a closed account can't strand the proceeds.
    #[account(mut)]
    pub seller_payment: UncheckedAccount<'info>,

    /// CHECK: the treasury owner key from config; authority of the ATA
    /// below.
    #[account(address = config.treasury @ MarketplaceError::WrongPayee)]
    pub treasury: UncheckedAccount<'info>,

    /// CHECK: the treasury's associated account for the paid mint; created
    /// idempotently.
    #[account(mut)]
    pub treasury_payment: UncheckedAccount<'info>,

    /// CHECK: the property's region; the handler proves the regions program
    /// owns it, matches its id, and reads the current owner.
    pub region: UncheckedAccount<'info>,

    /// CHECK: the region's current owner; the handler checks it against the
    /// region record. Authority of the ATA below.
    pub region_owner: UncheckedAccount<'info>,

    /// CHECK: the region owner's associated account for the paid mint;
    /// created idempotently.
    #[account(mut)]
    pub operator_payment: UncheckedAccount<'info>,

    /// CHECK: the share mint, pinned to the property's stored mint key (out
    /// of `try_accounts` for stack room).
    #[account(address = property.share_mint @ MarketplaceError::WrongShareMint)]
    pub share_mint: UncheckedAccount<'info>,

    /// CHECK: the share mint's authority PDA; permanent delegate, signs the
    /// transfer and the lock-state changes. Derived in the handler (stack
    /// room).
    pub mint_auth: UncheckedAccount<'info>,

    /// CHECK: the seller's share account; the handler pins it to its
    /// derivation (kept out of `try_accounts` for stack room).
    #[account(mut)]
    pub seller_share_account: UncheckedAccount<'info>,

    /// CHECK: the buyer's associated share account; created idempotently,
    /// so the ATA program verifies the derivation.
    #[account(mut)]
    pub buyer_share_account: UncheckedAccount<'info>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs the
    /// income settlements. Derived in the handler (stack room).
    pub cpi_auth: UncheckedAccount<'info>,

    /// CHECK: the property's income ledger; the handler pins it to its
    /// derivation under the property program, because its existence decides
    /// whether the settlements run at all, so a stand-in account can't be
    /// used to skip them.
    pub income: UncheckedAccount<'info>,

    /// CHECK: the seller's income checkpoint; verified by the property
    /// program.
    #[account(mut)]
    pub seller_checkpoint: UncheckedAccount<'info>,

    /// CHECK: the buyer's income checkpoint; verified by the property
    /// program.
    #[account(mut)]
    pub buyer_checkpoint: UncheckedAccount<'info>,

    /// CHECK: the property program the settlements CPI into, pinned by
    /// address.
    #[account(address = PROPERTY_PROGRAM @ MarketplaceError::WrongProgram)]
    pub property_program: UncheckedAccount<'info>,

    /// The payment mint's token program (classic or Token-2022).
    pub payment_token_program: Interface<'info, TokenInterface>,
    /// The share mint's program is always Token-2022.
    pub share_token_program: Program<'info, Token2022>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn buy_relisted_shares_handler<'info>(
    ctx: Context<'info, BuyRelistedShares<'info>>,
    asset_id: u64,
    id: u64,
    amount: u32,
    max_total_cost: u64,
) -> Result<()> {
    require_compliant(&ctx.accounts.buyer_compliance, &ctx.accounts.buyer.key())?;
    // `try_from` proves this program owns the record; one listing ever
    // exists per asset, so the id match pins it without a derivation.
    let primary: Account<Listing> = Account::try_from(&ctx.accounts.listing)?;
    require!(
        primary.asset_id == asset_id,
        MarketplaceError::LedgerMismatch
    );
    require!(
        primary.status == ListingStatus::Finalized,
        MarketplaceError::PropertyNotFinalized
    );
    require!(amount > 0, MarketplaceError::InvalidShareAmount);
    let share_listing = &ctx.accounts.share_listing;
    require!(
        amount <= share_listing.amount,
        MarketplaceError::NotEnoughSharesListed
    );
    let mint_key = ctx.accounts.payment_mint.key();
    // The derivation pins moved out of `try_accounts` for stack room.
    require!(
        ctx.accounts.seller_share_account.key()
            == get_associated_token_address_with_program_id(
                &ctx.accounts.seller.key(),
                &ctx.accounts.share_mint.key(),
                &ctx.accounts.share_token_program.key(),
            ),
        MarketplaceError::WrongTokenAccount
    );
    let id_bytes = asset_id.to_le_bytes();
    require_pda(
        &ctx.accounts.income.key(),
        &[INCOME_SEED, &id_bytes, &[ctx.accounts.property.income_bump]],
        &PROPERTY_PROGRAM,
        MarketplaceError::WrongIncomeLedger,
    )?;
    let mint_auth_bump = ctx.accounts.property.mint_auth_bump;
    require_pda(
        &ctx.accounts.mint_auth.key(),
        &[MINT_AUTH_SEED, &id_bytes, &[mint_auth_bump]],
        &crate::ID,
        MarketplaceError::WrongMintAuthority,
    )?;
    // The fee split pays the region's current owner, read live like the
    // primary settlement does. `try_from` proves the regions program owns
    // the record; region ids are unique, so the id match pins the account
    // without a derivation.
    let region: Account<regions::state::Region> = Account::try_from(&ctx.accounts.region)?;
    require!(
        region.region_id == ctx.accounts.property.region_id,
        MarketplaceError::WrongRegionAccount
    );
    require!(
        ctx.accounts.region_owner.key() == region.owner,
        MarketplaceError::WrongPayee
    );

    // Ownership cap, against the snapshot taken at listing time; same rule
    // as the primary sale.
    let owned_after = (ctx.accounts.buyer_holding.amount as u64)
        .checked_add(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    primary.require_below_ownership_cap(owned_after, ctx.accounts.property.share_amount)?;

    // Price off the listing snapshots, rescaled to the paid mint. The buyer
    // pays the price plus the buyer fee, the seller fee comes out of the
    // proceeds. The caller caps the full outlay, so nothing can charge more
    // than they signed for.
    let total_quote = share_listing
        .share_price
        .checked_mul(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    let mint_decimals = {
        let data = ctx.accounts.payment_mint.try_borrow_data()?;
        StateWithExtensions::<MintState>::unpack(&data)?
            .base
            .decimals
    };
    let total = scale_to_mint(total_quote, mint_decimals)?;
    let buyer_fee = scale_to_mint(
        bps_of(total_quote, share_listing.buyer_fee_bps)?,
        mint_decimals,
    )?;
    let seller_fee = scale_to_mint(
        bps_of(total_quote, share_listing.seller_fee_bps)?,
        mint_decimals,
    )?;
    let buyer_cost = total
        .checked_add(buyer_fee)
        .ok_or(MarketplaceError::Overflow)?;
    require!(buyer_cost <= max_total_cost, MarketplaceError::CostTooHigh);
    let seller_part = total
        .checked_sub(seller_fee)
        .ok_or(MarketplaceError::Overflow)?;
    let fees = buyer_fee
        .checked_add(seller_fee)
        .ok_or(MarketplaceError::Overflow)?;

    let seller_key = ctx.accounts.seller.key();
    let buyer_key = ctx.accounts.buyer.key();

    // A first-time buyer's holding was created just now, so its zero-share
    // state must be flushed for the settlement CPI to read; Anchor would
    // otherwise only write it at exit.
    let buyer_holding = &mut ctx.accounts.buyer_holding;
    if buyer_holding.owner == Pubkey::default() {
        buyer_holding.asset_id = asset_id;
        buyer_holding.owner = buyer_key;
        buyer_holding.locks = [0; LOCK_REASONS];
        buyer_holding.listed = 0;
        buyer_holding.bump = ctx.bumps.buyer_holding;
        ctx.accounts.property.holder_count = ctx
            .accounts
            .property
            .holder_count
            .checked_add(1)
            .ok_or(MarketplaceError::Overflow)?;
        ctx.accounts.buyer_holding.exit(&crate::ID)?;
    }

    // Both parties settle their accrued income at pre-trade balances, so
    // the trade can neither capture nor strand anyone's rent. A property
    // that never distributed income has no ledger yet and nothing to
    // settle; once one exists, the address pin above makes these calls
    // unavoidable. The signer PDA is pinned here rather than in
    // `try_accounts` (stack room), and only the real one can sign the CPI.
    if !ctx.accounts.income.data_is_empty() {
        let cpi_auth_bump = ctx.accounts.config.cpi_auth_bump;
        require_pda(
            &ctx.accounts.cpi_auth.key(),
            &[CPI_AUTH_SEED, &[cpi_auth_bump]],
            &crate::ID,
            MarketplaceError::WrongCpiSigner,
        )?;
        settle_income(
            &ctx.accounts.property_program.to_account_info(),
            &ctx.accounts.cpi_auth.to_account_info(),
            &ctx.accounts.payer.to_account_info(),
            &ctx.accounts.income.to_account_info(),
            &ctx.accounts.seller_holding.to_account_info(),
            &ctx.accounts.seller_checkpoint.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            cpi_auth_bump,
            asset_id,
            seller_key,
        )?;
        settle_income(
            &ctx.accounts.property_program.to_account_info(),
            &ctx.accounts.cpi_auth.to_account_info(),
            &ctx.accounts.payer.to_account_info(),
            &ctx.accounts.income.to_account_info(),
            &ctx.accounts.buyer_holding.to_account_info(),
            &ctx.accounts.buyer_checkpoint.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            cpi_auth_bump,
            asset_id,
            buyer_key,
        )?;
    }

    // The net fees split between the region's operator and the treasury at
    // the config's live share, mirroring the primary settlement; rounding
    // dust stays with the treasury.
    let operator_fee =
        u64::try_from(fees as u128 * ctx.accounts.config.operator_fee_share_bps as u128 / 10_000)
            .map_err(|_| MarketplaceError::Overflow)?;
    let treasury_fee = fees
        .checked_sub(operator_fee)
        .ok_or(MarketplaceError::Overflow)?;

    // Every payee is paid at their ATA, created if needed, so a closed
    // account can't block the sale.
    let payouts = [
        (
            ctx.accounts.seller.to_account_info(),
            ctx.accounts.seller_payment.to_account_info(),
            seller_part,
        ),
        (
            ctx.accounts.region_owner.to_account_info(),
            ctx.accounts.operator_payment.to_account_info(),
            operator_fee,
        ),
        (
            ctx.accounts.treasury.to_account_info(),
            ctx.accounts.treasury_payment.to_account_info(),
            treasury_fee,
        ),
    ];
    for (authority, payment_account, payout) in payouts {
        if payout == 0 {
            continue;
        }
        create_idempotent(CpiContext::new(
            ctx.accounts.associated_token_program.key(),
            CreateAta {
                payer: ctx.accounts.payer.to_account_info(),
                associated_token: payment_account.clone(),
                authority,
                mint: ctx.accounts.payment_mint.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
                token_program: ctx.accounts.payment_token_program.to_account_info(),
            },
        ))?;
        transfer_checked(
            CpiContext::new(
                ctx.accounts.payment_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.buyer_payment.to_account_info(),
                    mint: ctx.accounts.payment_mint.to_account_info(),
                    to: payment_account,
                    authority: ctx.accounts.buyer.to_account_info(),
                },
            ),
            payout,
            mint_decimals,
        )?;
    }

    // Shares through the usual airlock, this time seller to buyer with the
    // permanent delegate signing.
    move_shares(
        &ctx.accounts.share_token_program.to_account_info(),
        &ctx.accounts.associated_token_program.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.share_mint.to_account_info(),
        &ctx.accounts.mint_auth.to_account_info(),
        mint_auth_bump,
        asset_id,
        &ctx.accounts.seller_share_account.to_account_info(),
        &ctx.accounts.buyer_share_account.to_account_info(),
        &ctx.accounts.buyer.to_account_info(),
        amount,
    )?;

    // Ledger: the sold shares leave the seller's balance and their listing
    // reserve together, keeping the lock invariant intact.
    let seller_holding = &mut ctx.accounts.seller_holding;
    seller_holding.amount = seller_holding
        .amount
        .checked_sub(amount)
        .ok_or(MarketplaceError::Overflow)?;
    seller_holding.listed = seller_holding
        .listed
        .checked_sub(amount)
        .ok_or(MarketplaceError::Overflow)?;

    let buyer_holding = &mut ctx.accounts.buyer_holding;
    buyer_holding.amount = buyer_holding
        .amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;

    let share_listing = &mut ctx.accounts.share_listing;
    share_listing.amount = share_listing
        .amount
        .checked_sub(amount)
        .ok_or(MarketplaceError::Overflow)?;
    let remaining = share_listing.amount;
    if remaining == 0 {
        share_listing.close(ctx.accounts.rent_payer.to_account_info())?;
    }

    emit!(RelistedSharesBought {
        id,
        asset_id,
        buyer: buyer_key,
        seller: seller_key,
        amount,
        mint: mint_key,
        paid: buyer_cost,
        fees,
        operator_fee,
        remaining,
    });
    Ok(())
}

/// Give shares away: a plain transfer between compliant investors, no
/// money involved. Both parties' income settles at pre-transfer balances
/// and the receiver is held to the ownership cap, exactly like a sale.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct SendShares<'info> {
    pub sender: Signer<'info>,

    /// Whoever fronts rent for accounts created along the way: the sponsor
    /// on the default path, or any willing wallet.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The sender's investor role, owned by the roles program; must be
    /// compliant.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            sender.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = sender_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub sender_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, sender.key().as_ref()],
        bump = sender_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = sender_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub sender_compliance: Box<Account<'info, Compliance>>,

    /// CHECK: the receiving wallet; its role account below proves standing.
    pub receiver: UncheckedAccount<'info>,

    /// The receiver's investor role; shares only ever land with compliant
    /// investors.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            receiver.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = receiver_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub receiver_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, receiver.key().as_ref()],
        bump = receiver_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = receiver_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub receiver_compliance: Box<Account<'info, Compliance>>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// CHECK: the primary listing (status gate and the ownership-cap
    /// snapshot), seeds-pinned here and deserialized in the handler.
    #[account(seeds = [LISTING_SEED, &asset_id.to_le_bytes()], bump)]
    pub listing: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [PROPERTY_SEED, &asset_id.to_le_bytes()],
        bump = property.bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    #[account(
        mut,
        seeds = [SHARE_SEED, &asset_id.to_le_bytes(), sender.key().as_ref()],
        bump = sender_holding.bump,
    )]
    pub sender_holding: Box<Account<'info, ShareHolding>>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + ShareHolding::INIT_SPACE,
        seeds = [SHARE_SEED, &asset_id.to_le_bytes(), receiver.key().as_ref()],
        bump,
    )]
    pub receiver_holding: Box<Account<'info, ShareHolding>>,

    /// CHECK: the share mint, pinned to the key the property recorded.
    #[account(address = property.share_mint @ MarketplaceError::WrongShareMint)]
    pub share_mint: UncheckedAccount<'info>,

    /// CHECK: the share mint's authority PDA; permanent delegate.
    #[account(
        seeds = [MINT_AUTH_SEED, &asset_id.to_le_bytes()],
        bump = property.mint_auth_bump,
    )]
    pub mint_auth: UncheckedAccount<'info>,

    /// CHECK: the sender's share account; the handler pins it to its
    /// derivation.
    #[account(mut)]
    pub sender_share_account: UncheckedAccount<'info>,

    /// CHECK: the receiver's associated share account; created
    /// idempotently, so the ATA program verifies the derivation.
    #[account(mut)]
    pub receiver_share_account: UncheckedAccount<'info>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs the
    /// income settlements.
    #[account(seeds = [CPI_AUTH_SEED], bump = config.cpi_auth_bump)]
    pub cpi_auth: UncheckedAccount<'info>,

    /// CHECK: the property's income ledger; the handler pins it to its
    /// derivation under the property program.
    pub income: UncheckedAccount<'info>,

    /// CHECK: the sender's income checkpoint; verified by the property
    /// program.
    #[account(mut)]
    pub sender_checkpoint: UncheckedAccount<'info>,

    /// CHECK: the receiver's income checkpoint; verified by the property
    /// program.
    #[account(mut)]
    pub receiver_checkpoint: UncheckedAccount<'info>,

    /// CHECK: the property program the settlements CPI into, pinned by
    /// address.
    #[account(address = PROPERTY_PROGRAM @ MarketplaceError::WrongProgram)]
    pub property_program: UncheckedAccount<'info>,

    pub share_token_program: Program<'info, Token2022>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn send_property_shares_handler<'info>(
    ctx: Context<'info, SendShares<'info>>,
    asset_id: u64,
    amount: u32,
) -> Result<()> {
    // Transfers wait for the settlement: the refund paths before it
    // reconcile positions against holdings, and a transfer would wedge
    // them.
    let primary: Account<Listing> = Account::try_from(&ctx.accounts.listing)?;
    require!(
        primary.status == ListingStatus::Finalized,
        MarketplaceError::PropertyNotFinalized
    );
    require!(amount > 0, MarketplaceError::InvalidShareAmount);
    require!(
        amount <= ctx.accounts.sender_holding.transferable(),
        MarketplaceError::NotEnoughShares
    );
    require!(
        ctx.accounts.sender_share_account.key()
            == get_associated_token_address_with_program_id(
                &ctx.accounts.sender.key(),
                &ctx.accounts.share_mint.key(),
                &ctx.accounts.share_token_program.key(),
            ),
        MarketplaceError::WrongTokenAccount
    );
    require_pda(
        &ctx.accounts.income.key(),
        &[
            INCOME_SEED,
            &asset_id.to_le_bytes(),
            &[ctx.accounts.property.income_bump],
        ],
        &PROPERTY_PROGRAM,
        MarketplaceError::WrongIncomeLedger,
    )?;

    let owned_after = (ctx.accounts.receiver_holding.amount as u64)
        .checked_add(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    primary.require_below_ownership_cap(owned_after, ctx.accounts.property.share_amount)?;

    let sender_key = ctx.accounts.sender.key();
    let receiver_key = ctx.accounts.receiver.key();

    // A first-time receiver's holding was created just now, so its
    // zero-share state must be flushed for the settlement CPI to read.
    let receiver_holding = &mut ctx.accounts.receiver_holding;
    if receiver_holding.owner == Pubkey::default() {
        receiver_holding.asset_id = asset_id;
        receiver_holding.owner = receiver_key;
        receiver_holding.locks = [0; LOCK_REASONS];
        receiver_holding.listed = 0;
        receiver_holding.bump = ctx.bumps.receiver_holding;
        ctx.accounts.property.holder_count = ctx
            .accounts
            .property
            .holder_count
            .checked_add(1)
            .ok_or(MarketplaceError::Overflow)?;
        ctx.accounts.receiver_holding.exit(&crate::ID)?;
    }

    // Both parties settle their accrued income at pre-transfer balances;
    // see `buy_relisted_shares` for the skip rule.
    if !ctx.accounts.income.data_is_empty() {
        settle_income(
            &ctx.accounts.property_program.to_account_info(),
            &ctx.accounts.cpi_auth.to_account_info(),
            &ctx.accounts.payer.to_account_info(),
            &ctx.accounts.income.to_account_info(),
            &ctx.accounts.sender_holding.to_account_info(),
            &ctx.accounts.sender_checkpoint.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            ctx.accounts.config.cpi_auth_bump,
            asset_id,
            sender_key,
        )?;
        settle_income(
            &ctx.accounts.property_program.to_account_info(),
            &ctx.accounts.cpi_auth.to_account_info(),
            &ctx.accounts.payer.to_account_info(),
            &ctx.accounts.income.to_account_info(),
            &ctx.accounts.receiver_holding.to_account_info(),
            &ctx.accounts.receiver_checkpoint.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            ctx.accounts.config.cpi_auth_bump,
            asset_id,
            receiver_key,
        )?;
    }

    move_shares(
        &ctx.accounts.share_token_program.to_account_info(),
        &ctx.accounts.associated_token_program.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.share_mint.to_account_info(),
        &ctx.accounts.mint_auth.to_account_info(),
        ctx.accounts.property.mint_auth_bump,
        asset_id,
        &ctx.accounts.sender_share_account.to_account_info(),
        &ctx.accounts.receiver_share_account.to_account_info(),
        &ctx.accounts.receiver.to_account_info(),
        amount,
    )?;

    let sender_holding = &mut ctx.accounts.sender_holding;
    sender_holding.amount = sender_holding
        .amount
        .checked_sub(amount)
        .ok_or(MarketplaceError::Overflow)?;
    let receiver_holding = &mut ctx.accounts.receiver_holding;
    receiver_holding.amount = receiver_holding
        .amount
        .checked_add(amount)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(PropertySharesSent {
        asset_id,
        sender: sender_key,
        receiver: receiver_key,
        amount,
    });
    Ok(())
}

/// Reclaim an emptied holding's rent and take it out of the holder count.
/// Permissionless: once a seller has sold their last share, anyone may
/// sweep the account, and the rent goes back to the sponsor that fronted
/// investor accounts.
#[derive(Accounts)]
pub struct CloseShareHolding<'info> {
    pub cranker: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// CHECK: the sponsor wallet holdings' rent returns to, from config.
    #[account(mut, address = config.rent_sponsor @ MarketplaceError::WrongPayee)]
    pub rent_sponsor: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [PROPERTY_SEED, &holding.asset_id.to_le_bytes()],
        bump = property.bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    #[account(
        mut,
        close = rent_sponsor,
        seeds = [
            SHARE_SEED,
            &holding.asset_id.to_le_bytes(),
            holding.owner.as_ref(),
        ],
        bump = holding.bump,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,
}

pub fn close_share_holding_handler(ctx: Context<CloseShareHolding>) -> Result<()> {
    let holding = &ctx.accounts.holding;
    require!(
        holding.amount == 0 && holding.locked() == 0 && holding.listed == 0,
        MarketplaceError::HoldingNotEmpty
    );
    ctx.accounts.property.holder_count = ctx
        .accounts
        .property
        .holder_count
        .checked_sub(1)
        .ok_or(MarketplaceError::Overflow)?;

    emit!(ShareHoldingClosed {
        asset_id: holding.asset_id,
        owner: holding.owner,
    });
    Ok(())
}

#[event]
pub struct SharesRelisted {
    pub id: u64,
    pub asset_id: u64,
    pub seller: Pubkey,
    pub payment_mint: Pubkey,
    pub share_price: u64,
    pub amount: u32,
}

#[event]
pub struct SharesDelisted {
    pub id: u64,
    pub asset_id: u64,
    pub amount: u32,
}

#[event]
pub struct RelistedSharesBought {
    pub id: u64,
    pub asset_id: u64,
    pub buyer: Pubkey,
    pub seller: Pubkey,
    pub amount: u32,
    pub mint: Pubkey,
    /// The buyer's full outlay in the mint's units: price plus buyer fee.
    pub paid: u64,
    /// Combined seller and buyer fees; the operator's slice of them is
    /// broken out, the treasury took the rest.
    pub fees: u64,
    pub operator_fee: u64,
    pub remaining: u32,
}

#[event]
pub struct PropertySharesSent {
    pub asset_id: u64,
    pub sender: Pubkey,
    pub receiver: Pubkey,
    pub amount: u32,
}

#[event]
pub struct ShareHoldingClosed {
    pub asset_id: u64,
    pub owner: Pubkey,
}
