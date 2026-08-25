use anchor_lang::prelude::*;
use anchor_lang::AccountsExit;
use anchor_spl::associated_token::{
    create_idempotent, get_associated_token_address_with_program_id, AssociatedToken,
    Create as CreateAta,
};
use anchor_spl::token_2022::spl_token_2022::{
    extension::StateWithExtensions,
    state::{Account as TokenAccountState, Mint as MintState},
};
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{
    close_account, transfer_checked, CloseAccount, TokenInterface, TransferChecked,
};

use crate::constants::{
    CONFIG_SEED, CPI_AUTH_SEED, INCOME_SEED, LISTING_SEED, MINT_AUTH_SEED, OFFER_SEED,
    OFFER_VAULT_SEED, PROPERTY_PROGRAM, PROPERTY_SEED, SHARE_LISTING_SEED, SHARE_MINT_SEED,
    SHARE_SEED,
};
use crate::error::MarketplaceError;
use crate::instructions::buy::{bps_of, scale_to_mint};
use crate::instructions::secondary::{move_shares, settle_income};
use crate::state::{
    Config, Listing, ListingStatus, Offer, PropertyAsset, ShareHolding, ShareListing, LOCK_REASONS,
    MIN_PAYMENT_DECIMALS, PRICE_DECIMALS,
};

use crate::compliance_guard::require_compliant;
use xcavate_whitelist::state::{Compliance, Role, RoleAccount};

/// Empty the offer's vault, then close its token account back to whoever
/// fronted the offer's rent. Pays the fixed amounts first and sweeps
/// whatever the vault actually holds to `rest_to`: anyone can donate
/// tokens into any ATA, and a close on a non-empty account fails, so
/// paying only the recorded amount would let dust wedge the offer shut.
/// Returns what `rest_to` received.
#[allow(clippy::too_many_arguments)]
fn drain_offer_vault<'info>(
    token_program: &AccountInfo<'info>,
    payment_mint: &AccountInfo<'info>,
    vault: &AccountInfo<'info>,
    vault_account: &AccountInfo<'info>,
    rent_payer: &AccountInfo<'info>,
    vault_seeds: &[&[u8]],
    fixed: &[(&AccountInfo<'info>, u64)],
    rest_to: &AccountInfo<'info>,
    mint_decimals: u8,
) -> Result<u64> {
    let mut rest = {
        let data = vault_account.try_borrow_data()?;
        StateWithExtensions::<TokenAccountState>::unpack(&data)?
            .base
            .amount
    };
    let pay = |destination: &AccountInfo<'info>, amount: u64| {
        transfer_checked(
            CpiContext::new_with_signer(
                token_program.key(),
                TransferChecked {
                    from: vault_account.clone(),
                    mint: payment_mint.clone(),
                    to: destination.clone(),
                    authority: vault.clone(),
                },
                &[vault_seeds],
            ),
            amount,
            mint_decimals,
        )
    };
    for (destination, amount) in fixed {
        if *amount == 0 {
            continue;
        }
        rest = rest
            .checked_sub(*amount)
            .ok_or(MarketplaceError::Overflow)?;
        pay(destination, *amount)?;
    }
    if rest > 0 {
        pay(rest_to, rest)?;
    }
    close_account(CpiContext::new_with_signer(
        token_program.key(),
        CloseAccount {
            account: vault_account.clone(),
            destination: rent_payer.clone(),
            authority: vault.clone(),
        },
        &[vault_seeds],
    ))?;
    Ok(rest)
}

