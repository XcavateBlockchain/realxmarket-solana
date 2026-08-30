//! Property-based solvency for the secondary market: random op walks on a
//! finalized property, cross-checking the accounts against each other after
//! every step. The income invariant spans the settle CPI into the property
//! program, so drift on either side of that seam shows up here.

mod common;
use common::*;

use anchor_spl::token_2022::spl_token_2022::{
    extension::StateWithExtensions, state::Account as TokenAccountState,
};
use proptest::prelude::*;

fn token_balance(svm: &LiteSVM, addr: &Pubkey) -> u64 {
    svm.get_account(addr)
        .filter(|a| !a.data.is_empty())
        .map(|a| {
            StateWithExtensions::<TokenAccountState>::unpack(&a.data)
                .unwrap()
                .base
                .amount
        })
        .unwrap_or(0)
}

fn account_alive(svm: &LiteSVM, addr: &Pubkey) -> bool {
    svm.get_account(addr).is_some_and(|a| !a.data.is_empty())
}

fn share_listing(svm: &LiteSVM, id: u64) -> Option<marketplace::state::ShareListing> {
    svm.get_account(&share_listing_pda(id))
        .filter(|a| !a.data.is_empty())
        .map(|a| marketplace::state::ShareListing::try_deserialize(&mut &a.data[..]).unwrap())
}

fn offer(svm: &LiteSVM, id: u64, offeror: &Pubkey) -> Option<marketplace::state::Offer> {
    svm.get_account(&offer_pda(id, offeror))
        .filter(|a| !a.data.is_empty())
        .map(|a| marketplace::state::Offer::try_deserialize(&mut &a.data[..]).unwrap())
}

