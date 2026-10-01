use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};
use xcavate_whitelist::state::{Role, RoleAccount};

use crate::constants::*;
use crate::error::HubError;
use crate::state::{BuyerPosition, Config, Hub, HubStatus};

#[derive(Accounts)]
#[instruction(hub_id: u64)]
pub struct BuyTokens<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [HUB_SEED, &hub_id.to_le_bytes()], bump = hub.bump)]
    pub hub: Box<Account<'info, Hub>>,
    // The existing investor role is reused for this demo; no shared role tags
    // are changed just to introduce another sale flow.
    #[account(
        seeds = [xcavate_whitelist::ROLE_SEED, buyer.key().as_ref(), &[Role::RealEstateInvestor.seed_byte()]],
        bump = buyer_role.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = buyer_role.is_compliant() @ HubError::NotCompliant,
    )]
    pub buyer_role: Box<Account<'info, RoleAccount>>,
    #[account(address = config.payment_mint @ HubError::InvalidMint)]
    pub payment_mint: Box<Account<'info, Mint>>,
    #[account(mut, token::mint = payment_mint, token::authority = buyer)]
    pub buyer_token: Box<Account<'info, TokenAccount>>,
    #[account(mut, seeds = [PAYMENT_VAULT_SEED, &hub_id.to_le_bytes()], bump, token::mint = payment_mint, token::authority = hub)]
    pub payment_vault: Box<Account<'info, TokenAccount>>,
    #[account(init_if_needed, payer = buyer, space = 8 + BuyerPosition::INIT_SPACE, seeds = [POSITION_SEED, &hub_id.to_le_bytes(), buyer.key().as_ref()], bump)]
    pub position: Box<Account<'info, BuyerPosition>>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn buy_tokens_handler(
    ctx: Context<BuyTokens>,
    hub_id: u64,
    amount: u64,
    max_total_cost: u64,
) -> Result<()> {
    let hub = &ctx.accounts.hub;
    require!(hub.status == HubStatus::Listed, HubError::InvalidStatus);
    require!(
        Clock::get()?.unix_timestamp < hub.sale_deadline,
        HubError::SaleExpired
    );
    require!(amount > 0, HubError::InvalidAmount);
    let tokens_sold = hub
        .tokens_sold
        .checked_add(amount)
        .ok_or(HubError::Overflow)?;
    require!(tokens_sold <= hub.token_supply, HubError::InvalidAmount);
    let cost = hub
        .token_price
        .checked_mul(amount)
        .ok_or(HubError::Overflow)?;
    require!(cost <= max_total_cost, HubError::CostTooHigh);
    let total_paid = hub.total_paid.checked_add(cost).ok_or(HubError::Overflow)?;
    let position = &mut ctx.accounts.position;
    require!(!position.settled, HubError::AlreadySettled);
    if position.hub == Pubkey::default() {
        position.hub = hub.key();
        position.buyer = ctx.accounts.buyer.key();
        position.bump = ctx.bumps.position;
    }
    require!(
        position.hub == hub.key() && position.buyer == ctx.accounts.buyer.key(),
        HubError::InvalidPosition
    );
    let position_amount = position
        .amount
        .checked_add(amount)
        .ok_or(HubError::Overflow)?;
    let position_paid = position.paid.checked_add(cost).ok_or(HubError::Overflow)?;
    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.buyer_token.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                to: ctx.accounts.payment_vault.to_account_info(),
                authority: ctx.accounts.buyer.to_account_info(),
            },
        ),
        cost,
        ctx.accounts.payment_mint.decimals,
    )?;
    position.amount = position_amount;
    position.paid = position_paid;
    let hub = &mut ctx.accounts.hub;
    hub.tokens_sold = tokens_sold;
    hub.total_paid = total_paid;
    // Buyers hold an allocation until the entire sale succeeds. Failed-sale
    // refunds therefore never depend on recovering transferable hub tokens.
    if tokens_sold == hub.token_supply {
        hub.status = HubStatus::Funded;
    }
    emit!(TokensPurchased {
        hub_id,
        buyer: ctx.accounts.buyer.key(),
        amount,
        cost
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(hub_id: u64)]
pub struct FinalizeSale<'info> {
    pub cranker: Signer<'info>,
    #[account(mut, seeds = [HUB_SEED, &hub_id.to_le_bytes()], bump = hub.bump)]
    pub hub: Box<Account<'info, Hub>>,
}

