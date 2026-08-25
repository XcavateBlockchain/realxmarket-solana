use anchor_lang::prelude::*;
use xcavate_whitelist::state::Compliance;

use crate::error::MarketplaceError;

/// Check a wallet's compliance record. Most gates take it as a typed account
/// with a seeds constraint instead; the few too stack-tight for that keep it
/// unchecked and verify here. `try_from` proves the roles program owns it
/// and it is a `Compliance` account, and that program only ever writes
/// `user` from the PDA seed, so the field pins the wallet as tightly as a
/// derivation would. `find_program_address` is avoided on purpose: its cost
/// varies with the keys, and this runs inside the tightest instruction.
pub fn require_compliant<'info>(record: &'info AccountInfo<'info>, wallet: &Pubkey) -> Result<()> {
    let record = Account::<Compliance>::try_from(record)?;
    require_keys_eq!(record.user, *wallet, MarketplaceError::WrongRegistryAccount);
    require!(record.is_live()?, MarketplaceError::NotCompliant);
    Ok(())
}
