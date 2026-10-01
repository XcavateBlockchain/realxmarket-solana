use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::spl_token::instruction::AuthorityType;
use anchor_spl::token::{self, Mint, MintTo, SetAuthority, Token, TokenAccount, TransferChecked};
use xcavate_whitelist::state::{Role, RoleAccount};

use crate::constants::*;
use crate::error::HubError;
use crate::state::{Config, Hub, HubStatus};

#[derive(Accounts)]
#[instruction(hub_id: u64)]
pub struct ActivateHub<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [HUB_SEED, &hub_id.to_le_bytes()], bump = hub.bump, has_one = operator @ HubError::NotOperator)]
    pub hub: Box<Account<'info, Hub>>,
    #[account(
        seeds = [xcavate_whitelist::ROLE_SEED, operator.key().as_ref(), &[Role::RegionalOperator.seed_byte()]],
        bump = operator_role.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = operator_role.is_compliant() @ HubError::NotCompliant,
    )]
    pub operator_role: Box<Account<'info, RoleAccount>>,
    #[account(
        seeds = [regions::REGION_SEED, &hub.region_id.to_le_bytes()],
        bump = region.bump,
        seeds::program = regions::ID,
        constraint = region.owner == operator.key() @ HubError::NotRegionOwner,
    )]
    pub region: Box<Account<'info, regions::state::Region>>,
    #[account(address = config.xcav_mint @ HubError::InvalidMint)]
    pub xcav_mint: Box<Account<'info, Mint>>,
    #[account(address = config.payment_mint @ HubError::InvalidMint)]
    pub payment_mint: Box<Account<'info, Mint>>,
    #[account(mut, token::mint = xcav_mint, token::authority = operator)]
    pub operator_token: Box<Account<'info, TokenAccount>>,
    #[account(init, payer = operator, seeds = [HUB_MINT_SEED, &hub_id.to_le_bytes()], bump, mint::decimals = 0, mint::authority = hub)]
    pub hub_mint: Box<Account<'info, Mint>>,
    #[account(init, payer = operator, seeds = [TOKEN_VAULT_SEED, &hub_id.to_le_bytes()], bump, token::mint = hub_mint, token::authority = hub)]
    pub token_vault: Box<Account<'info, TokenAccount>>,
    #[account(init, payer = operator, seeds = [PAYMENT_VAULT_SEED, &hub_id.to_le_bytes()], bump, token::mint = payment_mint, token::authority = hub)]
    pub payment_vault: Box<Account<'info, TokenAccount>>,
    #[account(init, payer = operator, seeds = [BOND_VAULT_SEED, &hub_id.to_le_bytes()], bump, token::mint = xcav_mint, token::authority = hub)]
    pub bond_vault: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn activate_hub_handler(ctx: Context<ActivateHub>, hub_id: u64, max_bond: u64) -> Result<()> {
    let hub = &ctx.accounts.hub;
    require!(hub.status == HubStatus::Approved, HubError::InvalidStatus);
    require!(hub.bond_amount <= max_bond, HubError::BondTooHigh);
    let deadline = Clock::get()?
        .unix_timestamp
        .checked_add(hub.sale_duration)
        .ok_or(HubError::Overflow)?;
    token::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.operator_token.to_account_info(),
                mint: ctx.accounts.xcav_mint.to_account_info(),
                to: ctx.accounts.bond_vault.to_account_info(),
                authority: ctx.accounts.operator.to_account_info(),
            },
        ),
        hub.bond_amount,
        ctx.accounts.xcav_mint.decimals,
    )?;

    let id_bytes = hub_id.to_le_bytes();
    let bump = [hub.bump];
    let seeds: &[&[u8]] = &[HUB_SEED, &id_bytes, &bump];
    token::mint_to(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            MintTo {
                mint: ctx.accounts.hub_mint.to_account_info(),
                to: ctx.accounts.token_vault.to_account_info(),
                authority: hub.to_account_info(),
            },
            &[seeds],
        ),
        hub.token_supply,
    )?;
    // Supply is fixed before the sale opens. No operator or verifier can mint
    // extra tokens after buyers commit their payments.
    token::set_authority(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            SetAuthority {
                current_authority: hub.to_account_info(),
                account_or_mint: ctx.accounts.hub_mint.to_account_info(),
            },
            &[seeds],
        ),
        AuthorityType::MintTokens,
        None,
    )?;

    let hub = &mut ctx.accounts.hub;
    hub.token_mint = ctx.accounts.hub_mint.key();
    hub.sale_deadline = deadline;
    hub.status = HubStatus::Listed;
    emit!(HubActivated {
        hub_id,
        token_mint: hub.token_mint,
        bond: hub.bond_amount,
        sale_deadline: deadline
    });
    Ok(())
}

/// Failed-sale collateral returns to the recorded operator even if their role
/// was revoked. Eligibility gates must not strand an existing refund.
#[derive(Accounts)]
#[instruction(hub_id: u64)]
pub struct RefundBond<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [HUB_SEED, &hub_id.to_le_bytes()], bump = hub.bump, has_one = operator @ HubError::NotOperator)]
    pub hub: Box<Account<'info, Hub>>,
    #[account(address = config.xcav_mint @ HubError::InvalidMint)]
    pub xcav_mint: Box<Account<'info, Mint>>,
    #[account(mut, seeds = [BOND_VAULT_SEED, &hub_id.to_le_bytes()], bump, token::mint = xcav_mint, token::authority = hub)]
    pub bond_vault: Box<Account<'info, TokenAccount>>,
    #[account(init_if_needed, payer = operator, associated_token::mint = xcav_mint, associated_token::authority = operator)]
    pub operator_token: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn refund_bond_handler(ctx: Context<RefundBond>, hub_id: u64) -> Result<()> {
    let hub = &ctx.accounts.hub;
    require!(hub.status == HubStatus::Failed, HubError::InvalidStatus);
    require!(!hub.bond_refunded, HubError::BondAlreadyRefunded);
    let id_bytes = hub_id.to_le_bytes();
    let bump = [hub.bump];
    let seeds: &[&[u8]] = &[HUB_SEED, &id_bytes, &bump];
    token::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.bond_vault.to_account_info(),
                mint: ctx.accounts.xcav_mint.to_account_info(),
                to: ctx.accounts.operator_token.to_account_info(),
                authority: hub.to_account_info(),
            },
            &[seeds],
        ),
        hub.bond_amount,
        ctx.accounts.xcav_mint.decimals,
    )?;
    ctx.accounts.hub.bond_refunded = true;
    emit!(BondRefunded {
        hub_id,
        amount: ctx.accounts.hub.bond_amount
    });
    Ok(())
}

#[event]
pub struct HubActivated {
    pub hub_id: u64,
    pub token_mint: Pubkey,
    pub bond: u64,
    pub sale_deadline: i64,
}
#[event]
pub struct BondRefunded {
    pub hub_id: u64,
    pub amount: u64,
}
