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

    /// Create a namespace; the signer becomes its first manager.
    pub fn create_namespace(
        ctx: Context<CreateNamespace>,
        metadata: NamespaceMetadata,
    ) -> Result<()> {
        namespaces::create_namespace_handler(ctx, metadata)
    }

    /// Seat another manager on a namespace. Manager-only.
    pub fn add_manager(ctx: Context<AddManager>) -> Result<()> {
        members::add_manager_handler(ctx)
    }

    /// Unseat a manager. Manager-only; the last manager cannot be removed.
    pub fn remove_manager(ctx: Context<RemoveManager>) -> Result<()> {
        members::remove_manager_handler(ctx)
    }

    /// Create a bucket under a namespace. Manager-only. A new bucket is
    /// locked until an admin sets its key.
    pub fn create_bucket(ctx: Context<CreateBucket>, metadata: BucketMetadata) -> Result<()> {
        buckets::create_bucket_handler(ctx, metadata)
    }

    /// Seat a bucket admin. Manager-only.
    pub fn add_admin(ctx: Context<AddAdmin>) -> Result<()> {
        members::add_admin_handler(ctx)
    }

    /// Unseat a bucket admin. Manager-only.
    pub fn remove_admin(ctx: Context<RemoveAdmin>) -> Result<()> {
        members::remove_admin_handler(ctx)
    }

    /// Seat a contributor. Admin-only.
    pub fn add_contributor(ctx: Context<AddContributor>) -> Result<()> {
        members::add_contributor_handler(ctx)
    }

    /// Unseat a contributor. Admin-only.
    pub fn remove_contributor(ctx: Context<RemoveContributor>) -> Result<()> {
        members::remove_contributor_handler(ctx)
    }

    /// List a viewer's X25519 public key. Admin-only.
    pub fn add_viewer(ctx: Context<AddViewer>, viewer_key: [u8; 32]) -> Result<()> {
        members::add_viewer_handler(ctx, viewer_key)
    }

    /// Delist a viewer. Admin-only.
    pub fn remove_viewer(ctx: Context<RemoveViewer>, viewer_key: [u8; 32]) -> Result<()> {
        members::remove_viewer_handler(ctx, viewer_key)
    }

    /// Lock the bucket; nothing can be written until a key is set. Admin-only.
    pub fn pause_writing(ctx: Context<AdminOnBucket>) -> Result<()> {
        buckets::pause_writing_handler(ctx)
    }

    /// Set the bucket's key and open it for writing. Admin-only.
    pub fn resume_writing(ctx: Context<AdminOnBucket>, encryption_key: [u8; 32]) -> Result<()> {
        buckets::resume_writing_handler(ctx, encryption_key)
    }

    /// Replace the key of an open bucket. Admin-only; fails while locked.
    pub fn rotate_key(ctx: Context<AdminOnBucket>, encryption_key: [u8; 32]) -> Result<()> {
        buckets::rotate_key_handler(ctx, encryption_key)
    }

    /// Create a tag messages in the bucket may carry. Admin-only.
    pub fn create_tag(ctx: Context<CreateTag>, tag: String) -> Result<()> {
        messages::create_tag_handler(ctx, tag)
    }

    /// Write a message. Contributor-only; the bucket must be open and the
    /// tag, if the message names one, must exist.
    pub fn write(ctx: Context<WriteMessage>, input: MessageInput) -> Result<()> {
        messages::write_handler(ctx, input)
    }
}
