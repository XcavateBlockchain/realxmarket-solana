use anchor_lang::prelude::*;

#[error_code]
pub enum BucketError {
    #[msg("Signer is not the authority")]
    NotAuthority,
    #[msg("Signer is not the program upgrade authority")]
    NotUpgradeAuthority,
    #[msg("Signer is not the pending authority")]
    NotPendingAuthority,
    #[msg("Invalid authority address")]
    InvalidAuthority,
    #[msg("Wrong rent payer")]
    WrongRentPayer,
    #[msg("A metadata field exceeds its length limit")]
    MetadataTooLong,
    #[msg("Too many properties")]
    TooManyProperties,
    #[msg("Duplicate property key")]
    DuplicateProperty,
    #[msg("Bucket is locked for writing")]
    BucketLocked,
    #[msg("Tag account does not match the message's tag")]
    WrongTag,
    #[msg("Cannot remove the last manager of a namespace")]
    LastManager,
    #[msg("Namespace still has buckets")]
    DanglingBuckets,
    #[msg("Namespace still has managers")]
    DanglingManagers,
    #[msg("Bucket still has messages")]
    DanglingMessages,
    #[msg("Bucket still has admins")]
    DanglingAdmins,
    #[msg("Bucket still has contributors")]
    DanglingContributors,
    #[msg("Bucket still has viewers")]
    DanglingViewers,
    #[msg("Bucket still has tags")]
    DanglingTags,
    #[msg("Messages still reference the tag")]
    TagInUse,
    #[msg("Arithmetic overflow")]
    Overflow,
}