/// Bid on a share listing, at any price the bidder likes. The full bid
/// moves into the offer's own vault now, so an accepted offer can always
/// pay. One open offer per bidder per listing; the listing stamps a nonce
/// so the seller later accepts exactly the offer they saw.
#[derive(Accounts)]
#[instruction(id: u64)]
pub struct MakeOffer<'info> {
    pub offeror: Signer<'info>,

    /// Whoever fronts the rent for the offer and its vault account: the
    /// bidder on the default path. The offer remembers who to refund.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The bidder's investor role, owned by the roles program; must be
    /// compliant, since investor money moves.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            offeror.key().as_ref(),
            &[Role::RealEstateInvestor.seed_byte()],
        ],
        bump = offeror_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub offeror_role: Box<Account<'info, RoleAccount>>,

    #[account(
        seeds = [xcavate_whitelist::COMPLIANCE_SEED, offeror.key().as_ref()],
        bump = offeror_compliance.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = offeror_compliance.is_live()? @ MarketplaceError::NotCompliant,
    )]
    pub offeror_compliance: Box<Account<'info, Compliance>>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [SHARE_LISTING_SEED, &id.to_le_bytes()],
        bump = share_listing.bump,
    )]
    pub share_listing: Box<Account<'info, ShareListing>>,

    #[account(
        init,
        payer = payer,
        space = 8 + Offer::INIT_SPACE,
        seeds = [OFFER_SEED, &id.to_le_bytes(), offeror.key().as_ref()],
        bump,
    )]
    pub offer: Box<Account<'info, Offer>>,

    /// CHECK: the offer's vault authority; a bare PDA owning the vault's
    /// token account.
    #[account(seeds = [OFFER_VAULT_SEED, &id.to_le_bytes(), offeror.key().as_ref()], bump)]
    pub offer_vault: UncheckedAccount<'info>,

    /// CHECK: the mint the bid is made in; must be on the accepted list.
    pub payment_mint: UncheckedAccount<'info>,

    /// CHECK: the bidder's token account the bid leaves; the token program
    /// rules on it during the transfer.
    #[account(mut)]
    pub offeror_payment: UncheckedAccount<'info>,

    /// CHECK: the vault's associated account for the bid mint; created
    /// here, so the ATA program verifies the derivation.
    #[account(mut)]
    pub vault_payment_account: UncheckedAccount<'info>,

    pub payment_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn make_offer_handler(
    ctx: Context<MakeOffer>,
    id: u64,
    amount: u32,
    share_price: u64,
) -> Result<()> {
    require!(amount > 0, MarketplaceError::InvalidShareAmount);
    // Same price floor as listings, so no bid can rescale to zero.
    require!(
        share_price >= 10u64.pow((PRICE_DECIMALS - MIN_PAYMENT_DECIMALS) as u32),
        MarketplaceError::InvalidSharePrice
    );
    let share_listing = &mut ctx.accounts.share_listing;
    require!(
        ctx.accounts.offeror.key() != share_listing.seller,
        MarketplaceError::SelfOffer
    );
    require!(
        amount <= share_listing.amount,
        MarketplaceError::NotEnoughSharesListed
    );
    let mint_key = ctx.accounts.payment_mint.key();
    require!(
        ctx.accounts
            .config
            .accepted_payment_mints
            .contains(&mint_key),
        MarketplaceError::MintNotAccepted
    );

    let total_quote = share_price
        .checked_mul(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    let mint_decimals = {
        let data = ctx.accounts.payment_mint.try_borrow_data()?;
        StateWithExtensions::<MintState>::unpack(&data)?
            .base
            .decimals
    };
    let held = scale_to_mint(total_quote, mint_decimals)?;

    let nonce = share_listing.next_offer_nonce;
    share_listing.next_offer_nonce = nonce.checked_add(1).ok_or(MarketplaceError::Overflow)?;

    create_idempotent(CpiContext::new(
        ctx.accounts.associated_token_program.key(),
        CreateAta {
            payer: ctx.accounts.payer.to_account_info(),
            associated_token: ctx.accounts.vault_payment_account.to_account_info(),
            authority: ctx.accounts.offer_vault.to_account_info(),
            mint: ctx.accounts.payment_mint.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            token_program: ctx.accounts.payment_token_program.to_account_info(),
        },
    ))?;
    transfer_checked(
        CpiContext::new(
            ctx.accounts.payment_token_program.key(),
            TransferChecked {
                from: ctx.accounts.offeror_payment.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                to: ctx.accounts.vault_payment_account.to_account_info(),
                authority: ctx.accounts.offeror.to_account_info(),
            },
        ),
        held,
        mint_decimals,
    )?;

    let offer = &mut ctx.accounts.offer;
    offer.listing_id = id;
    offer.asset_id = share_listing.asset_id;
    offer.offeror = ctx.accounts.offeror.key();
    offer.share_price = share_price;
    offer.amount = amount;
    offer.payment_mint = mint_key;
    offer.held = held;
    offer.nonce = nonce;
    offer.rent_payer = ctx.accounts.payer.key();
    offer.bump = ctx.bumps.offer;

    emit!(OfferMade {
        listing_id: id,
        asset_id: offer.asset_id,
        offeror: offer.offeror,
        share_price,
        amount,
        mint: mint_key,
        held,
        nonce,
    });
    Ok(())
}

/// The seller takes an offer, named by nonce so a swapped bid can't ride
/// their signature. Settles exactly like a buy at the offered price: income
/// checkpointed both sides, fee to the treasury, remainder to the seller,
/// all paid from the offer's vault, and the shares move over the airlock.
#[derive(Accounts)]
#[instruction(id: u64)]
pub struct AcceptOffer<'info> {
    pub seller: Signer<'info>,

    /// Whoever fronts rent for accounts created along the way: the sponsor
    /// on the default path, or any willing wallet.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: the seller's investor role; address derived and existence
    /// proved in the handler, like the bidder's.
    pub seller_role: UncheckedAccount<'info>,

    /// CHECK: both compliance records; addresses derived and verdicts read in
    /// the handler, to keep `try_accounts` inside the BPF stack frame.
    pub seller_compliance: UncheckedAccount<'info>,

    /// CHECK: see `seller_compliance`.
    pub offeror_compliance: UncheckedAccount<'info>,

    /// CHECK: the config, seeds-pinned here and deserialized in the handler
    /// to keep its bulk off the `try_accounts` stack; only the treasury key
    /// is read.
    #[account(seeds = [CONFIG_SEED], bump)]
    pub config: UncheckedAccount<'info>,

    /// CHECK: the primary listing (the ownership-cap snapshot), seeds-pinned
    /// here and deserialized in the handler to keep its bulk off the
    /// `try_accounts` stack.
    #[account(seeds = [LISTING_SEED, &share_listing.asset_id.to_le_bytes()], bump)]
    pub listing: UncheckedAccount<'info>,

    /// CHECK: the property, seeds-pinned here and deserialized in the
    /// handler, which writes it back itself when the holder count moves.
    #[account(mut, seeds = [PROPERTY_SEED, &share_listing.asset_id.to_le_bytes()], bump)]
    pub property: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [SHARE_LISTING_SEED, &id.to_le_bytes()],
        bump = share_listing.bump,
        constraint = share_listing.seller == seller.key() @ MarketplaceError::WrongSeller,
    )]
    pub share_listing: Box<Account<'info, ShareListing>>,

    /// CHECK: the wallet that fronted the listing's rent; gets it back if
    /// this acceptance empties the listing.
    #[account(mut, address = share_listing.rent_payer @ MarketplaceError::WrongRentPayer)]
    pub listing_rent_payer: UncheckedAccount<'info>,

    /// CHECK: the bidder; bound to the offer by its seeds.
    pub offeror: UncheckedAccount<'info>,

    /// CHECK: the bidder's investor role; derived and checked in the
    /// handler (stack room), and compliance must still hold at delivery.
    pub offeror_role: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [OFFER_SEED, &id.to_le_bytes(), offeror.key().as_ref()],
        bump = offer.bump,
    )]
    pub offer: Box<Account<'info, Offer>>,

    /// CHECK: the wallet that fronted the offer's rent; gets the offer and
    /// its vault account back as they close.
    #[account(mut, address = offer.rent_payer @ MarketplaceError::WrongRentPayer)]
    pub offer_rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [SHARE_SEED, &share_listing.asset_id.to_le_bytes(), seller.key().as_ref()],
        bump = seller_holding.bump,
    )]
    pub seller_holding: Box<Account<'info, ShareHolding>>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + ShareHolding::INIT_SPACE,
        seeds = [SHARE_SEED, &share_listing.asset_id.to_le_bytes(), offeror.key().as_ref()],
        bump,
    )]
    pub offeror_holding: Box<Account<'info, ShareHolding>>,

    /// CHECK: the mint the offer was made in, pinned by address.
    #[account(address = offer.payment_mint @ MarketplaceError::PaymentMintMismatch)]
    pub payment_mint: UncheckedAccount<'info>,

    /// CHECK: the offer's vault authority; a bare PDA owning the vault's
    /// token account.
    #[account(seeds = [OFFER_VAULT_SEED, &id.to_le_bytes(), offeror.key().as_ref()], bump)]
    pub offer_vault: UncheckedAccount<'info>,

    /// CHECK: the vault's token account the payout leaves; the handler pins
    /// it to its derivation.
    #[account(mut)]
    pub vault_payment_account: UncheckedAccount<'info>,

    /// CHECK: the seller's associated account for the paid mint; created
    /// idempotently, so a closed account can't strand the proceeds.
    #[account(mut)]
    pub seller_payment: UncheckedAccount<'info>,

    /// CHECK: the treasury owner key; the handler checks it against config.
    pub treasury: UncheckedAccount<'info>,

    /// CHECK: the treasury's associated account for the paid mint; created
    /// idempotently.
    #[account(mut)]
    pub treasury_payment: UncheckedAccount<'info>,

    /// CHECK: the share mint PDA (owned by the Token-2022 program).
    #[account(seeds = [SHARE_MINT_SEED, &share_listing.asset_id.to_le_bytes()], bump)]
    pub share_mint: UncheckedAccount<'info>,

    /// CHECK: the share mint's authority PDA; permanent delegate, signs the
    /// transfer and the lock-state changes.
    #[account(seeds = [MINT_AUTH_SEED, &share_listing.asset_id.to_le_bytes()], bump)]
    pub mint_auth: UncheckedAccount<'info>,

    /// CHECK: the seller's share account; the handler pins it to its
    /// derivation.
    #[account(mut)]
    pub seller_share_account: UncheckedAccount<'info>,

    /// CHECK: the bidder's associated share account; created idempotently,
    /// so the ATA program verifies the derivation.
    #[account(mut)]
    pub offeror_share_account: UncheckedAccount<'info>,

    /// CHECK: this program's CPI signer PDA; holds no data, only signs the
    /// income settlements.
    #[account(seeds = [CPI_AUTH_SEED], bump)]
    pub cpi_auth: UncheckedAccount<'info>,

    /// CHECK: the property's income ledger; the handler pins it to its
    /// derivation under the property program.
    pub income: UncheckedAccount<'info>,

    /// CHECK: the seller's income checkpoint; verified by the property
    /// program.
    #[account(mut)]
    pub seller_checkpoint: UncheckedAccount<'info>,

    /// CHECK: the bidder's income checkpoint; verified by the property
    /// program.
    #[account(mut)]
    pub offeror_checkpoint: UncheckedAccount<'info>,

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

