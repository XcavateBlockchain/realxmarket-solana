//! Property-based checks over the legal phase: random walks over verdicts,
//! the silence crank, resignations, case releases, both refund exits, the
//! fee settlement and teardown, starting from a sold-out sale settled in a
//! 9- or 6-decimal mint. Every step must conserve the payment tokens,
//! keep the vault equal to what the accounts say it owes, keep the lawyer's
//! pay inside their quoted costs, and keep every counter honest. Each walk
//! then drains to nothing, proving no reachable state is a trap.

mod common;
use common::*;

use anchor_spl::token_2022::spl_token_2022::{
    extension::StateWithExtensions, state::Account as TokenAccountState,
};
use marketplace::state::{DocumentStatus, ListingStatus};
use proptest::prelude::*;

const COSTS: u64 = 1_000_000_000;
const DOCS: [u8; 32] = [7u8; 32];
const DOCS2: [u8; 32] = [8u8; 32];

fn mint_of(gbp6: bool) -> Pubkey {
    if gbp6 {
        gbp6_mint()
    } else {
        tgbp_mint()
    }
}

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

/// Every account a payment mint's units can sit in during the legal phase.
fn mint_holders(
    investors: &[Keypair],
    dl: &Pubkey,
    sl: &Pubkey,
    developer: &Pubkey,
    operator: &Pubkey,
    gbp6: bool,
) -> Vec<Pubkey> {
    let acc = |owner: &Pubkey| {
        if gbp6 {
            gbp6_acc(owner)
        } else {
            tgbp_acc(owner)
        }
    };
    let mint = mint_of(gbp6);
    let mut all: Vec<Pubkey> = investors.iter().map(|kp| acc(&kp.pubkey())).collect();
    all.push(acc(dl));
    all.push(acc(sl));
    all.push(acc(developer));
    all.push(acc(operator));
    // The fee settlement pays lawyers at their canonical ATAs.
    all.push(payment_ata(dl, &mint));
    all.push(payment_ata(sl, &mint));
    all.push(payment_ata(&listing_vault_pda(0), &mint));
    all.push(payment_ata(&treasury(), &mint));
    all
}

fn total_in(svm: &LiteSVM, accounts: &[Pubkey]) -> u64 {
    // ATAs the flow hasn't created yet count as zero.
    accounts
        .iter()
        .filter(|a| {
            svm.get_account(a)
                .map(|acc| !acc.data.is_empty())
                .unwrap_or(false)
        })
        .map(|a| token_balance(svm, a))
        .sum()
}

/// The fee component of one position, in its own mint's units (floored the
/// same way the purchase math floors).
fn fee_in_mint(shares: u32, gbp6: bool) -> u64 {
    let fee = SHARE_PRICE * shares as u64 / 100;
    if gbp6 {
        fee / 1_000
    } else {
        fee
    }
}

