use anchor_lang::prelude::*;
use xcavate_whitelist::state::Compliance;

use crate::error::MarketplaceError;

/// Check a wallet's compliance record. Most gates take it as a typed account
/// with a seeds constraint instead; the few too stack-tight for that keep it
/// unchecked and pay a PDA derivation here.
pub fn require_compliant<'info>(record: &'info AccountInfo<'info>, wallet: &Pubkey) -> Result<()> {
    let (expected, _) = Pubkey::find_program_address(
        &[xcavate_whitelist::COMPLIANCE_SEED, wallet.as_ref()],
        &xcavate_whitelist::ID,
    );
    require_keys_eq!(
        *record.key,
        expected,
        MarketplaceError::WrongRegistryAccount
    );
    require!(
        Account::<Compliance>::try_from(record)?.is_live()?,
        MarketplaceError::NotCompliant
    );
    Ok(())
}
