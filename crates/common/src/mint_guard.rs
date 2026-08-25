use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_spl::token::spl_token;
use anchor_spl::token_2022::spl_token_2022::{
    extension::{BaseStateWithExtensions, ExtensionType, StateWithExtensions},
    state::Mint as MintState,
};

/// Why a mint can't back a vault. Each program maps these onto its own
/// error enum, so its error codes stay stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintIssue {
    /// The owner is not a token program.
    NotAToken,
    /// A base-mint freeze authority; whoever holds it could lock the vault.
    UnsupportedAuthority,
    /// A Token-2022 extension letting a third party move, block, or hide
    /// vaulted funds.
    UnsupportedExtension,
}

/// Check a mint against what the vault accounting can support. A live
/// `mint_authority` stays allowed on purpose: the operator bond tracks the
/// XCAV supply, and changing that supply is a protocol-held lever. Returns
/// the mint's decimals for callers that bound them; malformed account data
/// surfaces as the outer error.
pub fn check_supported_mint(mint: &AccountInfo) -> Result<std::result::Result<u8, MintIssue>> {
    let data = mint.try_borrow_data()?;
    if *mint.owner == spl_token::ID {
        let state = spl_token::state::Mint::unpack(&data)?;
        if state.freeze_authority.is_some() {
            return Ok(Err(MintIssue::UnsupportedAuthority));
        }
        return Ok(Ok(state.decimals));
    }
    if *mint.owner != anchor_spl::token_2022::ID {
        return Ok(Err(MintIssue::NotAToken));
    }
    let state = StateWithExtensions::<MintState>::unpack(&data)?;
    if state.base.freeze_authority.is_some() {
        return Ok(Err(MintIssue::UnsupportedAuthority));
    }
    for extension in state.get_extension_types()? {
        match extension {
            ExtensionType::TransferFeeConfig
            | ExtensionType::MintCloseAuthority
            | ExtensionType::DefaultAccountState
            | ExtensionType::NonTransferable
            | ExtensionType::PermanentDelegate
            | ExtensionType::TransferHook
            | ExtensionType::Pausable
            | ExtensionType::ConfidentialTransferMint
            | ExtensionType::ConfidentialMintBurn => {
                return Ok(Err(MintIssue::UnsupportedExtension));
            }
            _ => {}
        }
    }
    Ok(Ok(state.base.decimals))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classic_mint_data() -> Vec<u8> {
        let mut data = vec![0u8; 82];
        data[45] = 1; // is_initialized
        data
    }

    fn t22_mint_data(extension_type: u16, len: u16) -> Vec<u8> {
        // Base mint (82 bytes) padded to the account type offset (165), the
        // mint tag, then a single TLV entry header plus a zeroed payload.
        let mut data = vec![0u8; 166 + 4 + len as usize];
        data[45] = 1; // is_initialized
        data[165] = 1; // account type: mint
        data[166..168].copy_from_slice(&extension_type.to_le_bytes());
        data[168..170].copy_from_slice(&len.to_le_bytes());
        data
    }

    // The base mint's freeze authority is a COption tag at offset 46.
    fn with_lock_authority(mut data: Vec<u8>) -> Vec<u8> {
        data[46] = 1;
        data
    }

    fn check(owner: Pubkey, mut data: Vec<u8>) -> Result<std::result::Result<u8, MintIssue>> {
        let key = Pubkey::new_unique();
        let mut lamports = 0u64;
        let info = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);
        check_supported_mint(&info)
    }

    #[test]
    fn classic_mint_passes() {
        assert!(matches!(
            check(spl_token::ID, classic_mint_data()),
            Ok(Ok(_))
        ));
    }

    #[test]
    fn non_token_owner_rejected() {
        // Valid mint bytes under the wrong owner must not parse as a mint.
        let owner = Pubkey::new_unique();
        assert!(matches!(
            check(owner, t22_mint_data(18, 64)),
            Ok(Err(MintIssue::NotAToken))
        ));
    }

    #[test]
    fn classic_mint_with_lock_authority_rejected() {
        assert!(matches!(
            check(spl_token::ID, with_lock_authority(classic_mint_data())),
            Ok(Err(MintIssue::UnsupportedAuthority))
        ));
    }

    #[test]
    fn t22_mint_with_lock_authority_rejected() {
        // Metadata pointer (18) is harmless, so the rejection is the authority.
        let data = with_lock_authority(t22_mint_data(18, 64));
        assert!(matches!(
            check(anchor_spl::token_2022::ID, data),
            Ok(Err(MintIssue::UnsupportedAuthority))
        ));
    }

    #[test]
    fn fee_bearing_mint_rejected() {
        // Extension type 1 is the transfer fee config.
        assert!(matches!(
            check(anchor_spl::token_2022::ID, t22_mint_data(1, 108)),
            Ok(Err(MintIssue::UnsupportedExtension))
        ));
    }

    #[test]
    fn permanent_delegate_rejected() {
        // Extension type 12 is the permanent delegate.
        assert!(matches!(
            check(anchor_spl::token_2022::ID, t22_mint_data(12, 32)),
            Ok(Err(MintIssue::UnsupportedExtension))
        ));
    }

    #[test]
    fn pausable_mint_rejected() {
        // Extension type 26 is the pausable config.
        assert!(matches!(
            check(anchor_spl::token_2022::ID, t22_mint_data(26, 33)),
            Ok(Err(MintIssue::UnsupportedExtension))
        ));
    }

    #[test]
    fn metadata_pointer_allowed() {
        // Extension type 18 is the metadata pointer, which is harmless here.
        assert!(matches!(
            check(anchor_spl::token_2022::ID, t22_mint_data(18, 64)),
            Ok(Ok(_))
        ));
    }
}
