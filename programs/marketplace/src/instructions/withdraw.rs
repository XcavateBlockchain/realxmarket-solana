use anchor_lang::prelude::*;
use anchor_spl::associated_token::{create_idempotent, AssociatedToken, Create as CreateAta};
use anchor_spl::token_2022::spl_token_2022::{
    extension::StateWithExtensions,
    state::{Account as TokenAccountState, Mint as MintState},
};
use anchor_spl::token_2022::{freeze_account, thaw_account, FreezeAccount, ThawAccount, Token2022};
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{
    CONFIG_SEED, LISTING_SEED, LISTING_VAULT_SEED, MINT_AUTH_SEED, POSITION_SEED, PROPERTY_SEED,
    PROPERTY_VAULT_SEED, SHARE_MINT_SEED, SHARE_SEED, VAULT_SEED,
};
use crate::error::MarketplaceError;
use crate::instructions::buy::{scale_from_mint, scale_to_mint};
use crate::state::{Config, InvestorPosition, Listing, ListingStatus, PropertyAsset, ShareHolding};
use crate::vault::release_from_vault;

/// Take everything back out of a listing that expired before selling out:
/// the shares return to the property vault and the full payment, fee and tax
/// included, comes back from the listing vault. The first withdrawal moves
/// the listing to `Expired`. Both accounts close, since a dead listing can't
/// be bought into again. Deliberately not role-gated: exits never are.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct WithdrawExpired<'info> {
    pub investor: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    /// CHECK: the sponsor wallet that fronted the accounts' rent; gets it
    /// back as they close.
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
        seeds = [PROPERTY_SEED, &listing_id.to_le_bytes()],
        bump = property.bump,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    #[account(
        mut,
        close = rent_collector,
        seeds = [POSITION_SEED, &listing_id.to_le_bytes(), investor.key().as_ref()],
        bump = position.bump,
    )]
    pub position: Box<Account<'info, InvestorPosition>>,

    #[account(
        mut,
        close = rent_collector,
        seeds = [SHARE_SEED, &listing_id.to_le_bytes(), investor.key().as_ref()],
        bump = holding.bump,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    /// CHECK: the mint the position was paid in, pinned by address; kept
    /// untyped to spare `try_accounts` stack, the reserve already vetted it.
    #[account(address = position.payment_mint @ MarketplaceError::PaymentMintMismatch)]
    pub payment_mint: UncheckedAccount<'info>,

    /// CHECK: the investor's payment account the refund lands in; kept
    /// untyped to spare `try_accounts` stack, the handler checks its mint
    /// and owner. Deliberately not pinned to the recorded account: that one
    /// may be closed, and a refund must never depend on it.
    #[account(mut)]
    pub investor_payment: UncheckedAccount<'info>,

    /// CHECK: the listing vault authority; a bare PDA owning the vault's
    /// token accounts.
    #[account(seeds = [LISTING_VAULT_SEED, &listing_id.to_le_bytes()], bump)]
    pub listing_vault: UncheckedAccount<'info>,

    /// CHECK: the vault's account for the payment mint, funded by the buys;
    /// pinned to its derivation.
    #[account(
        mut,
        address = anchor_spl::associated_token::get_associated_token_address_with_program_id(
            &listing_vault.key(),
            &payment_mint.key(),
            &payment_token_program.key(),
        ) @ MarketplaceError::WrongVaultAccount,
    )]
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

    /// CHECK: the vault's share account the shares return to, pinned to its
    /// derivation.
    #[account(
        mut,
        address = anchor_spl::associated_token::get_associated_token_address_with_program_id(
            &property_vault.key(),
            &share_mint.key(),
            &share_token_program.key(),
        ) @ MarketplaceError::WrongVaultAccount,
    )]
    pub vault_share_account: UncheckedAccount<'info>,

    /// CHECK: the investor's associated share account the shares leave,
    /// pinned to its derivation; the token program rules on the balance.
    #[account(
        mut,
        address = anchor_spl::associated_token::get_associated_token_address_with_program_id(
            &investor.key(),
            &share_mint.key(),
            &share_token_program.key(),
        ) @ MarketplaceError::WrongVaultAccount,
    )]
    pub investor_share_account: UncheckedAccount<'info>,

    /// The payment mint's token program (classic or Token-2022).
    pub payment_token_program: Interface<'info, TokenInterface>,
    /// The share mint's program is always Token-2022.
    pub share_token_program: Program<'info, Token2022>,
}

