use anchor_lang::prelude::*;

use crate::constants::{CONFIG_SEED, MANAGER_SEED, NAMESPACE_SEED};
use crate::error::BucketError;
use crate::state::{Config, Manager, Namespace, NamespaceMetadata};

/// Creates a namespace and seats the creator as its first manager. The
/// creator pays the rent of both records and gets it back when they are
/// removed.
#[derive(Accounts)]
#[instruction(metadata: NamespaceMetadata)]
pub struct CreateNamespace<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,

    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(
        init,
        payer = creator,
        space = Namespace::space(&metadata),
        seeds = [NAMESPACE_SEED, &config.next_namespace_id.to_le_bytes()],
        bump,
    )]
    pub namespace: Account<'info, Namespace>,

    #[account(
        init,
        payer = creator,
        space = 8 + Manager::INIT_SPACE,
        seeds = [MANAGER_SEED, &config.next_namespace_id.to_le_bytes(), creator.key().as_ref()],
        bump,
    )]
    pub manager: Account<'info, Manager>,

    pub system_program: Program<'info, System>,
}

pub fn create_namespace_handler(
    ctx: Context<CreateNamespace>,
    metadata: NamespaceMetadata,
) -> Result<()> {
    metadata.validate()?;
    let config = &mut ctx.accounts.config;
    let id = config.next_namespace_id;
    config.next_namespace_id = id.checked_add(1).ok_or(BucketError::Overflow)?;

    let creator = ctx.accounts.creator.key();
    let namespace = &mut ctx.accounts.namespace;
    namespace.id = id;
    namespace.name = metadata.name;
    namespace.schema_uri = metadata.schema_uri;
    namespace.properties = metadata.properties;
    namespace.created_at = Clock::get()?.unix_timestamp;
    namespace.manager_count = 1;
    namespace.bucket_count = 0;
    namespace.rent_payer = creator;
    namespace.bump = ctx.bumps.namespace;

    let manager = &mut ctx.accounts.manager;
    manager.namespace_id = id;
    manager.wallet = creator;
    manager.rent_payer = creator;
    manager.bump = ctx.bumps.manager;

    emit!(NamespaceCreated {
        namespace_id: id,
        creator,
    });
    emit!(ManagerAdded {
        namespace_id: id,
        manager: creator,
        caller: Some(creator),
    });
    Ok(())
}

#[event]
pub struct NamespaceCreated {
    pub namespace_id: u64,
    pub creator: Pubkey,
}

/// `caller` is `None` when the authority seated the manager by force.
#[event]
pub struct ManagerAdded {
    pub namespace_id: u64,
    pub manager: Pubkey,
    pub caller: Option<Pubkey>,
}

#[event]
pub struct ManagerRemoved {
    pub namespace_id: u64,
    pub manager: Pubkey,
    pub caller: Option<Pubkey>,
}