pub fn finalize_sale_handler(ctx: Context<FinalizeSale>, hub_id: u64) -> Result<()> {
    let hub = &mut ctx.accounts.hub;
    require!(hub.status == HubStatus::Listed, HubError::InvalidStatus);
    require!(
        Clock::get()?.unix_timestamp >= hub.sale_deadline,
        HubError::SaleStillOpen
    );
    hub.status = HubStatus::Failed;
    emit!(SaleFailed {
        hub_id,
        total_paid: hub.total_paid
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(hub_id: u64)]
pub struct ClaimTokens<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(mut, seeds = [HUB_SEED, &hub_id.to_le_bytes()], bump = hub.bump)]
    pub hub: Box<Account<'info, Hub>>,
    #[account(mut, seeds = [POSITION_SEED, &hub_id.to_le_bytes(), buyer.key().as_ref()], bump = position.bump,
        has_one = buyer @ HubError::InvalidPosition, has_one = hub @ HubError::InvalidPosition)]
    pub position: Box<Account<'info, BuyerPosition>>,
    #[account(address = hub.token_mint @ HubError::InvalidMint)]
    pub hub_mint: Box<Account<'info, Mint>>,
    #[account(mut, seeds = [TOKEN_VAULT_SEED, &hub_id.to_le_bytes()], bump, token::mint = hub_mint, token::authority = hub)]
    pub token_vault: Box<Account<'info, TokenAccount>>,
    #[account(init_if_needed, payer = buyer, associated_token::mint = hub_mint, associated_token::authority = buyer)]
    pub buyer_token: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn claim_tokens_handler(ctx: Context<ClaimTokens>, hub_id: u64) -> Result<()> {
    let hub = &ctx.accounts.hub;
    let position = &ctx.accounts.position;
    require!(hub.status == HubStatus::Funded, HubError::InvalidStatus);
    require!(!position.settled, HubError::AlreadySettled);
    require!(position.amount > 0, HubError::EmptyPosition);
    let tokens_claimed = hub
        .tokens_claimed
        .checked_add(position.amount)
        .ok_or(HubError::Overflow)?;
    let id_bytes = hub_id.to_le_bytes();
    let bump = [hub.bump];
    let seeds: &[&[u8]] = &[HUB_SEED, &id_bytes, &bump];
    token::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.token_vault.to_account_info(),
                mint: ctx.accounts.hub_mint.to_account_info(),
                to: ctx.accounts.buyer_token.to_account_info(),
                authority: hub.to_account_info(),
            },
            &[seeds],
        ),
        position.amount,
        0,
    )?;
    ctx.accounts.position.settled = true;
    ctx.accounts.hub.tokens_claimed = tokens_claimed;
    emit!(TokensClaimed {
        hub_id,
        buyer: ctx.accounts.buyer.key(),
        amount: ctx.accounts.position.amount
    });
    Ok(())
}

/// No role gate on refunds: a buyer losing eligibility must still be able to
/// recover funds paid while they were eligible. The signer and PDA bind the recipient.
#[derive(Accounts)]
#[instruction(hub_id: u64)]
pub struct RefundPurchase<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [HUB_SEED, &hub_id.to_le_bytes()], bump = hub.bump)]
    pub hub: Box<Account<'info, Hub>>,
    #[account(mut, seeds = [POSITION_SEED, &hub_id.to_le_bytes(), buyer.key().as_ref()], bump = position.bump,
        has_one = buyer @ HubError::InvalidPosition, has_one = hub @ HubError::InvalidPosition)]
    pub position: Box<Account<'info, BuyerPosition>>,
    #[account(address = config.payment_mint @ HubError::InvalidMint)]
    pub payment_mint: Box<Account<'info, Mint>>,
    #[account(mut, seeds = [PAYMENT_VAULT_SEED, &hub_id.to_le_bytes()], bump, token::mint = payment_mint, token::authority = hub)]
    pub payment_vault: Box<Account<'info, TokenAccount>>,
    #[account(init_if_needed, payer = buyer, associated_token::mint = payment_mint, associated_token::authority = buyer)]
    pub buyer_token: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn refund_purchase_handler(ctx: Context<RefundPurchase>, hub_id: u64) -> Result<()> {
    let hub = &ctx.accounts.hub;
    let position = &ctx.accounts.position;
    require!(hub.status == HubStatus::Failed, HubError::InvalidStatus);
    require!(!position.settled, HubError::AlreadySettled);
    require!(position.paid > 0, HubError::EmptyPosition);
    let total_refunded = hub
        .total_refunded
        .checked_add(position.paid)
        .ok_or(HubError::Overflow)?;
    let id_bytes = hub_id.to_le_bytes();
    let bump = [hub.bump];
    let seeds: &[&[u8]] = &[HUB_SEED, &id_bytes, &bump];
    token::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.payment_vault.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                to: ctx.accounts.buyer_token.to_account_info(),
                authority: hub.to_account_info(),
            },
            &[seeds],
        ),
        position.paid,
        ctx.accounts.payment_mint.decimals,
    )?;
    ctx.accounts.position.settled = true;
    ctx.accounts.hub.total_refunded = total_refunded;
    emit!(PurchaseRefunded {
        hub_id,
        buyer: ctx.accounts.buyer.key(),
        amount: ctx.accounts.position.paid
    });
    Ok(())
}

#[event]
pub struct TokensPurchased {
    pub hub_id: u64,
    pub buyer: Pubkey,
    pub amount: u64,
    pub cost: u64,
}
#[event]
pub struct SaleFailed {
    pub hub_id: u64,
    pub total_paid: u64,
}
#[event]
pub struct TokensClaimed {
    pub hub_id: u64,
    pub buyer: Pubkey,
    pub amount: u64,
}
#[event]
pub struct PurchaseRefunded {
    pub hub_id: u64,
    pub buyer: Pubkey,
    pub amount: u64,
}