pub fn withdraw_expired_handler(ctx: Context<WithdrawExpired>, listing_id: u64) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    match ctx.accounts.listing.status {
        // The first withdrawal proves the expiry and flips the status, which
        // also opens the cancelled-position crank.
        ListingStatus::Listed => {
            require!(
                now >= ctx.accounts.listing.listing_expiry,
                MarketplaceError::ListingNotExpired
            );
            ctx.accounts.listing.status = ListingStatus::Expired;
        }
        ListingStatus::Expired => {}
        _ => return err!(MarketplaceError::ListingNotActive),
    }
    let investor = ctx.accounts.investor.key();
    let (amount, refund, payment_mint) = settle_dead_listing_exit(ctx, listing_id, true)?;

    emit!(ExpiredSharesWithdrawn {
        listing_id,
        investor,
        amount,
        payment_mint,
        refunded: refund,
    });
    Ok(())
}

/// The timeout exit for a sale that sold out but whose legal process never
/// settled: once the deadline passes, investors take their money back the
/// same way they would from an expired listing. The first withdrawal moves
/// the listing to `Refunding`. This is what keeps a successful sale from
/// ever being a trap, approved documents included, in case settlement
/// never executes.
pub fn withdraw_legal_process_expired_handler(
    ctx: Context<WithdrawExpired>,
    listing_id: u64,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    match ctx.accounts.listing.status {
        ListingStatus::SoldOut | ListingStatus::Legal => {
            require!(
                now > ctx.accounts.listing.legal_deadline,
                MarketplaceError::LegalProcessNotExpired
            );
            ctx.accounts.listing.status = ListingStatus::Refunding;
        }
        ListingStatus::Refunding => {}
        _ => return err!(MarketplaceError::ListingNotActive),
    }
    let investor = ctx.accounts.investor.key();
    let (amount, refund, payment_mint) = settle_dead_listing_exit(ctx, listing_id, true)?;

    emit!(LegalTimeoutSharesWithdrawn {
        listing_id,
        investor,
        amount,
        payment_mint,
        refunded: refund,
    });
    Ok(())
}

/// The exit from a sale the lawyers rejected. The rejection already set the
/// status, so there is no deadline to prove. Unlike the expiry exits, the
/// buyer fee stays behind: the review that killed the sale still gets
/// paid from it, and `settle_cancelled_fees` distributes what is retained.
pub fn withdraw_cancelled_handler(ctx: Context<WithdrawExpired>, listing_id: u64) -> Result<()> {
    require!(
        ctx.accounts.listing.status == ListingStatus::Cancelled,
        MarketplaceError::ListingNotActive
    );
    let investor = ctx.accounts.investor.key();
    let (amount, refund, payment_mint) = settle_dead_listing_exit(ctx, listing_id, false)?;

    emit!(CancelledSharesWithdrawn {
        listing_id,
        investor,
        amount,
        payment_mint,
        refunded: refund,
    });
    Ok(())
}

