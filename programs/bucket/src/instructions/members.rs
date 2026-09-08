use anchor_lang::prelude::*;

use crate::constants::{
    ADMIN_SEED, BUCKET_SEED, CONTRIBUTOR_SEED, MANAGER_SEED, NAMESPACE_SEED, VIEWER_SEED,
};
use crate::error::BucketError;
use crate::instructions::namespaces::{ManagerAdded, ManagerRemoved};
use crate::state::{Admin, Bucket, Contributor, Manager, Namespace, Viewer};

// --- managers (seated by managers) ---

#[derive(Accounts)]
pub struct AddManager<'info> {
    #[account(mut)]
    pub manager_signer: Signer<'info>,

    #[account(
        mut,
        seeds = [NAMESPACE_SEED, &namespace.id.to_le_bytes()],
        bump = namespace.bump,
    )]
    pub namespace: Account<'info, Namespace>,

    /// Proves the signer manages this namespace.
    #[account(
        seeds = [MANAGER_SEED, &namespace.id.to_le_bytes(), manager_signer.key().as_ref()],
        bump = manager.bump,
    )]
    pub manager: Account<'info, Manager>,

    /// CHECK: the wallet being seated; used as PDA seed only.
    pub new_manager: UncheckedAccount<'info>,

    #[account(
        init,
        payer = manager_signer,
        space = 8 + Manager::INIT_SPACE,
        seeds = [MANAGER_SEED, &namespace.id.to_le_bytes(), new_manager.key().as_ref()],
        bump,
    )]
    pub new_manager_account: Account<'info, Manager>,

    pub system_program: Program<'info, System>,
}

pub fn add_manager_handler(ctx: Context<AddManager>) -> Result<()> {
    let namespace = &mut ctx.accounts.namespace;
    namespace.manager_count = namespace
        .manager_count
        .checked_add(1)
        .ok_or(BucketError::Overflow)?;

    let caller = ctx.accounts.manager_signer.key();
    let record = &mut ctx.accounts.new_manager_account;
    record.namespace_id = namespace.id;
    record.wallet = ctx.accounts.new_manager.key();
    record.rent_payer = caller;
    record.bump = ctx.bumps.new_manager_account;

    emit!(ManagerAdded {
        namespace_id: namespace.id,
        manager: record.wallet,
        caller: Some(caller),
    });
    Ok(())
}

/// A manager may unseat any manager, themselves included, as long as one
/// remains. Rent goes back to whoever paid at seating.
#[derive(Accounts)]
pub struct RemoveManager<'info> {
    pub manager_signer: Signer<'info>,

    #[account(
        mut,
        seeds = [NAMESPACE_SEED, &namespace.id.to_le_bytes()],
        bump = namespace.bump,
    )]
    pub namespace: Account<'info, Namespace>,

    #[account(
        seeds = [MANAGER_SEED, &namespace.id.to_le_bytes(), manager_signer.key().as_ref()],
        bump = manager.bump,
    )]
    pub manager: Account<'info, Manager>,

    /// CHECK: rent destination, fixed to whoever paid at seating.
    #[account(mut, address = target.rent_payer @ BucketError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [MANAGER_SEED, &namespace.id.to_le_bytes(), target.wallet.as_ref()],
        bump = target.bump,
    )]
    pub target: Account<'info, Manager>,
}

pub fn remove_manager_handler(ctx: Context<RemoveManager>) -> Result<()> {
    let namespace = &mut ctx.accounts.namespace;
    require!(namespace.manager_count > 1, BucketError::LastManager);
    namespace.manager_count -= 1;

    emit!(ManagerRemoved {
        namespace_id: namespace.id,
        manager: ctx.accounts.target.wallet,
        caller: Some(ctx.accounts.manager_signer.key()),
    });
    Ok(())
}

// --- admins (seated by the namespace's managers) ---

#[derive(Accounts)]
pub struct AddAdmin<'info> {
    #[account(mut)]
    pub manager_signer: Signer<'info>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    /// Proves the signer manages the bucket's namespace.
    #[account(
        seeds = [MANAGER_SEED, &bucket.namespace_id.to_le_bytes(), manager_signer.key().as_ref()],
        bump = manager.bump,
    )]
    pub manager: Account<'info, Manager>,

    /// CHECK: the wallet being seated; used as PDA seed only.
    pub new_admin: UncheckedAccount<'info>,

    #[account(
        init,
        payer = manager_signer,
        space = 8 + Admin::INIT_SPACE,
        seeds = [ADMIN_SEED, &bucket.id.to_le_bytes(), new_admin.key().as_ref()],
        bump,
    )]
    pub admin: Account<'info, Admin>,

    pub system_program: Program<'info, System>,
}

pub fn add_admin_handler(ctx: Context<AddAdmin>) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.admin_count = bucket
        .admin_count
        .checked_add(1)
        .ok_or(BucketError::Overflow)?;

    let caller = ctx.accounts.manager_signer.key();
    let admin = &mut ctx.accounts.admin;
    admin.bucket_id = bucket.id;
    admin.wallet = ctx.accounts.new_admin.key();
    admin.rent_payer = caller;
    admin.bump = ctx.bumps.admin;

    emit!(AdminAdded {
        bucket_id: bucket.id,
        admin: admin.wallet,
        caller,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct RemoveAdmin<'info> {
    pub manager_signer: Signer<'info>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    #[account(
        seeds = [MANAGER_SEED, &bucket.namespace_id.to_le_bytes(), manager_signer.key().as_ref()],
        bump = manager.bump,
    )]
    pub manager: Account<'info, Manager>,

    /// CHECK: rent destination, fixed to whoever paid at seating.
    #[account(mut, address = admin.rent_payer @ BucketError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [ADMIN_SEED, &bucket.id.to_le_bytes(), admin.wallet.as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,
}