/// The tGBP stream's accumulator, zero until income is first seeded.
fn income_acc(svm: &LiteSVM) -> u128 {
    svm.get_account(&property_income_pda(0))
        .filter(|a| !a.data.is_empty())
        .map(|a| {
            let income =
                property::state::PropertyIncome::try_deserialize(&mut &a.data[..]).unwrap();
            income
                .streams
                .iter()
                .find(|s| s.mint == tgbp_mint())
                .map(|s| s.per_share)
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// Ids of share listings still open, for aiming ops at live targets.
fn live_listings(svm: &LiteSVM) -> Vec<u64> {
    (0..config_of(svm).next_share_listing_id)
        .filter(|id| share_listing(svm, *id).is_some())
        .collect()
}

/// (listing id, offeror) pairs with a live offer on a live listing.
fn live_offers(svm: &LiteSVM, wallets: &[Keypair]) -> Vec<(u64, Pubkey)> {
    let mut pairs = Vec::new();
    for id in live_listings(svm) {
        for wallet in wallets {
            if offer(svm, id, &wallet.pubkey()).is_some() {
                pairs.push((id, wallet.pubkey()));
            }
        }
    }
    pairs
}

fn check_invariants(svm: &LiteSVM, wallets: &[Keypair]) -> Result<(), TestCaseError> {
    let next_id = config_of(svm).next_share_listing_id;

    let mut held = 0u64;
    let mut holders = 0u32;
    for wallet in wallets {
        let key = wallet.pubkey();
        // Every listed share is backed by exactly one live share listing.
        let listed_for_sale: u32 = (0..next_id)
            .filter_map(|id| share_listing(svm, id))
            .filter(|l| l.seller == key)
            .map(|l| l.amount)
            .sum();
        if account_alive(svm, &holding_pda(0, &key)) {
            let holding = holding_of(svm, 0, &key);
            held += holding.amount as u64;
            holders += 1;
            prop_assert_eq!(holding.listed, listed_for_sale);
            prop_assert!(holding.locked() + holding.listed <= holding.amount);
        } else {
            prop_assert_eq!(listed_for_sale, 0);
        }
        // Every offer vault holds exactly the recorded bid; a settled or
        // refunded offer leaves nothing behind.
        for id in 0..next_id {
            let vault_ata = payment_ata(&offer_vault_pda(id, &key), &tgbp_mint());
            match offer(svm, id, &key) {
                Some(o) => prop_assert_eq!(token_balance(svm, &vault_ata), o.held),
                None => prop_assert_eq!(token_balance(svm, &vault_ata), 0),
            }
        }
    }
    // Shares: all of them live in the holdings, none stray back to the vault.
    prop_assert_eq!(held, SHARE_AMOUNT as u64);
    prop_assert_eq!(token_balance(svm, &vault_share_account(0)), 0);
    prop_assert_eq!(property_of(svm, 0).holder_count, holders);

    // Income: everything ever accrued is either banked as pending or still
    // owed against a checkpoint, no matter how the shares moved.
    let acc = income_acc(svm);
    if acc > 0 {
        let mut entitled = 0u128;
        for wallet in wallets {
            let key = wallet.pubkey();
            let shares = if account_alive(svm, &holding_pda(0, &key)) {
                holding_of(svm, 0, &key).amount
            } else {
                0
            };
            let (banked, settled_to) = if account_alive(svm, &property_checkpoint_pda(0, &key)) {
                let cp = checkpoint_of(svm, &key);
                cp.entries
                    .first()
                    .map(|e| (e.pending, e.per_share))
                    .unwrap_or((0, 0))
            } else {
                (0, 0)
            };
            entitled += banked as u128 + (acc - settled_to) * shares as u128;
        }
        prop_assert_eq!(entitled, acc * SHARE_AMOUNT as u128);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]
    #[test]
    fn random_secondary_walks_stay_solvent(
        ops in proptest::collection::vec(
            (0usize..5usize, 0usize..5usize, 0u8..11u8, 1u32..30u32), 6..18
        )
    ) {
        let (mut svm, admin, mut wallets) = finalized_property();
        // A fifth wallet with no shares yet, so fresh holdings and
        // checkpoints get created mid-walk.
        wallets.push(new_investor(&mut svm, &admin));
        for wallet in &wallets {
            give_tgbp(&mut svm, &wallet.pubkey(), 1_000_000_000_000);
        }
        // One listing up front, so market ops always have a target even in
        // walks whose random prefix never relists.
        ok(
            &mut svm,
            relist_ix(&wallets[1].pubkey(), 0, 0, 20, SHARE_PRICE),
            &wallets[1],
            &[&wallets[1]],
        );
        // Cumulative tGBP accrued per share; bumped by the income op.
        let mut acc = 0u128;

        for (who, other, kind, amount) in ops {
            let actor = &wallets[who];
            let peer = &wallets[other];
            // Rejections (locked shares, thin holdings, self-trades, offers
            // outgrowing a shrunk listing) are part of the walk; the books
            // must balance either way.
            match kind {
                0 => {
                    let id = config_of(&svm).next_share_listing_id;
                    let price = SHARE_PRICE + amount as u64;
                    let ix = relist_ix(&actor.pubkey(), 0, id, amount, price);
                    let _ = process(&mut svm, ix, actor, &[actor]);
                }
                1 => {
                    let ids = live_listings(&svm);
                    if ids.is_empty() {
                        continue;
                    }
                    let id = ids[amount as usize % ids.len()];
                    let listing = share_listing(&svm, id).unwrap();
                    let seller = wallets.iter().find(|w| w.pubkey() == listing.seller);
                    if let Some(seller) = seller {
                        let ix = delist_ix(&seller.pubkey(), 0, id);
                        let _ = process(&mut svm, ix, seller, &[seller]);
                    }
                }
                2 => {
                    let ids = live_listings(&svm);
                    if ids.is_empty() {
                        continue;
                    }
                    let id = ids[amount as usize % ids.len()];
                    let listing = share_listing(&svm, id).unwrap();
                    let ix = buy_relisted_ix(
                        &actor.pubkey(),
                        0,
                        id,
                        &listing.seller,
                        amount.min(listing.amount),
                        u64::MAX,
                    );
                    let _ = process_with_budget(&mut svm, ix, actor, &[actor]);
                }
                3 => {
                    let ids = live_listings(&svm);
                    if ids.is_empty() {
                        continue;
                    }
                    let id = ids[amount as usize % ids.len()];
                    let for_sale = share_listing(&svm, id).unwrap().amount;
                    let ix = make_offer_ix(
                        &actor.pubkey(),
                        id,
                        amount.min(for_sale),
                        SHARE_PRICE - amount as u64,
                        tgbp_mint(),
                        tgbp_acc(&actor.pubkey()),
                    );
                    let _ = process(&mut svm, ix, actor, &[actor]);
                }
                4 => {
                    // Any offer of the actor's, live listing or not: cancel
                    // must work either way.
                    let next = config_of(&svm).next_share_listing_id;
                    let owned: Vec<u64> = (0..next)
                        .filter(|id| offer(&svm, *id, &actor.pubkey()).is_some())
                        .collect();
                    if owned.is_empty() {
                        continue;
                    }
                    let id = owned[amount as usize % owned.len()];
                    let ix = cancel_offer_ix(&actor.pubkey(), id, tgbp_mint());
                    let _ = process(&mut svm, ix, actor, &[actor]);
                }
                5 => {
                    let pairs = live_offers(&svm, &wallets);
                    if pairs.is_empty() {
                        continue;
                    }
                    let (id, offeror) = pairs[amount as usize % pairs.len()];
                    let listing = share_listing(&svm, id).unwrap();
                    let seller = wallets.iter().find(|w| w.pubkey() == listing.seller);
                    let nonce = offer(&svm, id, &offeror).unwrap().nonce;
                    if let Some(seller) = seller {
                        let ix = accept_offer_ix(
                            &seller.pubkey(),
                            0,
                            id,
                            &offeror,
                            nonce,
                            tgbp_mint(),
                        );
                        let _ = process_with_budget(&mut svm, ix, seller, &[seller]);
                    }
                }
                6 => {
                    let pairs = live_offers(&svm, &wallets);
                    if pairs.is_empty() {
                        continue;
                    }
                    let (id, offeror) = pairs[amount as usize % pairs.len()];
                    let listing = share_listing(&svm, id).unwrap();
                    let seller = wallets.iter().find(|w| w.pubkey() == listing.seller);
                    let nonce = offer(&svm, id, &offeror).unwrap().nonce;
                    if let Some(seller) = seller {
                        let ix =
                            reject_offer_ix(&seller.pubkey(), id, &offeror, nonce, tgbp_mint());
                        let _ = process(&mut svm, ix, seller, &[seller]);
                    }
                }
                7 => {
                    let ix = send_shares_ix(&actor.pubkey(), &peer.pubkey(), 0, amount);
                    let _ = process(&mut svm, ix, actor, &[actor]);
                }
                8 => {
                    let ix = unlock_votes_ix(&actor.pubkey(), 0, 1);
                    let _ = process(&mut svm, ix, actor, &[actor]);
                }
                9 => {
                    // Close an emptied holding, or empty the actor's out
                    // first so a later pass has one to close.
                    let empty = wallets.iter().find(|w| {
                        account_alive(&svm, &holding_pda(0, &w.pubkey()))
                            && holding_of(&svm, 0, &w.pubkey()).amount == 0
                    });
                    if let Some(owner) = empty {
                        let ix = close_holding_ix(&actor.pubkey(), 0, &owner.pubkey());
                        let _ = process(&mut svm, ix, actor, &[actor]);
                    } else if account_alive(&svm, &holding_pda(0, &actor.pubkey())) {
                        let holding = holding_of(&svm, 0, &actor.pubkey());
                        let all = holding.transferable();
                        if all > 0 {
                            let ix = send_shares_ix(&actor.pubkey(), &peer.pubkey(), 0, all);
                            let _ = process(&mut svm, ix, actor, &[actor]);
                        }
                    }
                }
                _ => {
                    acc += amount as u128 * 1_000_000_000;
                    seed_income_stream(&mut svm, acc);
                }
            }
            check_invariants(&svm, &wallets)?;
        }
    }
}
