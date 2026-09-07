use anchor_lang::prelude::*;

use crate::constants::CONFIG_SEED;
use crate::error::BucketError;
use crate::state::Config;

/// Creates the singleton config. Only the program's upgrade authority can
/// call this, so the config can't be claimed by a front-runner between
/// deploy and initialization.
#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    /// This program's executable account, tying `program_data` to it.
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ BucketError::NotUpgradeAuthority)]
    pub program: Program<'info, crate::program::Bucket>,

    /// The program's upgrade authority must be the initializing signer.
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ BucketError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,

    #[account(
        init,
        payer = authority,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Account<'info, Config>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitializeConfig>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.authority = ctx.accounts.authority.key();
    config.pending_authority = None;
    config.next_namespace_id = 0;
    config.next_bucket_id = 0;
    config.bump = ctx.bumps.config;

    emit!(ConfigInitialized {
        authority: config.authority
    });
    Ok(())
}

/// Proposes a new authority. The handover only completes when the proposed
/// key signs `accept_authority`, so a typo'd address can't lose control.
/// Proposing again overwrites any earlier proposal.
#[derive(Accounts)]
pub struct UpdateAuthority<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ BucketError::NotAuthority,
    )]
    pub config: Account<'info, Config>,
}

pub fn update_authority_handler(
    ctx: Context<UpdateAuthority>,
    new_authority: Pubkey,
) -> Result<()> {
    require!(
        new_authority != Pubkey::default(),
        BucketError::InvalidAuthority
    );
    let config = &mut ctx.accounts.config;
    config.pending_authority = Some(new_authority);

    emit!(AuthorityUpdateProposed {
        authority: config.authority,
        pending_authority: new_authority,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct AcceptAuthority<'info> {
    pub new_authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = config.pending_authority == Some(new_authority.key()) @ BucketError::NotPendingAuthority,
    )]
    pub config: Account<'info, Config>,
}

pub fn accept_authority_handler(ctx: Context<AcceptAuthority>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    let old_authority = config.authority;
    config.authority = ctx.accounts.new_authority.key();
    config.pending_authority = None;

    emit!(AuthorityUpdated {
        old_authority,
        new_authority: config.authority,
    });
    Ok(())
}

#[event]
pub struct ConfigInitialized {
    pub authority: Pubkey,
}

#[event]
pub struct AuthorityUpdateProposed {
    pub authority: Pubkey,
    pub pending_authority: Pubkey,
}

#[event]
pub struct AuthorityUpdated {
    pub old_authority: Pubkey,
    pub new_authority: Pubkey,
}