pub fn accept_offer_handler<'info>(
    ctx: Context<'info, AcceptOffer<'info>>,
    id: u64,
    nonce: u64,
) -> Result<()> {
    let offer = &ctx.accounts.offer;
    require!(offer.nonce == nonce, MarketplaceError::OfferNonceMismatch);
    let amount = offer.amount;
    // The listing may have shrunk under the offer through direct buys.
    require!(
        amount <= ctx.accounts.share_listing.amount,
        MarketplaceError::NotEnoughSharesListed
    );
    let asset_id = ctx.accounts.share_listing.asset_id;
    require!(
        ctx.accounts.vault_payment_account.key()
            == get_associated_token_address_with_program_id(
                &ctx.accounts.offer_vault.key(),
                &ctx.accounts.payment_mint.key(),
                &ctx.accounts.payment_token_program.key(),
            ),
        MarketplaceError::WrongVaultAccount
    );
    require!(
        ctx.accounts.seller_share_account.key()
            == get_associated_token_address_with_program_id(
                &ctx.accounts.seller.key(),
                &ctx.accounts.share_mint.key(),
                &ctx.accounts.share_token_program.key(),
            ),
        MarketplaceError::WrongVaultAccount
    );
    require!(
        ctx.accounts.income.key()
            == Pubkey::find_program_address(
                &[INCOME_SEED, &asset_id.to_le_bytes()],
                &PROPERTY_PROGRAM
            )
            .0,
        MarketplaceError::WrongVaultAccount
    );

    // Everything here is unchecked so `try_accounts` fits the BPF stack, so
    // both roles are derived and then deserialized: the address proves whose
    // role it is, deserializing proves the assignment exists.
    for (record, wallet) in [
        (&ctx.accounts.seller_role, ctx.accounts.seller.key()),
        (&ctx.accounts.offeror_role, ctx.accounts.offeror.key()),
    ] {
        let (expected, _) = Pubkey::find_program_address(
            &[
                xcavate_whitelist::ROLE_SEED,
                wallet.as_ref(),
                &[Role::RealEstateInvestor.seed_byte()],
            ],
            &xcavate_whitelist::ID,
        );
        require_keys_eq!(
            record.key(),
            expected,
            MarketplaceError::WrongRegistryAccount
        );
        Account::<RoleAccount>::try_from(record)?;
    }
    require_compliant(&ctx.accounts.seller_compliance, &ctx.accounts.seller.key())?;
    require_compliant(
        &ctx.accounts.offeror_compliance,
        &ctx.accounts.offeror.key(),
    )?;
    let config: Account<Config> = Account::try_from(&ctx.accounts.config)?;
    require!(
        ctx.accounts.treasury.key() == config.treasury,
        MarketplaceError::WrongPayee
    );

    // Ownership cap and status, against the primary listing. Finalized is
    // terminal today; the gate keeps that assumption local.
    let primary: Account<Listing> = Account::try_from(&ctx.accounts.listing)?;
    require!(
        primary.status == ListingStatus::Finalized,
        MarketplaceError::PropertyNotFinalized
    );
    let mut property: Account<PropertyAsset> = Account::try_from(&ctx.accounts.property)?;
    let owned_after = (ctx.accounts.offeror_holding.amount as u64)
        .checked_add(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    let max_shares = (primary.max_ownership_bps as u64)
        .checked_mul(property.share_amount as u64)
        .ok_or(MarketplaceError::Overflow)?
        / 10_000;
    require!(
        owned_after < max_shares,
        MarketplaceError::MaxOwnershipExceeded
    );

    // The fee comes out of the offered total, at the rate snapshotted on
    // the listing.
    let total_quote = offer
        .share_price
        .checked_mul(amount as u64)
        .ok_or(MarketplaceError::Overflow)?;
    let mint_decimals = {
        let data = ctx.accounts.payment_mint.try_borrow_data()?;
        StateWithExtensions::<MintState>::unpack(&data)?
            .base
            .decimals
    };
    let fee = scale_to_mint(
        bps_of(total_quote, ctx.accounts.share_listing.fee_bps)?,
        mint_decimals,
    )?;

    let seller_key = ctx.accounts.seller.key();
    let offeror_key = ctx.accounts.offeror.key();

    // A first-time holder's account was created just now, so its zero-share
    // state must be flushed for the settlement CPI to read.
    let offeror_holding = &mut ctx.accounts.offeror_holding;
    if offeror_holding.owner == Pubkey::default() {
        offeror_holding.asset_id = asset_id;
        offeror_holding.owner = offeror_key;
        offeror_holding.locks = [0; LOCK_REASONS];
        offeror_holding.listed = 0;
        offeror_holding.bump = ctx.bumps.offeror_holding;
        property.holder_count = property
            .holder_count
            .checked_add(1)
            .ok_or(MarketplaceError::Overflow)?;
        // Written back by hand: a `try_from` account isn't in Anchor's exit
        // list.
        property.exit(&crate::ID)?;
        ctx.accounts.offeror_holding.exit(&crate::ID)?;
    }

    // Both parties settle their accrued income at pre-trade balances; see
    // `buy_relisted_shares` for the skip rule.
    if !ctx.accounts.income.data_is_empty() {
        settle_income(
            &ctx.accounts.property_program.to_account_info(),
            &ctx.accounts.cpi_auth.to_account_info(),
            &ctx.accounts.payer.to_account_info(),
            &ctx.accounts.income.to_account_info(),
            &ctx.accounts.seller_holding.to_account_info(),
            &ctx.accounts.seller_checkpoint.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            ctx.bumps.cpi_auth,
            asset_id,
            seller_key,
        )?;
        settle_income(
            &ctx.accounts.property_program.to_account_info(),
            &ctx.accounts.cpi_auth.to_account_info(),
            &ctx.accounts.payer.to_account_info(),
            &ctx.accounts.income.to_account_info(),
            &ctx.accounts.offeror_holding.to_account_info(),
            &ctx.accounts.offeror_checkpoint.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            ctx.bumps.cpi_auth,
            asset_id,
            offeror_key,
        )?;
    }

    // The seller and the treasury are paid from the offer vault, which then
    // closes; its rent rides back with the offer's.
    create_idempotent(CpiContext::new(
        ctx.accounts.associated_token_program.key(),
        CreateAta {
            payer: ctx.accounts.payer.to_account_info(),
            associated_token: ctx.accounts.seller_payment.to_account_info(),
            authority: ctx.accounts.seller.to_account_info(),
            mint: ctx.accounts.payment_mint.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            token_program: ctx.accounts.payment_token_program.to_account_info(),
        },
    ))?;
    if fee > 0 {
        create_idempotent(CpiContext::new(
            ctx.accounts.associated_token_program.key(),
            CreateAta {
                payer: ctx.accounts.payer.to_account_info(),
                associated_token: ctx.accounts.treasury_payment.to_account_info(),
                authority: ctx.accounts.treasury.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
                token_program: ctx.accounts.payment_token_program.to_account_info(),
            },
        ))?;
    }
    let id_bytes = id.to_le_bytes();
    let vault_seeds: &[&[u8]] = &[
        OFFER_VAULT_SEED,
        &id_bytes,
        offeror_key.as_ref(),
        &[ctx.bumps.offer_vault],
    ];
    let seller_part = drain_offer_vault(
        &ctx.accounts.payment_token_program.to_account_info(),
        &ctx.accounts.payment_mint.to_account_info(),
        &ctx.accounts.offer_vault.to_account_info(),
        &ctx.accounts.vault_payment_account.to_account_info(),
        &ctx.accounts.offer_rent_payer.to_account_info(),
        vault_seeds,
        &[(&ctx.accounts.treasury_payment.to_account_info(), fee)],
        &ctx.accounts.seller_payment.to_account_info(),
        mint_decimals,
    )?;

    move_shares(
        &ctx.accounts.share_token_program.to_account_info(),
        &ctx.accounts.associated_token_program.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        &ctx.accounts.payer.to_account_info(),
        &ctx.accounts.share_mint.to_account_info(),
        &ctx.accounts.mint_auth.to_account_info(),
        ctx.bumps.mint_auth,
        asset_id,
        &ctx.accounts.seller_share_account.to_account_info(),
        &ctx.accounts.offeror_share_account.to_account_info(),
        &ctx.accounts.offeror.to_account_info(),
        amount,
    )?;

    // Ledger: sold shares leave the seller's balance and listing reserve
    // together, keeping the lock invariant intact.
    let seller_holding = &mut ctx.accounts.seller_holding;
    seller_holding.amount = seller_holding
        .amount
        .checked_sub(amount)
        .ok_or(MarketplaceError::Overflow)?;
    seller_holding.listed = seller_holding
        .listed
        .checked_sub(amount)
        .ok_or(MarketplaceError::Overflow)?;
    let offeror_holding = &mut ctx.accounts.offeror_holding;
    offeror_holding.amount = offeror_holding
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
        share_listing.close(ctx.accounts.listing_rent_payer.to_account_info())?;
    }
    ctx.accounts
        .offer
        .close(ctx.accounts.offer_rent_payer.to_account_info())?;

    emit!(OfferAccepted {
        listing_id: id,
        asset_id,
        offeror: offeror_key,
        seller: seller_key,
        amount,
        paid: seller_part
            .checked_add(fee)
            .ok_or(MarketplaceError::Overflow)?,
        fee,
        remaining,
    });
    Ok(())
}

