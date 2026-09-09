//! Authority-only escape hatches: seat a manager without being one, and
//! tear records down. Removal goes leaf first: messages and tags, then the
//! bucket once its seats are gone, then managers, then the namespace.
//! Rent always goes back to whoever paid it.

use anchor_lang::prelude::*;

use crate::constants::{
    tag_seed, BUCKET_SEED, CONFIG_SEED, MANAGER_SEED, MESSAGE_SEED, NAMESPACE_SEED, TAG_SEED,
};
use crate::error::BucketError;
use crate::instructions::namespaces::{ManagerAdded, ManagerRemoved};
use crate::state::{Bucket, Config, Manager, Message, Namespace, Tag};

#[derive(Accounts)]
pub struct ForceAddManager<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ BucketError::NotAuthority,
    )]
    pub config: Account<'info, Config>,

    #[account(
        mut,
        seeds = [NAMESPACE_SEED, &namespace.id.to_le_bytes()],
        bump = namespace.bump,
    )]
    pub namespace: Account<'info, Namespace>,

    /// CHECK: the wallet being seated; used as PDA seed only.
    pub new_manager: UncheckedAccount<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + Manager::INIT_SPACE,
        seeds = [MANAGER_SEED, &namespace.id.to_le_bytes(), new_manager.key().as_ref()],
        bump,
    )]
    pub new_manager_account: Account<'info, Manager>,

    pub system_program: Program<'info, System>,
}

pub fn force_add_manager_handler(ctx: Context<ForceAddManager>) -> Result<()> {
    let namespace = &mut ctx.accounts.namespace;
    namespace.manager_count = namespace
        .manager_count
        .checked_add(1)
        .ok_or(BucketError::Overflow)?;

    let record = &mut ctx.accounts.new_manager_account;
    record.namespace_id = namespace.id;
    record.wallet = ctx.accounts.new_manager.key();
    record.rent_payer = ctx.accounts.authority.key();
    record.bump = ctx.bumps.new_manager_account;

    emit!(ManagerAdded {
        namespace_id: namespace.id,
        manager: record.wallet,
        caller: None,
    });
    Ok(())
}

/// No last-manager rule here: this is how a namespace gets emptied.
#[derive(Accounts)]
pub struct ForceRemoveManager<'info> {
    pub authority: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ BucketError::NotAuthority,
    )]
    pub config: Account<'info, Config>,

    #[account(
        mut,
        seeds = [NAMESPACE_SEED, &namespace.id.to_le_bytes()],
        bump = namespace.bump,
    )]
    pub namespace: Account<'info, Namespace>,

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

pub fn force_remove_manager_handler(ctx: Context<ForceRemoveManager>) -> Result<()> {
    let namespace = &mut ctx.accounts.namespace;
    namespace.manager_count -= 1;
    emit!(ManagerRemoved {
        namespace_id: namespace.id,
        manager: ctx.accounts.target.wallet,
        caller: None,
    });
    Ok(())
}

/// When the message carries a tag, that tag's account must be passed so its
/// count can drop.
#[derive(Accounts)]
pub struct ForceRemoveMessage<'info> {
    pub authority: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ BucketError::NotAuthority,
    )]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    /// CHECK: rent destination, the contributor who wrote the message.
    #[account(mut, address = message.contributor @ BucketError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(mut)]
    pub tag: Option<Account<'info, Tag>>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [MESSAGE_SEED, &bucket.id.to_le_bytes(), &message.id.to_le_bytes()],
        bump = message.bump,
    )]
    pub message: Account<'info, Message>,
}