#[allow(clippy::too_many_arguments)]
fn check_invariants(
    svm: &LiteSVM,
    investors: &[Keypair],
    dl: &Pubkey,
    sl: &Pubkey,
    developer: &Pubkey,
    operator: &Pubkey,
    gbp6: bool,
    total: u64,
) -> Result<(), TestCaseError> {
    let mint = mint_of(gbp6);
    // Conservation: nothing mints or burns payment tokens, whatever happens.
    prop_assert_eq!(
        total_in(
            svm,
            &mint_holders(investors, dl, sl, developer, operator, gbp6)
        ),
        total
    );

    // Neither lawyer can collect more than their quote, plus the tax the
    // settlement routes through the SPV side.
    let tax_quote = SHARE_PRICE * 100 * 300 / 10_000;
    let quote_gain = |who: &Pubkey| {
        total_in(svm, &[tgbp_acc(who), payment_ata(who, &tgbp_mint())])
            + total_in(svm, &[gbp6_acc(who), payment_ata(who, &gbp6_mint())]) * 1_000
    };
    let dl_gain_quote = quote_gain(dl);
    let sl_gain_quote = quote_gain(sl);
    prop_assert!(dl_gain_quote <= COSTS);
    prop_assert!(sl_gain_quote <= COSTS + tax_quote);

    if !account_alive(svm, &listing_pda(0)) {
        // Torn down: nothing investor- or lawyer-facing may survive it.
        for investor in investors {
            prop_assert!(!account_alive(svm, &position_pda(0, &investor.pubkey())));
            prop_assert!(!account_alive(svm, &holding_pda(0, &investor.pubkey())));
        }
        prop_assert!(!account_alive(svm, &property_pda(0)));
        prop_assert_eq!(
            token_balance(svm, &payment_ata(&listing_vault_pda(0), &mint)),
            0
        );
        return Ok(());
    }

    let listing = listing_of(svm, 0);
    prop_assert!(matches!(
        listing.status,
        ListingStatus::SoldOut
            | ListingStatus::Legal
            | ListingStatus::Cancelled
            | ListingStatus::Refunding
            | ListingStatus::Finalized
    ));
    // Before settlement the developer's lawyer is never paid, and only the
    // cancellation pays the SPV side.
    if listing.status != ListingStatus::Finalized {
        prop_assert_eq!(dl_gain_quote, 0);
        prop_assert!(sl_gain_quote <= COSTS);
    }
    if listing.status == ListingStatus::Legal {
        prop_assert_eq!(
            listing.developer_lawyer.doc_status,
            DocumentStatus::Approved
        );
        prop_assert_eq!(listing.spv_lawyer.doc_status, DocumentStatus::Approved);
        prop_assert_ne!(listing.developer_lawyer.documents_hash, [0u8; 32]);
        prop_assert_eq!(
            listing.developer_lawyer.documents_hash,
            listing.spv_lawyer.documents_hash
        );
    }
    if listing.status == ListingStatus::Cancelled {
        prop_assert!(listing.spv_costs_due <= COSTS);
        prop_assert!(sl_gain_quote + listing.spv_costs_due <= COSTS);
    }
    if listing.status == ListingStatus::Finalized {
        // Settlement drains the vault to zero.
        prop_assert_eq!(
            token_balance(svm, &payment_ata(&listing_vault_pda(0), &mint)),
            0
        );
    }

    // Case counts mirror the assignments exactly, except after settlement:
    // execute_deal releases both cases and keeps the assignments on the
    // listing only as the record of who settled the sale.
    for lawyer in [dl, sl] {
        let expected = if listing.status == ListingStatus::Finalized {
            0
        } else {
            u32::from(listing.developer_lawyer.lawyer == *lawyer)
                + u32::from(listing.spv_lawyer.lawyer == *lawyer)
        };
        prop_assert_eq!(lawyer_of(svm, lawyer).active_cases, expected);
    }

    // The vault holds exactly what open positions are owed, plus, once
    // cancelled, whatever share of the retained fees hasn't been paid out to
    // the lawyer and treasury yet. A settled sale paid everything out, so
    // the equation no longer applies.
    if listing.status != ListingStatus::Finalized {
        let mut open_owed = 0u64;
        for investor in investors {
            if !account_alive(svm, &position_pda(0, &investor.pubkey())) {
                continue;
            }
            let p = position_of(svm, 0, &investor.pubkey());
            open_owed += p.paid_funds + p.paid_tax;
            if listing.status != ListingStatus::Cancelled {
                open_owed += p.paid_fee;
            }
        }
        let retained = if listing.status == ListingStatus::Cancelled {
            let total_fees: u64 = BUYS.iter().map(|&shares| fee_in_mint(shares, gbp6)).sum();
            let paid_out = token_balance(svm, &(if gbp6 { gbp6_acc(sl) } else { tgbp_acc(sl) }))
                + token_balance(svm, &payment_ata(&treasury(), &mint));
            total_fees - paid_out
        } else {
            0
        };
        prop_assert_eq!(
            token_balance(svm, &payment_ata(&listing_vault_pda(0), &mint)),
            open_owed + retained
        );
    }

    // Voting locks: investor a's lock always matches their live record.
    let record = svm
        .get_account(&lawyer_vote_pda(0, 1, &investors[0].pubkey()))
        .filter(|a| !a.data.is_empty())
        .map(|a| marketplace::state::LawyerVote::try_deserialize(&mut &a.data[..]).unwrap());
    if account_alive(svm, &holding_pda(0, &investors[0].pubkey())) {
        prop_assert_eq!(
            holding_of(svm, 0, &investors[0].pubkey()).locked(),
            record.map(|r| r.power).unwrap_or(0)
        );
    }
    Ok(())
}