/// The seller turns an offer down; the bid goes back to the bidder and the
/// offer closes.
#[derive(Accounts)]
#[instruction(id: u64)]
pub struct RejectOffer<'info> {
    pub seller: Signer<'info>,

    /// Whoever fronts rent for the bidder's refund account if it needs
    /// creating.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [SHARE_LISTING_SEED, &id.to_le_bytes()],
        bump = share_listing.bump,
        constraint = share_listing.seller == seller.key() @ MarketplaceError::WrongSeller,
    )]
    pub share_listing: Box<Account<'info, ShareListing>>,

    /// CHECK: the bidder; bound to the offer by its seeds, authority of the
    /// refund account below.
    pub offeror: UncheckedAccount<'info>,

    #[account(
        mut,
        close = offer_rent_payer,
        seeds = [OFFER_SEED, &id.to_le_bytes(), offeror.key().as_ref()],
        bump = offer.bump,
    )]
    pub offer: Box<Account<'info, Offer>>,

    /// CHECK: the wallet that fronted the offer's rent; gets the offer and
    /// its vault account back as they close.
    #[account(mut, address = offer.rent_payer @ MarketplaceError::WrongRentPayer)]
    pub offer_rent_payer: UncheckedAccount<'info>,

    /// CHECK: the mint the offer was made in, pinned by address.
    #[account(address = offer.payment_mint @ MarketplaceError::PaymentMintMismatch)]
    pub payment_mint: UncheckedAccount<'info>,

    /// CHECK: the offer's vault authority.
    #[account(seeds = [OFFER_VAULT_SEED, &id.to_le_bytes(), offeror.key().as_ref()], bump)]
    pub offer_vault: UncheckedAccount<'info>,

    /// CHECK: the vault's token account the refund leaves; the handler pins
    /// it to its derivation.
    #[account(mut)]
    pub vault_payment_account: UncheckedAccount<'info>,

    /// CHECK: the bidder's associated account for the refunded mint;
    /// created idempotently, so a closed account can't strand the refund.
    #[account(mut)]
    pub offeror_payment: UncheckedAccount<'info>,

    pub payment_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn reject_offer_handler(ctx: Context<RejectOffer>, id: u64, nonce: u64) -> Result<()> {
    require!(
        ctx.accounts.offer.nonce == nonce,
        MarketplaceError::OfferNonceMismatch
    );
    let refunded = refund_offer(
        &ctx.accounts.payment_token_program,
        &ctx.accounts.associated_token_program,
        &ctx.accounts.system_program,
        &ctx.accounts.payer,
        &ctx.accounts.payment_mint,
        &ctx.accounts.offer_vault,
        &ctx.accounts.vault_payment_account,
        &ctx.accounts.offeror,
        &ctx.accounts.offeror_payment,
        &ctx.accounts.offer_rent_payer,
        id,
        ctx.bumps.offer_vault,
    )?;
    emit!(OfferRejected {
        listing_id: id,
        offeror: ctx.accounts.offeror.key(),
        refunded,
    });
    Ok(())
}

