use anchor_lang::prelude::*;
use xcavate_common::mint_guard::{check_supported_mint, MintIssue};

use crate::error::RegionsError;

/// The shared guard, its findings mapped onto this program's errors.
pub fn require_supported_mint(mint: &AccountInfo) -> Result<u8> {
    match check_supported_mint(mint)? {
        Ok(decimals) => Ok(decimals),
        Err(MintIssue::NotAToken) => err!(RegionsError::InvalidMint),
        Err(MintIssue::UnsupportedAuthority) => err!(RegionsError::UnsupportedMintAuthority),
        Err(MintIssue::UnsupportedExtension) => err!(RegionsError::UnsupportedMintExtension),
    }
}