/// Distribute the fees a cancelled sale retained, one payment mint per
/// call: the SPV lawyer collects what they are still owed and the treasury
/// takes the rest, emptying the vault account so teardown can close it.
/// Permissionless, and only once every refund is out, because until then
/// the account still holds investor money.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct SettleCancelledFees<'info> {
    #[account(mut)]
    pub cranker: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
    )]
    pub listing: Box<Account<'info, Listing>>,

    /// Must be a mint the sale actually collected; the listing's own record,
    /// so a config rotation can't strand the payout.
    #[account(
        constraint = listing.collected.iter().any(|c| c.mint == payment_mint.key())
            @ MarketplaceError::InvalidMint,
    )]
    pub payment_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: the listing vault authority; signs the payouts.
    #[account(seeds = [LISTING_VAULT_SEED, &listing_id.to_le_bytes()], bump)]
    pub listing_vault: UncheckedAccount<'info>,

    /// The vault account holding the retained fees for this mint.
    #[account(
        mut,
        associated_token::mint = payment_mint,
        associated_token::authority = listing_vault,
        associated_token::token_program = payment_token_program,
    )]
    pub listing_payment_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: the payee lawyer, from the listing; authority of the ATA
    /// below.
    #[account(address = listing.spv_costs_payee @ MarketplaceError::WrongPayee)]
    pub lawyer: UncheckedAccount<'info>,

    /// CHECK: the lawyer's ATA for this mint, created here if it doesn't
    /// exist yet, so a closed account can't strand their costs.
    #[account(mut)]
    pub lawyer_payment_account: UncheckedAccount<'info>,

    /// CHECK: the treasury owner key from config; authority of the ATA below.
    #[account(address = config.treasury @ MarketplaceError::InvalidConfig)]
    pub treasury: UncheckedAccount<'info>,

    /// CHECK: the treasury's ATA for this mint, created here if it doesn't
    /// exist yet; the ATA program verifies the derivation.
    #[account(mut)]
    pub treasury_payment_account: UncheckedAccount<'info>,

    pub payment_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn settle_cancelled_fees_handler(
    ctx: Context<SettleCancelledFees>,
    listing_id: u64,
) -> Result<()> {
    require!(
        ctx.accounts.listing.status == ListingStatus::Cancelled,
        MarketplaceError::ListingNotActive
    );
    require!(
        ctx.accounts.listing.sold_share_amount == 0,
        MarketplaceError::SharesOutstanding
    );
    let pot = ctx.accounts.listing_payment_account.amount;
    // Idempotent: a mint that retained nothing, or was already drained,
    // settles as a no-op so a wind-down script can crank every mint blindly.
    if pot == 0 {
        return Ok(());
    }

    // The split is computed in the quote scale the costs were named in, so
    // the amount owed carries exactly across mints of different decimals.
    let decimals = ctx.accounts.payment_mint.decimals;
    let pay_quote = scale_from_mint(pot, decimals)?.min(ctx.accounts.listing.spv_costs_due);
    let lawyer_cut = scale_to_mint(pay_quote, decimals)?;
    ctx.accounts.listing.spv_costs_due = ctx
        .accounts
        .listing
        .spv_costs_due
        .checked_sub(pay_quote)
        .ok_or(MarketplaceError::Overflow)?;

    let id_bytes = listing_id.to_le_bytes();
    let vault_seeds: &[&[u8]] = &[LISTING_VAULT_SEED, &id_bytes, &[ctx.bumps.listing_vault]];
    if lawyer_cut > 0 {
        create_idempotent(CpiContext::new(
            ctx.accounts.associated_token_program.key(),
            CreateAta {
                payer: ctx.accounts.cranker.to_account_info(),
                associated_token: ctx.accounts.lawyer_payment_account.to_account_info(),
                authority: ctx.accounts.lawyer.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
                token_program: ctx.accounts.payment_token_program.to_account_info(),
            },
        ))?;
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.payment_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.listing_payment_account.to_account_info(),
                    mint: ctx.accounts.payment_mint.to_account_info(),
                    to: ctx.accounts.lawyer_payment_account.to_account_info(),
                    authority: ctx.accounts.listing_vault.to_account_info(),
                },
                &[vault_seeds],
            ),
            lawyer_cut,
            decimals,
        )?;
    }
    let treasury_cut = pot
        .checked_sub(lawyer_cut)
        .ok_or(MarketplaceError::Overflow)?;
    if treasury_cut > 0 {
        create_idempotent(CpiContext::new(
            ctx.accounts.associated_token_program.key(),
            CreateAta {
                payer: ctx.accounts.cranker.to_account_info(),
                associated_token: ctx.accounts.treasury_payment_account.to_account_info(),
                authority: ctx.accounts.treasury.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
                token_program: ctx.accounts.payment_token_program.to_account_info(),
            },
        ))?;
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.payment_token_program.key(),
                TransferChecked {
                    from: ctx.accounts.listing_payment_account.to_account_info(),
                    mint: ctx.accounts.payment_mint.to_account_info(),
                    to: ctx.accounts.treasury_payment_account.to_account_info(),
                    authority: ctx.accounts.listing_vault.to_account_info(),
                },
                &[vault_seeds],
            ),
            treasury_cut,
            decimals,
        )?;
    }

    emit!(CancelledFeesSettled {
        listing_id,
        payment_mint: ctx.accounts.payment_mint.key(),
        lawyer_paid: lawyer_cut,
        treasury_paid: treasury_cut,
    });
    Ok(())
}

