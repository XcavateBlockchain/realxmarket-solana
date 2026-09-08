use anchor_lang::prelude::*;

use crate::error::BucketError;

pub const MAX_NAME_LEN: usize = 100;
pub const MAX_URI_LEN: usize = 256;
pub const MAX_CATEGORY_LEN: usize = 50;
pub const MAX_PROPERTIES: usize = 10;
pub const MAX_PROPERTY_KEY_LEN: usize = 50;
pub const MAX_PROPERTY_VALUE_LEN: usize = 200;
pub const MAX_TAG_LEN: usize = 200;
pub const MAX_REFERENCE_LEN: usize = 200;

/// One free-form key/value pair of metadata.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct Property {
    pub key: String,
    pub value: String,
}

fn check_len(value: &str, max: usize) -> Result<()> {
    require!(value.len() <= max, BucketError::MetadataTooLong);
    Ok(())
}

/// Bounded count, bounded lengths, unique keys.
fn check_properties(properties: &[Property]) -> Result<()> {
    require!(
        properties.len() <= MAX_PROPERTIES,
        BucketError::TooManyProperties
    );
    for (i, p) in properties.iter().enumerate() {
        check_len(&p.key, MAX_PROPERTY_KEY_LEN)?;
        check_len(&p.value, MAX_PROPERTY_VALUE_LEN)?;
        require!(
            properties[..i].iter().all(|q| q.key != p.key),
            BucketError::DuplicateProperty
        );
    }
    Ok(())
}

fn string_space(s: &str) -> usize {
    4 + s.len()
}

fn properties_space(properties: &[Property]) -> usize {
    4 + properties
        .iter()
        .map(|p| string_space(&p.key) + string_space(&p.value))
        .sum::<usize>()
}

/// Namespace metadata as supplied at creation.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct NamespaceMetadata {
    pub name: String,
    pub schema_uri: Option<String>,
    pub properties: Vec<Property>,
}

impl NamespaceMetadata {
    pub fn validate(&self) -> Result<()> {
        check_len(&self.name, MAX_NAME_LEN)?;
        if let Some(uri) = &self.schema_uri {
            check_len(uri, MAX_URI_LEN)?;
        }
        check_properties(&self.properties)
    }
}

/// Bucket metadata as supplied at creation.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct BucketMetadata {
    pub name: String,
    pub category: String,
    pub properties: Vec<Property>,
}

impl BucketMetadata {
    pub fn validate(&self) -> Result<()> {
        check_len(&self.name, MAX_NAME_LEN)?;
        check_len(&self.category, MAX_CATEGORY_LEN)?;
        check_properties(&self.properties)
    }
}

/// A message as supplied by a contributor: a reference into the encrypted
/// storage layer plus descriptive metadata.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub struct MessageInput {
    pub reference: String,
    pub tag: Option<String>,
    pub description: String,
    pub content_type: String,
    pub content_hash: [u8; 32],
    pub properties: Vec<Property>,
}

impl MessageInput {
    pub fn validate(&self) -> Result<()> {
        check_len(&self.reference, MAX_REFERENCE_LEN)?;
        if let Some(tag) = &self.tag {
            check_len(tag, MAX_TAG_LEN)?;
        }
        check_len(&self.description, MAX_NAME_LEN)?;
        check_len(&self.content_type, MAX_CATEGORY_LEN)?;
        check_properties(&self.properties)
    }
}

/// Singleton: the force authority and the id counters.
#[account]
#[derive(InitSpace)]
pub struct Config {
    /// May force-remove anything and seat managers without being one.
    pub authority: Pubkey,
    /// Proposed replacement; takes over through `accept_authority`.
    pub pending_authority: Option<Pubkey>,
    pub next_namespace_id: u64,
    pub next_bucket_id: u64,
    pub bump: u8,
}

/// An entity's namespace. Managers create buckets under it.
#[account]
pub struct Namespace {
    pub id: u64,
    pub name: String,
    pub schema_uri: Option<String>,
    pub properties: Vec<Property>,
    pub created_at: i64,
    pub manager_count: u32,
    pub bucket_count: u32,
    pub rent_payer: Pubkey,
    pub bump: u8,
}

impl Namespace {
    /// Sized to the given metadata, not to the maxima.
    pub fn space(m: &NamespaceMetadata) -> usize {
        8 + 8
            + string_space(&m.name)
            + 1
            + m.schema_uri.as_deref().map_or(0, string_space)
            + properties_space(&m.properties)
            + 8
            + 4
            + 4
            + 32
            + 1
    }
}

/// A bucket of messages. `None` for the key means the bucket is locked and
/// nothing can be written.
#[account]
pub struct Bucket {
    pub id: u64,
    pub namespace_id: u64,
    pub name: String,
    pub category: String,
    pub properties: Vec<Property>,
    pub created_at: i64,
    pub encryption_key: Option<[u8; 32]>,
    /// Message ids are never reused, so this only grows.
    pub next_message_id: u64,
    pub message_count: u64,
    pub admin_count: u32,
    pub contributor_count: u32,
    pub viewer_count: u32,
    pub tag_count: u32,
    pub rent_payer: Pubkey,
    pub bump: u8,
}

impl Bucket {
    pub fn space(m: &BucketMetadata) -> usize {
        8 + 8
            + 8
            + string_space(&m.name)
            + string_space(&m.category)
            + properties_space(&m.properties)
            + 8
            + 33
            + 8
            + 8
            + 4 * 4
            + 32
            + 1
    }
}

/// A namespace manager. Its existence is the grant.
#[account]
#[derive(InitSpace)]
pub struct Manager {
    pub namespace_id: u64,
    pub wallet: Pubkey,
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A bucket admin. Its existence is the grant.
#[account]
#[derive(InitSpace)]
pub struct Admin {
    pub bucket_id: u64,
    pub wallet: Pubkey,
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A bucket contributor. Its existence is the grant.
#[account]
#[derive(InitSpace)]
pub struct Contributor {
    pub bucket_id: u64,
    pub wallet: Pubkey,
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A reader's X25519 public key, listed so contributors know whom to encrypt
/// for. Viewers never sign anything on chain.
#[account]
#[derive(InitSpace)]
pub struct Viewer {
    pub bucket_id: u64,
    pub key: [u8; 32],
    pub rent_payer: Pubkey,
    pub bump: u8,
}

/// A tag messages in one bucket may carry.
#[account]
pub struct Tag {
    pub bucket_id: u64,
    pub tag: String,
    pub message_count: u64,
    pub rent_payer: Pubkey,
    pub bump: u8,
}

impl Tag {
    pub fn validate(tag: &str) -> Result<()> {
        check_len(tag, MAX_TAG_LEN)
    }

    pub fn space(tag: &str) -> usize {
        8 + 8 + string_space(tag) + 8 + 32 + 1
    }
}

/// A reference to an encrypted document, with its descriptive metadata.
/// The contributor paid its rent.
#[account]
pub struct Message {
    pub bucket_id: u64,
    pub id: u64,
    pub reference: String,
    pub tag: Option<String>,
    pub description: String,
    pub content_type: String,
    pub content_hash: [u8; 32],
    pub properties: Vec<Property>,
    pub created_at: i64,
    pub contributor: Pubkey,
    pub bump: u8,
}

impl Message {
    pub fn space(m: &MessageInput) -> usize {
        8 + 8
            + 8
            + string_space(&m.reference)
            + 1
            + m.tag.as_deref().map_or(0, string_space)
            + string_space(&m.description)
            + string_space(&m.content_type)
            + 32
            + properties_space(&m.properties)
            + 8
            + 32
            + 1
    }
}
