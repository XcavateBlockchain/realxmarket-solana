use anchor_lang::prelude::*;
use anchor_spl::associated_token::{
    create_idempotent, get_associated_token_address_with_program_id, AssociatedToken,
    Create as CreateAta,
};
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{
    CHECKPOINT_SEED, CPI_AUTH_SEED, INCOME_SEED, INCOME_VAULT_SEED, LETTING_SEED,
};
use crate::error::PropertyError;
use crate::state::{
    CheckpointEntry, IncomeCheckpoint, IncomeStream, PropertyIncome, PropertyLetting,
    MAX_INCOME_STREAMS,
};

use marketplace::state::{Config as MarketConfig, PropertyAsset, ShareHolding};
use xcavate_whitelist::state::{Role, RoleAccount};

/// Bank everything the holder accrued on one stream since their last
/// checkpoint into `pending`, then move the checkpoint up to date.
fn bank(entry: &mut CheckpointEntry, stream: &IncomeStream, shares: u32) -> Result<()> {
    let delta = stream
        .per_share
        .checked_sub(entry.per_share)
        .ok_or(PropertyError::Overflow)?;
    let accrued: u64 = delta
        .checked_mul(shares as u128)
        .ok_or(PropertyError::Overflow)?
        .try_into()
        .map_err(|_| PropertyError::Overflow)?;
    entry.pending = entry
        .pending
        .checked_add(accrued)
        .ok_or(PropertyError::Overflow)?;
    entry.per_share = stream.per_share;
    Ok(())
}

/// Grow the checkpoint's entry list to cover every stream. Streams are
/// append-only, so a zeroed entry means "held since before this stream
/// opened" and correctly earns from its start.
fn sync_entries(entries: &mut Vec<CheckpointEntry>, streams: usize) {
    while entries.len() < streams {
        entries.push(CheckpointEntry::default());
    }
}