/// The shared exit body: shares back to the property vault, the payment
/// back to the investor (with or without the fee), both per-investor
/// accounts closed, the counters unwound. Callers have already validated
/// and transitioned the status.
fn settle_dead_listing_exit(
    ctx: Context<WithdrawExpired>,
    listing_id: u64,
    include_fee: bool,
) -> Result<(u32, u64, Pubkey)> {
    let amount = ctx.accounts.position.share_amount;
    require!(amount > 0, MarketplaceError::NothingToUnreserve);
    // Closing the position would orphan an unclaimed reservation: the
    // position is the only key to it. The release crank clears it first.
    require!(
        ctx.accounts.position.reserved_share_amount == 0,
        MarketplaceError::ReservationOutstanding
    );
    // The ledger must agree with the position before it closes on the
    // position's number, and no locked share may leave. A paid position is
    // never also reserved (`create_spv`'s full-reservation rule), so the
    // reservation guard below can't trap one.
    require!(
        ctx.accounts.holding.amount == amount,
        MarketplaceError::LedgerMismatch
    );
    require!(
        ctx.accounts.holding.locked() == 0,
        MarketplaceError::SharesLocked
    );

    let fee = if include_fee {
        ctx.accounts.position.paid_fee
    } else {
        0
    };
    let refund = ctx
        .accounts
        .position
        .paid_funds
        .checked_add(fee)
        .and_then(|r| r.checked_add(ctx.accounts.position.paid_tax))
        .ok_or(MarketplaceError::Overflow)?;

    // Shares back to the property vault through the usual airlock.
    let id_bytes = listing_id.to_le_bytes();
    let auth_seeds: &[&[u8]] = &[MINT_AUTH_SEED, &id_bytes, &[ctx.bumps.mint_auth]];
    let vault_seeds: &[&[u8]] = &[LISTING_VAULT_SEED, &id_bytes, &[ctx.bumps.listing_vault]];
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
        CpiContext::new(
            ctx.accounts.share_token_program.key(),
            TransferChecked {
                from: ctx.accounts.investor_share_account.to_account_info(),
                mint: ctx.accounts.share_mint.to_account_info(),
                to: ctx.accounts.vault_share_account.to_account_info(),
                authority: ctx.accounts.investor.to_account_info(),
            },
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

    // The refund account must belong to the investor and carry the paid
    // mint; any such account works, so a closed original can't strand it.
    {
        let data = ctx.accounts.investor_payment.try_borrow_data()?;
        let account = StateWithExtensions::<TokenAccountState>::unpack(&data)?;
        require!(
            account.base.mint == ctx.accounts.position.payment_mint
                && account.base.owner == ctx.accounts.investor.key(),
            MarketplaceError::PaymentMintMismatch
        );
    }
    let mint_decimals = {
        let data = ctx.accounts.payment_mint.try_borrow_data()?;
        StateWithExtensions::<MintState>::unpack(&data)?
            .base
            .decimals
    };
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.payment_token_program.key(),
            TransferChecked {
                from: ctx.accounts.listing_payment_account.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                to: ctx.accounts.investor_payment.to_account_info(),
                authority: ctx.accounts.listing_vault.to_account_info(),
            },
            &[vault_seeds],
        ),
        refund,
        mint_decimals,
    )?;

    ctx.accounts.property.holder_count = ctx
        .accounts
        .property
        .holder_count
        .checked_sub(1)
        .ok_or(MarketplaceError::Overflow)?;
    let listing = &mut ctx.accounts.listing;
    listing.sold_share_amount = listing
        .sold_share_amount
        .checked_sub(amount)
        .ok_or(MarketplaceError::Overflow)?;
    // The position closes with this exit.
    listing.position_count = listing
        .position_count
        .checked_sub(1)
        .ok_or(MarketplaceError::Overflow)?;

    Ok((amount, refund, ctx.accounts.position.payment_mint))
}

