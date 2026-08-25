use anchor_lang::prelude::*;
use xcavate_common::mint_guard::{check_supported_mint, MintIssue};

use crate::error::MarketplaceError;

/// The shared guard, its findings mapped onto this program's errors.
pub fn require_supported_mint(mint: &AccountInfo) -> Result<u8> {
    match check_supported_mint(mint)? {
        Ok(decimals) => Ok(decimals),
        Err(MintIssue::NotAToken) => err!(MarketplaceError::InvalidMint),
        Err(MintIssue::UnsupportedAuthority) => err!(MarketplaceError::UnsupportedMintAuthority),
        Err(MintIssue::UnsupportedExtension) => err!(MarketplaceError::UnsupportedMintExtension),
    }
}