pub fn force_remove_message_handler(ctx: Context<ForceRemoveMessage>) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    let message = &ctx.accounts.message;

    match (&message.tag, ctx.accounts.tag.as_mut()) {
        (None, None) => {}
        (Some(name), Some(tag)) => {
            require!(
                tag.bucket_id == bucket.id && tag.tag == *name,
                BucketError::WrongTag
            );
            tag.message_count -= 1;
        }
        _ => return err!(BucketError::WrongTag),
    }
    bucket.message_count -= 1;

    emit!(MessageRemoved {
        bucket_id: bucket.id,
        message_id: message.id,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct ForceRemoveTag<'info> {
    pub authority: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ BucketError::NotAuthority,
    )]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()], bump = bucket.bump)]
    pub bucket: Account<'info, Bucket>,

    /// CHECK: rent destination, fixed to whoever created the tag.
    #[account(mut, address = tag_account.rent_payer @ BucketError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [TAG_SEED, &bucket.id.to_le_bytes(), &tag_seed(&tag_account.tag)],
        bump = tag_account.bump,
        constraint = tag_account.message_count == 0 @ BucketError::TagInUse,
    )]
    pub tag_account: Account<'info, Tag>,
}

pub fn force_remove_tag_handler(ctx: Context<ForceRemoveTag>) -> Result<()> {
    let bucket = &mut ctx.accounts.bucket;
    bucket.tag_count -= 1;
    emit!(TagRemoved {
        bucket_id: bucket.id,
        tag: ctx.accounts.tag_account.tag.clone(),
    });
    Ok(())
}

/// Every message, seat and tag must be gone first.
#[derive(Accounts)]
pub struct ForceRemoveBucket<'info> {
    pub authority: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ BucketError::NotAuthority,
    )]
    pub config: Account<'info, Config>,

    #[account(
        mut,
        seeds = [NAMESPACE_SEED, &bucket.namespace_id.to_le_bytes()],
        bump = namespace.bump,
    )]
    pub namespace: Account<'info, Namespace>,

    /// CHECK: rent destination, fixed to the manager who created the bucket.
    #[account(mut, address = bucket.rent_payer @ BucketError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()],
        bump = bucket.bump,
        constraint = bucket.message_count == 0 @ BucketError::DanglingMessages,
        constraint = bucket.admin_count == 0 @ BucketError::DanglingAdmins,
        constraint = bucket.contributor_count == 0 @ BucketError::DanglingContributors,
        constraint = bucket.viewer_count == 0 @ BucketError::DanglingViewers,
        constraint = bucket.tag_count == 0 @ BucketError::DanglingTags,
    )]
    pub bucket: Account<'info, Bucket>,
}

pub fn force_remove_bucket_handler(ctx: Context<ForceRemoveBucket>) -> Result<()> {
    let namespace = &mut ctx.accounts.namespace;
    namespace.bucket_count -= 1;
    emit!(BucketRemoved {
        namespace_id: namespace.id,
        bucket_id: ctx.accounts.bucket.id,
    });
    Ok(())
}

/// Every bucket and manager must be gone first.
#[derive(Accounts)]
pub struct ForceRemoveNamespace<'info> {
    pub authority: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ BucketError::NotAuthority,
    )]
    pub config: Account<'info, Config>,

    /// CHECK: rent destination, fixed to the namespace's creator.
    #[account(mut, address = namespace.rent_payer @ BucketError::WrongRentPayer)]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [NAMESPACE_SEED, &namespace.id.to_le_bytes()],
        bump = namespace.bump,
        constraint = namespace.bucket_count == 0 @ BucketError::DanglingBuckets,
        constraint = namespace.manager_count == 0 @ BucketError::DanglingManagers,
    )]
    pub namespace: Account<'info, Namespace>,
}

pub fn force_remove_namespace_handler(ctx: Context<ForceRemoveNamespace>) -> Result<()> {
    emit!(NamespaceRemoved {
        namespace_id: ctx.accounts.namespace.id,
    });
    Ok(())
}

#[event]
pub struct MessageRemoved {
    pub bucket_id: u64,
    pub message_id: u64,
}

#[event]
pub struct TagRemoved {
    pub bucket_id: u64,
    pub tag: String,
}

#[event]
pub struct BucketRemoved {
    pub namespace_id: u64,
    pub bucket_id: u64,
}

#[event]
pub struct NamespaceRemoved {
    pub namespace_id: u64,
}