/// The bidder walks away and takes the bid back. Role-free, and standing
/// alone from the listing: the refund works even after the listing sold
/// out or was delisted.
#[derive(Accounts)]
pub struct CancelOffer<'info> {
    pub offeror: Signer<'info>,

    /// Whoever fronts rent for the refund account if it needs creating.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        mut,
        close = offer_rent_payer,
        seeds = [OFFER_SEED, &offer.listing_id.to_le_bytes(), offeror.key().as_ref()],
        bump = offer.bump,
    )]
    pub offer: Box<Account<'info, Offer>>,

    /// CHECK: the wallet that fronted the offer's rent; gets the offer and
    /// its vault account back as they close.
    #[account(mut, address = offer.rent_payer @ MarketplaceError::WrongRentPayer)]
    pub offer_rent_payer: UncheckedAccount<'info>,

    /// CHECK: the mint the offer was made in, pinned by address.
    #[account(address = offer.payment_mint @ MarketplaceError::PaymentMintMismatch)]
    pub payment_mint: UncheckedAccount<'info>,

    /// CHECK: the offer's vault authority.
    #[account(
        seeds = [OFFER_VAULT_SEED, &offer.listing_id.to_le_bytes(), offeror.key().as_ref()],
        bump,
    )]
    pub offer_vault: UncheckedAccount<'info>,

    /// CHECK: the vault's token account the refund leaves; the handler pins
    /// it to its derivation.
    #[account(mut)]
    pub vault_payment_account: UncheckedAccount<'info>,

    /// CHECK: the bidder's associated account for the refunded mint;
    /// created idempotently.
    #[account(mut)]
    pub offeror_payment: UncheckedAccount<'info>,

    pub payment_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn cancel_offer_handler(ctx: Context<CancelOffer>) -> Result<()> {
    let id = ctx.accounts.offer.listing_id;
    let refunded = refund_offer(
        &ctx.accounts.payment_token_program,
        &ctx.accounts.associated_token_program,
        &ctx.accounts.system_program,
        &ctx.accounts.payer,
        &ctx.accounts.payment_mint,
        &ctx.accounts.offer_vault,
        &ctx.accounts.vault_payment_account,
        &ctx.accounts.offeror,
        &ctx.accounts.offeror_payment,
        &ctx.accounts.offer_rent_payer,
        id,
        ctx.bumps.offer_vault,
    )?;
    emit!(OfferCancelled {
        listing_id: id,
        offeror: ctx.accounts.offeror.key(),
        refunded,
    });
    Ok(())
}

