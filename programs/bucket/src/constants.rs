use anchor_lang::prelude::*;
use solana_sha256_hasher::hash;

/// PDA seed for the singleton config (authority and id counters).
#[constant]
pub const CONFIG_SEED: &[u8] = b"config";

/// PDA seed for a namespace: `["namespace", id]`.
#[constant]
pub const NAMESPACE_SEED: &[u8] = b"namespace";

/// PDA seed for a namespace manager: `["manager", namespace_id, wallet]`.
#[constant]
pub const MANAGER_SEED: &[u8] = b"manager";

/// PDA seed for a bucket: `["bucket", id]`.
#[constant]
pub const BUCKET_SEED: &[u8] = b"bucket";

/// PDA seed for a bucket admin: `["admin", bucket_id, wallet]`.
#[constant]
pub const ADMIN_SEED: &[u8] = b"admin";

/// PDA seed for a bucket contributor: `["contributor", bucket_id, wallet]`.
#[constant]
pub const CONTRIBUTOR_SEED: &[u8] = b"contributor";

/// PDA seed for a bucket viewer: `["viewer", bucket_id, x25519 key]`.
#[constant]
pub const VIEWER_SEED: &[u8] = b"viewer";

/// PDA seed for a tag: `["tag", bucket_id, sha256(tag)]`.
#[constant]
pub const TAG_SEED: &[u8] = b"tag";

/// PDA seed for a message: `["message", bucket_id, message_id]`.
#[constant]
pub const MESSAGE_SEED: &[u8] = b"message";

/// A tag can be up to 200 bytes, longer than a seed may be, so its PDA is
/// keyed by the tag's hash.
pub fn tag_seed(tag: &str) -> [u8; 32] {
    hash(tag.as_bytes()).to_bytes()
}