/// Sold out in the chosen mint, SPV attested, both lawyers engaged, investor
/// a's 34 shares locked behind their election vote.
#[allow(clippy::type_complexity)]
fn setup_legal(gbp6: bool) -> (LiteSVM, Keypair, Keypair, Vec<Keypair>, Keypair, Keypair) {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_property_ix_in(
            &developer.pubkey(),
            0,
            1,
            POSTCODE,
            mint_of(gbp6),
            SHARE_PRICE,
            SHARE_AMOUNT,
            u64::MAX,
        ),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let spn = sponsor();
    let investors: Vec<Keypair> = (0..4).map(|_| new_investor(&mut svm, &admin)).collect();
    for (investor, shares) in investors.iter().zip(BUYS) {
        let ix = if gbp6 {
            give_gbp6(&mut svm, &investor.pubkey(), 1_000_000_000);
            reserve_ix_with_mint(
                &investor.pubkey(),
                &spn.pubkey(),
                0,
                shares,
                u64::MAX,
                gbp6_mint(),
                gbp6_acc(&investor.pubkey()),
            )
        } else {
            reserve_ix(&investor.pubkey(), &spn.pubkey(), 0, shares, u64::MAX)
        };
        ok(&mut svm, ix, &spn, &[&spn, investor]);
    }
    let confirmer = new_confirmer(&mut svm, &admin);
    ok(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
    );
    for investor in &investors {
        let ix = if gbp6 {
            claim_ix_with_mint(
                &investor.pubkey(),
                &spn.pubkey(),
                0,
                gbp6_mint(),
                gbp6_acc(&investor.pubkey()),
                payment_ata(&listing_vault_pda(0), &gbp6_mint()),
            )
        } else {
            claim_ix(&investor.pubkey(), &spn.pubkey(), 0)
        };
        ok(&mut svm, ix, &spn, &[&spn, investor]);
    }

    let dl = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), 0, &dl.pubkey()),
        &developer,
        &[&developer],
    );
    let sl = new_registered_lawyer(&mut svm, &admin, 1);
    ok(
        &mut svm,
        claim_spv_ix(&sl.pubkey(), 0, 1, COSTS),
        &sl,
        &[&sl, &sponsor()],
    );
    ok(
        &mut svm,
        vote_spv_ix(&investors[0].pubkey(), 0, 1, &sl.pubkey(), None, 34),
        &spn,
        &[&spn, &investors[0]],
    );
    warp(&mut svm, 10_001);
    ok(
        &mut svm,
        finalize_spv_ix(&operator.pubkey(), 0, 1, Some(&sl.pubkey()), &[sl.pubkey()]),
        &operator,
        &[&operator],
    );

    // Empty payment accounts for every settlement payee and the treasury,
    // so payouts and sweeps always have somewhere to land.
    for wallet in [
        &dl.pubkey(),
        &sl.pubkey(),
        &developer.pubkey(),
        &operator.pubkey(),
    ] {
        give_tgbp(&mut svm, wallet, 0);
        give_gbp6(&mut svm, wallet, 0);
    }
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        treasury_payment_ata(),
        &treasury(),
        0,
    );
    set_token_account_for(
        &mut svm,
        gbp6_mint(),
        payment_ata(&treasury(), &gbp6_mint()),
        &treasury(),
        0,
    );

    (svm, operator, developer, investors, dl, sl)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]
    #[test]
    fn random_legal_walks_stay_solvent_and_drain(
        gbp6 in any::<bool>(),
        ops in proptest::collection::vec((0u8..4u8, 0u8..15u8, any::<bool>()), 1..12)
    ) {
        let (mut svm, operator, developer, investors, dl, sl) = setup_legal(gbp6);
        let mint = mint_of(gbp6);
        let total = total_in(&svm, &mint_holders(
            &investors, &dl.pubkey(), &sl.pubkey(), &developer.pubkey(), &operator.pubkey(), gbp6,
        ));
        let cranker = funded(&mut svm);

        let exit = |investor: usize, cancelled: bool| {
            let who = &investors[investor];
            let acc = if gbp6 { gbp6_acc(&who.pubkey()) } else { tgbp_acc(&who.pubkey()) };
            let mut ix = withdraw_exit_ix_with_mint(&who.pubkey(), 0, mint, acc);
            use anchor_lang::InstructionData;
            ix.data = if cancelled {
                marketplace::instruction::WithdrawCancelled { listing_id: 0 }.data()
            } else {
                marketplace::instruction::WithdrawLegalProcessExpired { listing_id: 0 }.data()
            };
            ix
        };

        // Failures are part of the exercise; invariants must hold either way.
        for (who, kind, flag) in ops {
            let who = who as usize;
            let investor = &investors[who];
            match kind {
                0 | 1 => {
                    let lawyer = if kind == 0 { &dl } else { &sl };
                    let hash = if flag { DOCS } else { DOCS2 };
                    let ix = confirm_docs_ix(&lawyer.pubkey(), 0, who % 2 == 0, hash);
                    let _ = process(&mut svm, ix, lawyer, &[lawyer]);
                }
                2 => {
                    let ix = resolve_silent_ix(&cranker.pubkey(), 0);
                    let _ = process(&mut svm, ix, &cranker, &[&cranker]);
                }
                3 => warp(&mut svm, 30_000),
                4 => warp(&mut svm, 70_000),
                5 => {
                    let ix = exit(who, true);
                    let _ = process(&mut svm, ix, investor, &[investor]);
                }
                6 => {
                    let ix = exit(who, false);
                    let _ = process(&mut svm, ix, investor, &[investor]);
                }
                7 => {
                    let ix = unlock_votes_ix(&investors[0].pubkey(), 0, 1);
                    let _ = process(&mut svm, ix, &investors[0], &[&investors[0]]);
                }
                8 => {
                    let lawyer = if flag { &dl } else { &sl };
                    let ix = resign_case_ix(&lawyer.pubkey(), 0);
                    let _ = process(&mut svm, ix, lawyer, &[lawyer]);
                }
                9 => {
                    let lawyer = if flag { &dl } else { &sl };
                    let ix = close_case_ix(&cranker.pubkey(), 0, &lawyer.pubkey());
                    let _ = process(&mut svm, ix, &cranker, &[&cranker]);
                }
                10 => {
                    let ix = settle_fees_ix_with_mint(&cranker.pubkey(), 0, mint, &sl.pubkey());
                    let _ = process(&mut svm, ix, &cranker, &[&cranker]);
                }
                // An accepted mint the sale never collected must bounce.
                11 => {
                    let other = mint_of(!gbp6);
                    let ix = settle_fees_ix_with_mint(&cranker.pubkey(), 0, other, &sl.pubkey());
                    prop_assert!(process(&mut svm, ix, &cranker, &[&cranker]).is_err());
                }
                12 => {
                    let ix = withdraw_deposit_ix(&developer.pubkey(), 0);
                    let _ = process(&mut svm, ix, &developer, &[&developer]);
                }
                13 => {
                    let ix = execute_deal_ix(
                        &cranker.pubkey(),
                        0,
                        1,
                        &developer.pubkey(),
                        &dl.pubkey(),
                        &sl.pubkey(),
                        &operator.pubkey(),
                        &[mint],
                    );
                    let _ = process(&mut svm, ix, &cranker, &[&cranker]);
                }
                _ => {
                    let ix = close_dead_listing_ix_for(
                        &cranker.pubkey(),
                        0,
                        &developer.pubkey(),
                        true,
                        &[mint],
                    );
                    let _ = process(&mut svm, ix, &cranker, &[&cranker]);
                }
            }
            check_invariants(
                &svm, &investors, &dl.pubkey(), &sl.pubkey(),
                &developer.pubkey(), &operator.pubkey(), gbp6, total,
            )?;
        }

        // However the walk went, the sale must drain to nothing: no verdict
        // sequence, crank order, or missed step may leave a trap.
        warp(&mut svm, 200_000);
        for _ in 0..3 {
            let _ = process(
                &mut svm,
                unlock_votes_ix(&investors[0].pubkey(), 0, 1),
                &investors[0],
                &[&investors[0]],
            );
            for (who, investor) in investors.iter().enumerate() {
                for cancelled in [true, false] {
                    let ix = exit(who, cancelled);
                    let _ = process(&mut svm, ix, investor, &[investor]);
                }
            }
            for lawyer in [&dl, &sl] {
                let ix = close_case_ix(&cranker.pubkey(), 0, &lawyer.pubkey());
                let _ = process(&mut svm, ix, &cranker, &[&cranker]);
            }
            let _ = process(
                &mut svm,
                settle_fees_ix_with_mint(&cranker.pubkey(), 0, mint, &sl.pubkey()),
                &cranker,
                &[&cranker],
            );
            // The recorded fee quote is the floored round trip of what the
            // sale collected, so once the refunds are out the retained pot
            // always covers the SPV lawyer: settling the fees clears the debt.
            if account_alive(&svm, &listing_pda(0)) {
                let listing = listing_of(&svm, 0);
                if listing.status == ListingStatus::Cancelled && listing.sold_share_amount == 0 {
                    prop_assert_eq!(listing.spv_costs_due, 0);
                }
            }
            let _ = process(
                &mut svm,
                withdraw_deposit_ix(&developer.pubkey(), 0),
                &developer,
                &[&developer],
            );
            let _ = process(
                &mut svm,
                close_dead_listing_ix_for(
                    &cranker.pubkey(),
                    0,
                    &developer.pubkey(),
                    true,
                    &[mint],
                ),
                &cranker,
                &[&cranker],
            );
            check_invariants(
                &svm, &investors, &dl.pubkey(), &sl.pubkey(),
                &developer.pubkey(), &operator.pubkey(), gbp6, total,
            )?;
        }
        // Either the sale settled, and the listing lives on as the record,
        // or it died and tore down completely.
        if account_alive(&svm, &listing_pda(0)) {
            prop_assert_eq!(listing_of(&svm, 0).status, ListingStatus::Finalized);
        }
    }
}
