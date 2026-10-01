use anchor_lang::prelude::*;

#[constant]
pub const CONFIG_SEED: &[u8] = b"config";
#[constant]
pub const HUB_SEED: &[u8] = b"hub";
#[constant]
pub const HUB_MINT_SEED: &[u8] = b"hub_mint";
#[constant]
pub const TOKEN_VAULT_SEED: &[u8] = b"token_vault";
#[constant]
pub const PAYMENT_VAULT_SEED: &[u8] = b"payment_vault";
#[constant]
pub const BOND_VAULT_SEED: &[u8] = b"bond_vault";
#[constant]
pub const POSITION_SEED: &[u8] = b"position";

#[constant]
pub const RESERVATION_SALE_SEED: &[u8] = b"reservation_sale";
#[constant]
pub const HUB_RESERVATION_SEED: &[u8] = b"hub_reservation";
#[constant]
pub const PAYMENT_RESERVATION_SEED: &[u8] = b"payment_reservation";

/// A fully reserved hub gives every buyer three complete days to pay.
pub const CLAIM_WINDOW_SECONDS: i64 = 3 * 24 * 60 * 60;