pub fn remove_admin_handler(ctx: Context<RemoveAdmin>) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.admin_count -= 1;
    emit!(AdminRemoved {
        bucket_id: bucket.id,
        admin: ctx.accounts.admin.wallet,
        caller: ctx.accounts.manager_signer.key(),
    });
    Ok(())
}

// --- contributors and viewers (seated by the bucket's admins) ---

#[derive(Accounts)]
pub struct AddContributor<'info> {
    #[account(mut)]
    pub admin_signer: Signer<'info>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    /// Proves the signer is one of the bucket's admins.
    #[account(
        seeds = [ADMIN_SEED, &bucket.id.to_le_bytes(), admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,

    /// CHECK: the wallet being seated; used as PDA seed only.
    pub new_contributor: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin_signer,
        space = 8 + Contributor::INIT_SPACE,
        seeds = [CONTRIBUTOR_SEED, &bucket.id.to_le_bytes(), new_contributor.key().as_ref()],
        bump,
    )]
    pub contributor: Account<'info, Contributor>,

    pub system_program: Program<'info, System>,
}

pub fn add_contributor_handler(ctx: Context<AddContributor>) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.contributor_count = bucket
        .contributor_count
        .checked_add(1)
        .ok_or(BucketError::Overflow)?;

    let caller = ctx.accounts.admin_signer.key();
    let contributor = &mut ctx.accounts.contributor;
    contributor.bucket_id = bucket.id;
    contributor.wallet = ctx.accounts.new_contributor.key();
    contributor.rent_payer = caller;
    contributor.bump = ctx.bumps.contributor;

    emit!(ContributorAdded {
        bucket_id: bucket.id,
        contributor: contributor.wallet,
        caller,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct RemoveContributor<'info> {
    pub admin_signer: Signer<'info>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    #[account(
        seeds = [ADMIN_SEED, &bucket.id.to_le_bytes(), admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,

    /// CHECK: rent destination, fixed to whoever paid at seating.
    #[account(mut, address = contributor.rent_payer @ BucketError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [CONTRIBUTOR_SEED, &bucket.id.to_le_bytes(), contributor.wallet.as_ref()],
        bump = contributor.bump,
    )]
    pub contributor: Account<'info, Contributor>,
}

pub fn remove_contributor_handler(ctx: Context<RemoveContributor>) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.contributor_count -= 1;
    emit!(ContributorRemoved {
        bucket_id: bucket.id,
        contributor: ctx.accounts.contributor.wallet,
        caller: ctx.accounts.admin_signer.key(),
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(viewer_key: [u8; 32])]
pub struct AddViewer<'info> {
    #[account(mut)]
    pub admin_signer: Signer<'info>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    #[account(
        seeds = [ADMIN_SEED, &bucket.id.to_le_bytes(), admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,

    #[account(
        init,
        payer = admin_signer,
        space = 8 + Viewer::INIT_SPACE,
        seeds = [VIEWER_SEED, &bucket.id.to_le_bytes(), &viewer_key],
        bump,
    )]
    pub viewer: Account<'info, Viewer>,

    pub system_program: Program<'info, System>,
}

pub fn add_viewer_handler(ctx: Context<AddViewer>, viewer_key: [u8; 32]) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.viewer_count = bucket
        .viewer_count
        .checked_add(1)
        .ok_or(BucketError::Overflow)?;

    let caller = ctx.accounts.admin_signer.key();
    let viewer = &mut ctx.accounts.viewer;
    viewer.bucket_id = bucket.id;
    viewer.key = viewer_key;
    viewer.rent_payer = caller;
    viewer.bump = ctx.bumps.viewer;

    emit!(ViewerAdded {
        bucket_id: bucket.id,
        viewer: viewer_key,
        caller,
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(viewer_key: [u8; 32])]
pub struct RemoveViewer<'info> {
    pub admin_signer: Signer<'info>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    #[account(
        seeds = [ADMIN_SEED, &bucket.id.to_le_bytes(), admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,

    /// CHECK: rent destination, fixed to whoever paid at seating.
    #[account(mut, address = viewer.rent_payer @ BucketError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [VIEWER_SEED, &bucket.id.to_le_bytes(), &viewer_key],
        bump = viewer.bump,
    )]
    pub viewer: Account<'info, Viewer>,
}

pub fn remove_viewer_handler(ctx: Context<RemoveViewer>, viewer_key: [u8; 32]) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.viewer_count -= 1;
    emit!(ViewerRemoved {
        bucket_id: bucket.id,
        viewer: viewer_key,
        caller: ctx.accounts.admin_signer.key(),
    });
    Ok(())
}

#[event]
pub struct AdminAdded {
    pub bucket_id: u64,
    pub admin: Pubkey,
    pub caller: Pubkey,
}

#[event]
pub struct AdminRemoved {
    pub bucket_id: u64,
    pub admin: Pubkey,
    pub caller: Pubkey,
}

#[event]
pub struct ContributorAdded {
    pub bucket_id: u64,
    pub contributor: Pubkey,
    pub caller: Pubkey,
}

#[event]
pub struct ContributorRemoved {
    pub bucket_id: u64,
    pub contributor: Pubkey,
    pub caller: Pubkey,
}

#[event]
pub struct ViewerAdded {
    pub bucket_id: u64,
    pub viewer: [u8; 32],
    pub caller: Pubkey,
}

#[event]
pub struct ViewerRemoved {
    pub bucket_id: u64,
    pub viewer: [u8; 32],
    pub caller: Pubkey,
}
