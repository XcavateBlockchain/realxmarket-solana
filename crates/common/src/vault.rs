use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

/// Every program keeps its vault under its config PDA at this seed.
const CONFIG_SEED: &[u8] = b"config";

/// Move `amount` XCAV from a user's token account into the protocol vault. The
/// user is the authority and signs the transaction.
pub fn lock_to_vault<'info>(
    token_program: &AccountInfo<'info>,
    from: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    vault: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    amount: u64,
    decimals: u8,
) -> Result<()> {
    transfer_checked(
        CpiContext::new(
            *token_program.key,
            TransferChecked {
                from: from.clone(),
                mint: mint.clone(),
                to: vault.clone(),
                authority: authority.clone(),
            },
        ),
        amount,
        decimals,
    )
}

/// Move `amount` XCAV out of the vault to a recipient token account. The config
/// PDA owns the vault, so the program signs with the config seeds.
#[allow(clippy::too_many_arguments)]
pub fn release_from_vault<'info>(
    token_program: &AccountInfo<'info>,
    vault: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    config: &AccountInfo<'info>,
    config_bump: u8,
    amount: u64,
    decimals: u8,
) -> Result<()> {
    let bump = [config_bump];
    let seeds: &[&[u8]] = &[CONFIG_SEED, &bump];
    transfer_checked(
        CpiContext::new_with_signer(
            *token_program.key,
            TransferChecked {
                from: vault.clone(),
                mint: mint.clone(),
                to: to.clone(),
                authority: config.clone(),
            },
            &[seeds],
        ),
        amount,
        decimals,
    )
}