/// The assigned letting agent pays rental income into the property's income
/// vault. Each payment mint keeps its own per-share stream; the leftover
/// below one unit per share carries into the next distribution.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct DistributeIncome<'info> {
    pub agent: Signer<'info>,

    /// Whoever fronts the rent for accounts created along the way; the agent
    /// on the default path.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The caller's LettingAgent role, owned by the roles program.
    #[account(
        seeds = [
            xcavate_whitelist::ROLE_SEED,
            agent.key().as_ref(),
            &[Role::LettingAgent.seed_byte()],
        ],
        bump = agent_role.bump,
        seeds::program = xcavate_whitelist::ID,
    )]
    pub agent_role: Box<Account<'info, RoleAccount>>,

    /// The property's letting seat; only its assigned agent distributes.
    #[account(
        seeds = [LETTING_SEED, &asset_id.to_le_bytes()],
        bump = letting.bump,
    )]
    pub letting: Box<Account<'info, PropertyLetting>>,

    /// The property, owned by the marketplace; supplies the share supply the
    /// income is split over.
    #[account(
        seeds = [marketplace::PROPERTY_SEED, &asset_id.to_le_bytes()],
        bump = property.bump,
        seeds::program = marketplace::ID,
    )]
    pub property: Box<Account<'info, PropertyAsset>>,

    /// CHECK: the marketplace config, seeds-pinned here and deserialized in
    /// the handler. Typed here it would collide with this program's own
    /// `Config` in the IDL, since both resolve to the same account name.
    /// Income arrives in its accepted payment mints, which its mint guard
    /// already vetted.
    #[account(
        seeds = [marketplace::CONFIG_SEED],
        bump,
        seeds::program = marketplace::ID,
    )]
    pub market_config: UncheckedAccount<'info>,

    /// The property's income ledger; the first distribution creates it.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + PropertyIncome::INIT_SPACE,
        seeds = [INCOME_SEED, &asset_id.to_le_bytes()],
        bump,
    )]
    pub income: Box<Account<'info, PropertyIncome>>,

    pub payment_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The agent's token account the income is paid from.
    #[account(
        mut,
        constraint = agent_payment.mint == payment_mint.key()
            @ PropertyError::PaymentAccountMismatch,
    )]
    pub agent_payment: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: the income vault authority; a bare PDA owning the vault's
    /// token accounts.
    #[account(seeds = [INCOME_VAULT_SEED, &asset_id.to_le_bytes()], bump)]
    pub income_vault: UncheckedAccount<'info>,

    /// CHECK: the vault's associated account for the payment mint; created
    /// idempotently, so the ATA program verifies the derivation.
    #[account(mut)]
    pub vault_payment_account: UncheckedAccount<'info>,

    pub payment_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn distribute_income_handler<'info>(
    ctx: Context<'info, DistributeIncome<'info>>,
    asset_id: u64,
    amount: u64,
) -> Result<()> {
    require!(amount > 0, PropertyError::ZeroDistribution);
    require!(
        ctx.accounts.property.finalized,
        PropertyError::PropertyNotFinalized
    );
    require!(
        ctx.accounts.letting.agent == ctx.accounts.agent.key(),
        PropertyError::NotAssignedAgent
    );
    let mint_key = ctx.accounts.payment_mint.key();
    let market_config: Account<MarketConfig> = Account::try_from(&ctx.accounts.market_config)?;
    require!(
        market_config.accepted_payment_mints.contains(&mint_key),
        PropertyError::PaymentMintNotAccepted
    );

    let income = &mut ctx.accounts.income;
    if income.rent_payer == Pubkey::default() {
        income.asset_id = asset_id;
        income.rent_payer = ctx.accounts.payer.key();
        income.bump = ctx.bumps.income;
    }
    let index = match income.streams.iter().position(|s| s.mint == mint_key) {
        Some(index) => index,
        None => {
            require!(
                income.streams.len() < MAX_INCOME_STREAMS,
                PropertyError::TooManyIncomeStreams
            );
            income.streams.push(IncomeStream {
                mint: mint_key,
                per_share: 0,
                dust: 0,
            });
            income.streams.len() - 1
        }
    };

    create_idempotent(CpiContext::new(
        ctx.accounts.associated_token_program.key(),
        CreateAta {
            payer: ctx.accounts.payer.to_account_info(),
            associated_token: ctx.accounts.vault_payment_account.to_account_info(),
            authority: ctx.accounts.income_vault.to_account_info(),
            mint: ctx.accounts.payment_mint.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            token_program: ctx.accounts.payment_token_program.to_account_info(),
        },
    ))?;
    transfer_checked(
        CpiContext::new(
            ctx.accounts.payment_token_program.key(),
            TransferChecked {
                from: ctx.accounts.agent_payment.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                to: ctx.accounts.vault_payment_account.to_account_info(),
                authority: ctx.accounts.agent.to_account_info(),
            },
        ),
        amount,
        ctx.accounts.payment_mint.decimals,
    )?;

    let supply = ctx.accounts.property.share_amount as u64;
    let stream = &mut income.streams[index];
    let total = amount
        .checked_add(stream.dust)
        .ok_or(PropertyError::Overflow)?;
    let gain = total.checked_div(supply).ok_or(PropertyError::Overflow)?;
    stream.dust = total.checked_rem(supply).ok_or(PropertyError::Overflow)?;
    stream.per_share = stream
        .per_share
        .checked_add(gain as u128)
        .ok_or(PropertyError::Overflow)?;

    emit!(IncomeDistributed {
        asset_id,
        mint: mint_key,
        amount,
        per_share_gain: gain,
    });
    Ok(())
}