/// Give the developer their XCAV deposit back once the listing is dead with
/// no shares in investor hands: abandoned before the assets ever existed, or
/// expired with nothing sold (or everything withdrawn). A sold-out sale the
/// developer let time out without ever appointing their lawyer costs them 1%
/// of the bond, paid to the treasury. Developer-only, and deliberately not
/// role-gated: exits never are.
#[derive(Accounts)]
#[instruction(listing_id: u64)]
pub struct WithdrawDepositUnsold<'info> {
    pub developer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [LISTING_SEED, &listing_id.to_le_bytes()],
        bump = listing.bump,
        constraint = listing.developer == developer.key() @ MarketplaceError::NotListingDeveloper,
    )]
    pub listing: Box<Account<'info, Listing>>,

    /// The XCAV mint (for `transfer_checked`).
    #[account(address = config.xcav_mint @ MarketplaceError::InvalidMint)]
    pub xcav_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The developer's XCAV account the deposit is returned to.
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

    /// CHECK: the treasury's XCAV account the slash lands in; only needed
    /// when one applies, and pinned to the derivation then.
    #[account(mut)]
    pub treasury_token: Option<UncheckedAccount<'info>>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn withdraw_deposit_unsold_handler(
    ctx: Context<WithdrawDepositUnsold>,
    listing_id: u64,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    match ctx.accounts.listing.status {
        // Never opened for sale: the developer can abandon it right away.
        ListingStatus::PendingAssets => {}
        ListingStatus::Listed | ListingStatus::Expired => {
            require!(
                now >= ctx.accounts.listing.listing_expiry,
                MarketplaceError::ListingNotExpired
            );
        }
        // These already proved their own conditions (Cancelled arrives with
        // the documents-rejection flow); once every investor has withdrawn,
        // the deposit follows.
        ListingStatus::Refunding | ListingStatus::Cancelled => {}
        _ => return err!(MarketplaceError::ListingNotActive),
    }
    require!(
        ctx.accounts.listing.sold_share_amount == 0,
        MarketplaceError::SharesOutstanding
    );
    let deposit = ctx.accounts.listing.deposit;
    require!(deposit > 0, MarketplaceError::DepositAlreadyWithdrawn);

    // Letting a sold-out sale die without ever appointing a lawyer costs 1%
    // of the bond, whether it ran into the deadline or the silence crank
    // cancelled it; investors had their money locked for nothing either way.
    let slash = if matches!(
        ctx.accounts.listing.status,
        ListingStatus::Refunding | ListingStatus::Cancelled
    ) && !ctx.accounts.listing.developer_engaged
    {
        deposit / 100
    } else {
        0
    };
    if slash > 0 {
        let treasury_token = ctx
            .accounts
            .treasury_token
            .as_ref()
            .ok_or(MarketplaceError::InvalidConfig)?;
        require!(
            treasury_token.key()
                == anchor_spl::associated_token::get_associated_token_address_with_program_id(
                    &ctx.accounts.config.treasury,
                    &ctx.accounts.xcav_mint.key(),
                    &ctx.accounts.token_program.key(),
                ),
            MarketplaceError::WrongVaultAccount
        );
        release_from_vault(
            &ctx.accounts.token_program.to_account_info(),
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.xcav_mint.to_account_info(),
            &treasury_token.to_account_info(),
            &ctx.accounts.config.to_account_info(),
            ctx.accounts.config.bump,
            slash,
            ctx.accounts.xcav_mint.decimals,
        )?;
    }
    release_from_vault(
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.vault.to_account_info(),
        &ctx.accounts.xcav_mint.to_account_info(),
        &ctx.accounts.developer_token.to_account_info(),
        &ctx.accounts.config.to_account_info(),
        ctx.accounts.config.bump,
        deposit
            .checked_sub(slash)
            .ok_or(MarketplaceError::Overflow)?,
        ctx.accounts.xcav_mint.decimals,
    )?;

    let listing = &mut ctx.accounts.listing;
    listing.deposit = 0;
    if !matches!(
        listing.status,
        ListingStatus::Refunding | ListingStatus::Cancelled
    ) {
        listing.status = ListingStatus::Expired;
    }

    emit!(ListingDepositWithdrawn {
        listing_id,
        developer: ctx.accounts.developer.key(),
        deposit,
        slashed: slash,
    });
    Ok(())
}

#[event]
pub struct ExpiredSharesWithdrawn {
    pub listing_id: u64,
    pub investor: Pubkey,
    pub amount: u32,
    pub payment_mint: Pubkey,
    pub refunded: u64,
}

#[event]
pub struct LegalTimeoutSharesWithdrawn {
    pub listing_id: u64,
    pub investor: Pubkey,
    pub amount: u32,
    pub payment_mint: Pubkey,
    pub refunded: u64,
}

#[event]
pub struct CancelledSharesWithdrawn {
    pub listing_id: u64,
    pub investor: Pubkey,
    pub amount: u32,
    pub payment_mint: Pubkey,
    pub refunded: u64,
}

#[event]
pub struct CancelledFeesSettled {
    pub listing_id: u64,
    pub payment_mint: Pubkey,
    pub lawyer_paid: u64,
    pub treasury_paid: u64,
}

#[event]
pub struct ListingDepositWithdrawn {
    pub listing_id: u64,
    pub developer: Pubkey,
    pub deposit: u64,
    /// The abandonment slash the treasury kept, if one applied.
    pub slashed: u64,
}
