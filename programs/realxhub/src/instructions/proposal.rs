use anchor_lang::prelude::*;
use xcavate_whitelist::state::{Role, RoleAccount};

use crate::constants::{CONFIG_SEED, HUB_SEED};
use crate::error::HubError;
use crate::state::{Config, Hub, HubStatus};

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ProposalParams {
    pub name: String,
    pub metadata_uri: String,
    pub metadata_hash: [u8; 32],
    pub token_price: u64,
    pub token_supply: u64,
    pub sale_duration: i64,
}

impl ProposalParams {
    fn validate(&self) -> Result<()> {
        require!(
            !self.name.trim().is_empty()
                && self.name.len() <= 64
                && !self.metadata_uri.trim().is_empty()
                && self.metadata_uri.len() <= 200
                && self.metadata_hash != [0; 32]
                && self.token_price > 0
                && self.token_supply > 0
                && self.sale_duration > 0,
            HubError::InvalidProposal
        );
        self.token_price
            .checked_mul(self.token_supply)
            .ok_or(HubError::Overflow)?;
        Ok(())
    }

    fn apply(self, hub: &mut Hub) {
        hub.name = self.name;
        hub.metadata_uri = self.metadata_uri;
        hub.metadata_hash = self.metadata_hash;
        hub.token_price = self.token_price;
        hub.token_supply = self.token_supply;
        hub.sale_duration = self.sale_duration;
    }
}

#[derive(Accounts)]
#[instruction(region_id: u16)]
pub struct CreateProposal<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        seeds = [xcavate_whitelist::ROLE_SEED, operator.key().as_ref(), &[Role::RegionalOperator.seed_byte()]],
        bump = operator_role.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = operator_role.is_compliant() @ HubError::NotCompliant,
    )]
    pub operator_role: Account<'info, RoleAccount>,
    #[account(
        seeds = [regions::REGION_SEED, &region_id.to_le_bytes()],
        bump = region.bump,
        seeds::program = regions::ID,
        constraint = region.owner == operator.key() @ HubError::NotRegionOwner,
    )]
    pub region: Account<'info, regions::state::Region>,
    #[account(init, payer = operator, space = 8 + Hub::INIT_SPACE, seeds = [HUB_SEED, &config.next_hub_id.to_le_bytes()], bump)]
    pub hub: Account<'info, Hub>,
    pub system_program: Program<'info, System>,
}

pub fn create_proposal_handler(
    ctx: Context<CreateProposal>,
    region_id: u16,
    params: ProposalParams,
) -> Result<()> {
    params.validate()?;
    let hub = &mut ctx.accounts.hub;
    hub.hub_id = ctx.accounts.config.next_hub_id;
    hub.operator = ctx.accounts.operator.key();
    hub.region_id = region_id;
    hub.revision = 1;
    hub.status = HubStatus::Draft;
    hub.bond_amount = ctx.accounts.config.bond_amount;
    hub.bump = ctx.bumps.hub;
    params.apply(hub);
    ctx.accounts.config.next_hub_id = hub.hub_id.checked_add(1).ok_or(HubError::Overflow)?;
    emit!(ProposalCreated {
        hub_id: hub.hub_id,
        operator: hub.operator,
        region_id
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(hub_id: u64)]
pub struct OperatorProposal<'info> {
    pub operator: Signer<'info>,
    #[account(mut, seeds = [HUB_SEED, &hub_id.to_le_bytes()], bump = hub.bump, has_one = operator @ HubError::NotOperator)]
    pub hub: Account<'info, Hub>,
    #[account(
        seeds = [xcavate_whitelist::ROLE_SEED, operator.key().as_ref(), &[Role::RegionalOperator.seed_byte()]],
        bump = operator_role.bump,
        seeds::program = xcavate_whitelist::ID,
        constraint = operator_role.is_compliant() @ HubError::NotCompliant,
    )]
    pub operator_role: Account<'info, RoleAccount>,
    #[account(
        seeds = [regions::REGION_SEED, &hub.region_id.to_le_bytes()],
        bump = region.bump,
        seeds::program = regions::ID,
        constraint = region.owner == operator.key() @ HubError::NotRegionOwner,
    )]
    pub region: Account<'info, regions::state::Region>,
}

pub fn update_proposal_handler(
    ctx: Context<OperatorProposal>,
    hub_id: u64,
    params: ProposalParams,
) -> Result<()> {
    params.validate()?;
    let hub = &mut ctx.accounts.hub;
    require!(
        matches!(hub.status, HubStatus::Draft | HubStatus::Rejected),
        HubError::InvalidStatus
    );
    hub.revision = hub.revision.checked_add(1).ok_or(HubError::Overflow)?;
    params.apply(hub);
    // A rejected version must be edited and submitted again. Neither the old
    // decision nor its timestamps may carry over to the revised application.
    hub.status = HubStatus::Draft;
    hub.submitted_at = 0;
    hub.reviewed_at = 0;
    hub.reviewed_by = Pubkey::default();
    emit!(ProposalUpdated {
        hub_id,
        revision: hub.revision
    });
    Ok(())
}

pub fn submit_proposal_handler(ctx: Context<OperatorProposal>, hub_id: u64) -> Result<()> {
    let hub = &mut ctx.accounts.hub;
    require!(hub.status == HubStatus::Draft, HubError::InvalidStatus);
    hub.status = HubStatus::Submitted;
    hub.submitted_at = Clock::get()?.unix_timestamp;
    emit!(ProposalSubmitted {
        hub_id,
        revision: hub.revision,
        metadata_hash: hub.metadata_hash
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(hub_id: u64)]
pub struct ReviewProposal<'info> {
    pub verifier: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = verifier @ HubError::NotVerifier)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [HUB_SEED, &hub_id.to_le_bytes()], bump = hub.bump)]
    pub hub: Account<'info, Hub>,
}

pub fn review_proposal_handler(
    ctx: Context<ReviewProposal>,
    hub_id: u64,
    revision: u32,
    metadata_hash: [u8; 32],
    approve: bool,
) -> Result<()> {
    let hub = &mut ctx.accounts.hub;
    require!(hub.status == HubStatus::Submitted, HubError::InvalidStatus);
    require!(
        hub.operator != ctx.accounts.verifier.key(),
        HubError::SelfReview
    );
    require!(
        hub.revision == revision && hub.metadata_hash == metadata_hash,
        HubError::ReviewMismatch
    );
    hub.status = if approve {
        HubStatus::Approved
    } else {
        HubStatus::Rejected
    };
    hub.reviewed_at = Clock::get()?.unix_timestamp;
    hub.reviewed_by = ctx.accounts.verifier.key();
    emit!(ProposalReviewed {
        hub_id,
        revision,
        verifier: hub.reviewed_by,
        approve,
        metadata_hash
    });
    Ok(())
}

#[event]
pub struct ProposalCreated {
    pub hub_id: u64,
    pub operator: Pubkey,
    pub region_id: u16,
}
#[event]
pub struct ProposalUpdated {
    pub hub_id: u64,
    pub revision: u32,
}
#[event]
pub struct ProposalSubmitted {
    pub hub_id: u64,
    pub revision: u32,
    pub metadata_hash: [u8; 32],
}
#[event]
pub struct ProposalReviewed {
    pub hub_id: u64,
    pub revision: u32,
    pub verifier: Pubkey,
    pub approve: bool,
    pub metadata_hash: [u8; 32],
}