/// A holder pulls what one stream owes them: income banked by earlier
/// settles plus everything accrued since their last checkpoint. Not
/// role-gated: the money is already theirs.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct ClaimIncome<'info> {
    pub holder: Signer<'info>,

    /// Whoever fronts the checkpoint's rent: the sponsor on the default
    /// path, or any willing wallet. The record remembers who to refund.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [INCOME_SEED, &asset_id.to_le_bytes()],
        bump = income.bump,
    )]
    pub income: Box<Account<'info, PropertyIncome>>,

    /// The holder's claim state; the first claim creates it.
    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + IncomeCheckpoint::INIT_SPACE,
        seeds = [CHECKPOINT_SEED, &asset_id.to_le_bytes(), holder.key().as_ref()],
        bump,
    )]
    pub checkpoint: Box<Account<'info, IncomeCheckpoint>>,

    /// CHECK: the holder's marketplace share ledger; the handler derives the
    /// expected address and inspects it directly. A ledger already closed by
    /// a teardown reads as zero shares, leaving banked income claimable.
    pub holding: UncheckedAccount<'info>,

    pub payment_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: the income vault authority; a bare PDA owning the vault's
    /// token accounts.
    #[account(seeds = [INCOME_VAULT_SEED, &asset_id.to_le_bytes()], bump)]
    pub income_vault: UncheckedAccount<'info>,

    /// CHECK: the vault's associated account for the payment mint, pinned to
    /// its derivation.
    #[account(
        mut,
        address = get_associated_token_address_with_program_id(
            &income_vault.key(),
            &payment_mint.key(),
            &payment_token_program.key(),
        ),
    )]
    pub vault_payment_account: UncheckedAccount<'info>,

    /// Where the income goes: any token account of the holder's carrying the
    /// paid mint, so a closed original can't strand the claim.
    #[account(
        mut,
        constraint = holder_payment.mint == payment_mint.key()
            && holder_payment.owner == holder.key()
            @ PropertyError::PaymentAccountMismatch,
    )]
    pub holder_payment: Box<InterfaceAccount<'info, TokenAccount>>,

    pub payment_token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn claim_income_handler<'info>(
    ctx: Context<'info, ClaimIncome<'info>>,
    asset_id: u64,
) -> Result<()> {
    let holder = ctx.accounts.holder.key();
    let expected = Pubkey::find_program_address(
        &[
            marketplace::SHARE_SEED,
            &asset_id.to_le_bytes(),
            holder.as_ref(),
        ],
        &marketplace::ID,
    )
    .0;
    require!(
        ctx.accounts.holding.key() == expected,
        PropertyError::HoldingMismatch
    );
    let shares = if ctx.accounts.holding.data_is_empty() {
        0
    } else {
        let holding: Account<ShareHolding> = Account::try_from(&ctx.accounts.holding)?;
        holding.amount
    };

    let income = &ctx.accounts.income;
    let mint_key = ctx.accounts.payment_mint.key();
    let index = income
        .streams
        .iter()
        .position(|s| s.mint == mint_key)
        .ok_or(PropertyError::UnknownIncomeStream)?;

    let checkpoint = &mut ctx.accounts.checkpoint;
    if checkpoint.rent_payer == Pubkey::default() {
        checkpoint.asset_id = asset_id;
        checkpoint.owner = holder;
        checkpoint.rent_payer = ctx.accounts.payer.key();
        checkpoint.bump = ctx.bumps.checkpoint;
    }
    sync_entries(&mut checkpoint.entries, income.streams.len());
    bank(
        &mut checkpoint.entries[index],
        &income.streams[index],
        shares,
    )?;
    let owed = checkpoint.entries[index].pending;
    require!(owed > 0, PropertyError::NothingToClaim);
    checkpoint.entries[index].pending = 0;

    let id_bytes = asset_id.to_le_bytes();
    let seeds: &[&[u8]] = &[INCOME_VAULT_SEED, &id_bytes, &[ctx.bumps.income_vault]];
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.payment_token_program.key(),
            TransferChecked {
                from: ctx.accounts.vault_payment_account.to_account_info(),
                mint: ctx.accounts.payment_mint.to_account_info(),
                to: ctx.accounts.holder_payment.to_account_info(),
                authority: ctx.accounts.income_vault.to_account_info(),
            },
            &[seeds],
        ),
        owed,
        ctx.accounts.payment_mint.decimals,
    )?;

    emit!(IncomeClaimed {
        asset_id,
        mint: mint_key,
        owner: holder,
        amount: owed,
    });
    Ok(())
}

/// Checkpoint a holder across every stream before their share balance
/// changes, so a transfer can't move accrued income with it. Only the
/// marketplace program can produce the gate signature; it calls this ahead
/// of any share movement once secondary transfers exist. No tokens move:
/// the accrual is banked into `pending` for a later claim.
#[derive(Accounts)]
#[instruction(asset_id: u64, owner: Pubkey)]
pub struct SettleIncome<'info> {
    /// The marketplace's CPI signer PDA; carries no data, only proves the
    /// caller.
    #[account(seeds = [CPI_AUTH_SEED], bump, seeds::program = marketplace::ID)]
    pub marketplace_signer: Signer<'info>,

    /// Whoever fronts the checkpoint's rent, forwarded through the CPI.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [INCOME_SEED, &asset_id.to_le_bytes()],
        bump = income.bump,
    )]
    pub income: Box<Account<'info, PropertyIncome>>,

    /// The holder's share ledger, owned by the marketplace; settles at its
    /// balance before the transfer touches it.
    #[account(
        seeds = [marketplace::SHARE_SEED, &asset_id.to_le_bytes(), owner.as_ref()],
        bump = holding.bump,
        seeds::program = marketplace::ID,
    )]
    pub holding: Box<Account<'info, ShareHolding>>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + IncomeCheckpoint::INIT_SPACE,
        seeds = [CHECKPOINT_SEED, &asset_id.to_le_bytes(), owner.as_ref()],
        bump,
    )]
    pub checkpoint: Box<Account<'info, IncomeCheckpoint>>,

    pub system_program: Program<'info, System>,
}

