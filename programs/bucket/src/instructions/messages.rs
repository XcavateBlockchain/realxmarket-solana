use anchor_lang::prelude::*;

use crate::constants::{
    tag_seed, ADMIN_SEED, BUCKET_SEED, CONTRIBUTOR_SEED, MESSAGE_SEED, TAG_SEED,
};
use crate::error::BucketError;
use crate::state::{Admin, Bucket, Contributor, Message, MessageInput, Tag};

/// Creates a tag messages in this bucket may carry. Admin-only.
#[derive(Accounts)]
#[instruction(tag: String)]
pub struct CreateTag<'info> {
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

    #[account(
        init,
        payer = admin_signer,
        space = Tag::space(&tag),
        seeds = [TAG_SEED, &bucket.id.to_le_bytes(), &tag_seed(&tag)],
        bump,
    )]
    pub tag_account: Account<'info, Tag>,

    pub system_program: Program<'info, System>,
}

pub fn create_tag_handler(ctx: Context<CreateTag>, tag: String) -> Result<()> {
    Tag::validate(&tag)?;
    let bucket = &mut ctx.accounts.bucket;
    bucket.tag_count = bucket
        .tag_count
        .checked_add(1)
        .ok_or(BucketError::Overflow)?;

    let creator = ctx.accounts.admin_signer.key();
    let record = &mut ctx.accounts.tag_account;
    record.bucket_id = bucket.id;
    record.tag = tag;
    record.message_count = 0;
    record.rent_payer = creator;
    record.bump = ctx.bumps.tag_account;

    emit!(TagCreated {
        bucket_id: bucket.id,
        tag: record.tag.clone(),
        creator,
    });
    Ok(())
}

/// Writes a message. Contributor-only, and the bucket must hold a key. When
/// the message names a tag, that tag's account must be passed.
#[derive(Accounts)]
#[instruction(input: MessageInput)]
pub struct WriteMessage<'info> {
    #[account(mut)]
    pub contributor_signer: Signer<'info>,

    #[account(
        mut,
        seeds = [BUCKET_SEED, &bucket.id.to_le_bytes()],
        bump = bucket.bump,
        constraint = bucket.encryption_key.is_some() @ BucketError::BucketLocked,
    )]
    pub bucket: Account<'info, Bucket>,

    /// Proves the signer is one of the bucket's contributors.
    #[account(
        seeds = [CONTRIBUTOR_SEED, &bucket.id.to_le_bytes(), contributor_signer.key().as_ref()],
        bump = contributor.bump,
    )]
    pub contributor: Account<'info, Contributor>,

    /// Matched against the input's tag in the handler.
    #[account(mut)]
    pub tag: Option<Account<'info, Tag>>,

    #[account(
        init,
        payer = contributor_signer,
        space = Message::space(&input),
        seeds = [MESSAGE_SEED, &bucket.id.to_le_bytes(), &bucket.next_message_id.to_le_bytes()],
        bump,
    )]
    pub message: Account<'info, Message>,

    pub system_program: Program<'info, System>,
}

pub fn write_handler(ctx: Context<WriteMessage>, input: MessageInput) -> Result<()> {
    input.validate()?;
    let bucket = &mut ctx.accounts.bucket;

    // Only the program creates Tag accounts, so its stored fields are trusted.
    match (&input.tag, ctx.accounts.tag.as_mut()) {
        (None, None) => {}
        (Some(name), Some(tag)) => {
            require!(
                tag.bucket_id == bucket.id && tag.tag == *name,
                BucketError::WrongTag
            );
            tag.message_count = tag
                .message_count
                .checked_add(1)
                .ok_or(BucketError::Overflow)?;
        }
        _ => return err!(BucketError::WrongTag),
    }

    let id = bucket.next_message_id;
    bucket.next_message_id = id.checked_add(1).ok_or(BucketError::Overflow)?;
    bucket.message_count = bucket
        .message_count
        .checked_add(1)
        .ok_or(BucketError::Overflow)?;

    let contributor = ctx.accounts.contributor_signer.key();
    let message = &mut ctx.accounts.message;
    message.bucket_id = bucket.id;
    message.id = id;
    message.reference = input.reference;
    message.tag = input.tag;
    message.description = input.description;
    message.content_type = input.content_type;
    message.content_hash = input.content_hash;
    message.properties = input.properties;
    message.created_at = Clock::get()?.unix_timestamp;
    message.contributor = contributor;
    message.bump = ctx.bumps.message;

    emit!(MessageWritten {
        bucket_id: bucket.id,
        message_id: id,
        contributor,
    });
    Ok(())
}

#[event]
pub struct TagCreated {
    pub bucket_id: u64,
    pub tag: String,
    pub creator: Pubkey,
}

#[event]
pub struct MessageWritten {
    pub bucket_id: u64,
    pub message_id: u64,
    pub contributor: Pubkey,
}
