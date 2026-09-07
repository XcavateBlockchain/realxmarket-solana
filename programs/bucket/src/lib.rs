pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("FyDcu5WMNJw2kmeDsoiyaeeumHfQxrZ91EuZjzveNs7G");

/// Namespaced message buckets for the realXmarket protocol.
///
/// A namespace belongs to an entity such as a property. Its managers create
/// buckets under it and seat each bucket's admins. Admins seat contributors
/// and viewers, hold the bucket's encryption key, and can pause writing.
/// Contributors write messages: references to encrypted documents held off
/// chain, with descriptive metadata on chain. Viewers are X25519 public keys
/// the documents are encrypted for; they never transact.
///
/// Every record is a rent-exempt account paid by whoever creates it and
/// refunded when it is removed.
#[program]
pub mod bucket {
    use super::*;

    /// Initialize the singleton config; the signer becomes the authority.
    pub fn initialize_config(ctx: Context<InitializeConfig>) -> Result<()> {
        initialize::handler(ctx)
    }

    /// Propose a new authority (two-step handover). Authority-only.
    pub fn update_authority(ctx: Context<UpdateAuthority>, new_authority: Pubkey) -> Result<()> {
        initialize::update_authority_handler(ctx, new_authority)
    }

    /// Complete the authority handover. Signed by the pending authority.
    pub fn accept_authority(ctx: Context<AcceptAuthority>) -> Result<()> {
        initialize::accept_authority_handler(ctx)
    }
}