pub fn settle_income_handler(
    ctx: Context<SettleIncome>,
    asset_id: u64,
    owner: Pubkey,
) -> Result<()> {
    let income = &ctx.accounts.income;
    let shares = ctx.accounts.holding.amount;
    let checkpoint = &mut ctx.accounts.checkpoint;
    if checkpoint.rent_payer == Pubkey::default() {
        checkpoint.asset_id = asset_id;
        checkpoint.owner = owner;
        checkpoint.rent_payer = ctx.accounts.payer.key();
        checkpoint.bump = ctx.bumps.checkpoint;
    }
    sync_entries(&mut checkpoint.entries, income.streams.len());
    for (entry, stream) in checkpoint.entries.iter_mut().zip(&income.streams) {
        bank(entry, stream, shares)?;
    }

    emit!(IncomeSettled {
        asset_id,
        owner,
        shares,
    });
    Ok(())
}

/// Reclaim a checkpoint's rent once it can owe nothing more: no banked
/// income and no shares left to accrue any. Not role-gated: this is a pure
/// exit.
#[derive(Accounts)]
#[instruction(asset_id: u64)]
pub struct CloseIncomeCheckpoint<'info> {
    pub holder: Signer<'info>,

    /// CHECK: the wallet that fronted the checkpoint's rent; gets it back as
    /// the checkpoint closes.
    #[account(mut, address = checkpoint.rent_payer @ PropertyError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [CHECKPOINT_SEED, &asset_id.to_le_bytes(), holder.key().as_ref()],
        bump = checkpoint.bump,
    )]
    pub checkpoint: Box<Account<'info, IncomeCheckpoint>>,

    /// CHECK: the holder's marketplace share ledger; the handler derives the
    /// expected address and inspects it directly. Must be closed or empty:
    /// a live balance would re-earn from zero if the checkpoint were
    /// recreated later.
    pub holding: UncheckedAccount<'info>,
}

pub fn close_income_checkpoint_handler<'info>(
    ctx: Context<'info, CloseIncomeCheckpoint<'info>>,
    asset_id: u64,
) -> Result<()> {
    let holder = ctx.accounts.holder.key();
    let expected = Pubkey::find_program_address(
        &[
            marketplace::SHARE_SEED,
            &asset_id.to_le_bytes(),
            holder.as_ref(),
        ],
        &marketplace::ID,
    )
    .0;
    require!(
        ctx.accounts.holding.key() == expected,
        PropertyError::HoldingMismatch
    );
    if !ctx.accounts.holding.data_is_empty() {
        let holding: Account<ShareHolding> = Account::try_from(&ctx.accounts.holding)?;
        require!(holding.amount == 0, PropertyError::SharesStillHeld);
    }
    require!(
        ctx.accounts
            .checkpoint
            .entries
            .iter()
            .all(|e| e.pending == 0),
        PropertyError::PendingIncome
    );

    emit!(IncomeCheckpointClosed {
        asset_id,
        owner: holder,
    });
    Ok(())
}

#[event]
pub struct IncomeDistributed {
    pub asset_id: u64,
    pub mint: Pubkey,
    pub amount: u64,
    pub per_share_gain: u64,
}

#[event]
pub struct IncomeClaimed {
    pub asset_id: u64,
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
}

#[event]
pub struct IncomeSettled {
    pub asset_id: u64,
    pub owner: Pubkey,
    pub shares: u32,
}

#[event]
pub struct IncomeCheckpointClosed {
    pub asset_id: u64,
    pub owner: Pubkey,
}