/// Return the held bid to the bidder's ATA (created if needed) and close
/// the vault's token account.
#[allow(clippy::too_many_arguments)]
fn refund_offer<'info>(
    payment_token_program: &Interface<'info, TokenInterface>,
    associated_token_program: &Program<'info, AssociatedToken>,
    system_program: &Program<'info, System>,
    payer: &Signer<'info>,
    payment_mint: &UncheckedAccount<'info>,
    offer_vault: &UncheckedAccount<'info>,
    vault_payment_account: &UncheckedAccount<'info>,
    offeror: &AccountInfo<'info>,
    offeror_payment: &UncheckedAccount<'info>,
    offer_rent_payer: &UncheckedAccount<'info>,
    id: u64,
    vault_bump: u8,
) -> Result<u64> {
    require!(
        vault_payment_account.key()
            == get_associated_token_address_with_program_id(
                &offer_vault.key(),
                &payment_mint.key(),
                &payment_token_program.key(),
            ),
        MarketplaceError::WrongVaultAccount
    );
    let mint_decimals = {
        let data = payment_mint.try_borrow_data()?;
        StateWithExtensions::<MintState>::unpack(&data)?
            .base
            .decimals
    };
    create_idempotent(CpiContext::new(
        associated_token_program.key(),
        CreateAta {
            payer: payer.to_account_info(),
            associated_token: offeror_payment.to_account_info(),
            authority: offeror.clone(),
            mint: payment_mint.to_account_info(),
            system_program: system_program.to_account_info(),
            token_program: payment_token_program.to_account_info(),
        },
    ))?;
    let id_bytes = id.to_le_bytes();
    let offeror_key = offeror.key();
    let vault_seeds: &[&[u8]] = &[
        OFFER_VAULT_SEED,
        &id_bytes,
        offeror_key.as_ref(),
        &[vault_bump],
    ];
    drain_offer_vault(
        &payment_token_program.to_account_info(),
        &payment_mint.to_account_info(),
        &offer_vault.to_account_info(),
        &vault_payment_account.to_account_info(),
        &offer_rent_payer.to_account_info(),
        vault_seeds,
        &[],
        &offeror_payment.to_account_info(),
        mint_decimals,
    )
}

#[event]
pub struct OfferMade {
    pub listing_id: u64,
    pub asset_id: u64,
    pub offeror: Pubkey,
    pub share_price: u64,
    pub amount: u32,
    pub mint: Pubkey,
    pub held: u64,
    pub nonce: u64,
}

#[event]
pub struct OfferAccepted {
    pub listing_id: u64,
    pub asset_id: u64,
    pub offeror: Pubkey,
    pub seller: Pubkey,
    pub amount: u32,
    /// What the bidder's vault paid out in the mint's units, and the slice
    /// that went to the treasury.
    pub paid: u64,
    pub fee: u64,
    pub remaining: u32,
}

#[event]
pub struct OfferRejected {
    pub listing_id: u64,
    pub offeror: Pubkey,
    pub refunded: u64,
}

#[event]
pub struct OfferCancelled {
    pub listing_id: u64,
    pub offeror: Pubkey,
    pub refunded: u64,
}
