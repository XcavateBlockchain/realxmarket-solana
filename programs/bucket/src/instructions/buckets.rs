use anchor_lang::prelude::*;

use crate::constants::{ADMIN_SEED, BUCKET_SEED, CONFIG_SEED, MANAGER_SEED, NAMESPACE_SEED};
use crate::error::BucketError;
use crate::state::{Admin, Bucket, BucketMetadata, Config, Manager, Namespace};

/// Creates a bucket under a namespace. Manager-only. The bucket starts
/// locked: writing needs a key, which only an admin can set.
#[derive(Accounts)]
#[instruction(metadata: BucketMetadata)]
pub struct CreateBucket<'info> {
    #[account(mut)]
    pub manager_signer: Signer<'info>,

    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

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

    #[account(
        init,
        payer = manager_signer,
        space = Bucket::space(&metadata),
        seeds = [BUCKET_SEED, &config.next_bucket_id.to_le_bytes()],
        bump,
    )]
    pub bucket: Account<'info, Bucket>,

    pub system_program: Program<'info, System>,
}

pub fn create_bucket_handler(ctx: Context<CreateBucket>, metadata: BucketMetadata) -> Result<()> {
    metadata.validate()?;
    let config = &mut ctx.accounts.config;
    let id = config.next_bucket_id;
    config.next_bucket_id = id.checked_add(1).ok_or(BucketError::Overflow)?;

    let namespace = &mut ctx.accounts.namespace;
    namespace.bucket_count = namespace
        .bucket_count
        .checked_add(1)
        .ok_or(BucketError::Overflow)?;

    let bucket = &mut ctx.accounts.bucket;
    bucket.id = id;
    bucket.namespace_id = namespace.id;
    bucket.name = metadata.name;
    bucket.category = metadata.category;
    bucket.properties = metadata.properties;
    bucket.created_at = Clock::get()?.unix_timestamp;
    bucket.encryption_key = None;
    bucket.next_message_id = 0;
    bucket.message_count = 0;
    bucket.admin_count = 0;
    bucket.contributor_count = 0;
    bucket.viewer_count = 0;
    bucket.tag_count = 0;
    bucket.rent_payer = ctx.accounts.manager_signer.key();
    bucket.bump = ctx.bumps.bucket;

    emit!(BucketCreated {
        namespace_id: namespace.id,
        bucket_id: id,
        creator: bucket.rent_payer,
    });
    Ok(())
}

/// The admin-only key controls: pause, resume and rotate.
#[derive(Accounts)]
pub struct AdminOnBucket<'info> {
    pub admin_signer: Signer<'info>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    /// Proves the signer is one of the bucket's admins.
    #[account(
        seeds = [ADMIN_SEED, &bucket.id.to_le_bytes(), admin_signer.key().as_ref()],
        bump = admin.bump,
    )]
    pub admin: Account<'info, Admin>,
}

pub fn pause_writing_handler(ctx: Context<AdminOnBucket>) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.encryption_key = None;
    emit!(BucketPaused {
        bucket_id: bucket.id,
        caller: ctx.accounts.admin_signer.key(),
    });
    Ok(())
}

pub fn resume_writing_handler(ctx: Context<AdminOnBucket>, encryption_key: [u8; 32]) -> Result<()> {
    set_key(ctx, encryption_key)
}

/// Unlike resume, rotating needs a key to be in place already.
pub fn rotate_key_handler(ctx: Context<AdminOnBucket>, encryption_key: [u8; 32]) -> Result<()> {
    require!(
        ctx.accounts.bucket.encryption_key.is_some(),
        BucketError::BucketLocked
    );
    set_key(ctx, encryption_key)
}

fn set_key(ctx: Context<AdminOnBucket>, encryption_key: [u8; 32]) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.encryption_key = Some(encryption_key);
    emit!(BucketWritableWithKey {
        bucket_id: bucket.id,
        encryption_key,
        caller: ctx.accounts.admin_signer.key(),
    });
    Ok(())
}

#[event]
pub struct BucketCreated {
    pub namespace_id: u64,
    pub bucket_id: u64,
    pub creator: Pubkey,
}

#[event]
pub struct BucketPaused {
    pub bucket_id: u64,
    pub caller: Pubkey,
}

/// Emitted on resume and on rotate alike.
#[event]
pub struct BucketWritableWithKey {
    pub bucket_id: u64,
    pub encryption_key: [u8; 32],
    pub caller: Pubkey,
}
