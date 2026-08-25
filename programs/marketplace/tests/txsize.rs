mod common;
use common::*;
use solana_message::Message;

/// Serialized size of a single-instruction legacy transaction: the message
/// plus its signatures. LiteSVM never enforces the packet limit, so without
/// this the suite happily passes instructions no cluster would accept.
fn tx_size(ix: anchor_lang::solana_program::instruction::Instruction, signers: usize) -> usize {
    let payer = Keypair::new();
    let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &Default::default());
    msg.serialize().len() + 1 + 64 * signers
}

const PACKET_LIMIT: usize = 1232;

#[test]
fn instructions_fit_in_one_packet() {
    let a = Pubkey::new_unique();
    let b = Pubkey::new_unique();
    for (label, size) in [
        ("list_property", tx_size(list_ix(&b, 0), 2)),
        (
            "buy_property_shares",
            tx_size(buy_ix(&a, &b, 0, 10, u64::MAX), 3),
        ),
        ("claim_shares", tx_size(claim_ix(&a, &b, 0), 3)),
        (
            "send_property_shares",
            tx_size(send_shares_ix(&b, &a, 0, 5), 2),
        ),
    ] {
        assert!(
            size <= PACKET_LIMIT,
            "{label} is {size} bytes, over the {PACKET_LIMIT} limit"
        );
    }
}

// The two settlement paths carry both sides' accounts and are v0-only: the
// client sends them with an address lookup table. buy_relisted_shares
// happens to sit two bytes under the legacy limit today, but that headroom
// is not contract; any added account spends it. Pinned so growth is a
// deliberate decision about what the table has to absorb.
#[test]
fn settlement_paths_need_a_lookup_table() {
    let a = Pubkey::new_unique();
    let b = Pubkey::new_unique();
    for (label, size, budget) in [
        (
            "accept_offer",
            tx_size(accept_offer_ix(&b, 0, 0, &a, 0, tgbp_mint()), 2),
            1350,
        ),
        (
            "buy_relisted_shares",
            tx_size(buy_relisted_ix(&a, 0, 0, &b, 5, u64::MAX), 2),
            1230,
        ),
    ] {
        assert!(
            size <= budget,
            "{label} grew to {size} bytes, over its {budget} budget"
        );
    }
}
