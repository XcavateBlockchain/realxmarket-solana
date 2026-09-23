//! Settling an approved sale: the one-transaction payout to the developer,
//! both lawyers, the region's operator and the treasury, in the listing's
//! own mint at either decimals, plus the gates that keep it to sales the
//! lawyers actually approved.

mod common;
use common::*;

use anchor_lang::InstructionData;
use marketplace::state::ListingStatus;

const COSTS: u64 = 1_000_000_000;
const DOCS: [u8; 32] = [7u8; 32];

/// Everything up to `Legal`: tGBP sellout, SPV attested, both lawyers
/// engaged and approving the same document set. Returns the region operator
/// too, since settlement pays them.
#[allow(clippy::type_complexity)]
fn setup_approved(
    tax_paid_by_developer: bool,
) -> (
    LiteSVM,
    Keypair,
    Keypair,
    Vec<Keypair>,
    Keypair,
    Keypair,
    Keypair,
) {
    setup_engaged(tax_paid_by_developer, true, tgbp_mint())
}

/// Same, settling in `mint`, minus the approvals when `approve` is false:
/// the sale stays `SoldOut` with both lawyers engaged.
#[allow(clippy::type_complexity)]
fn setup_engaged(
    tax_paid_by_developer: bool,
    approve: bool,
    mint: Pubkey,
) -> (
    LiteSVM,
    Keypair,
    Keypair,
    Vec<Keypair>,
    Keypair,
    Keypair,
    Keypair,
) {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    let mut list = list_property_ix_in(
        &developer.pubkey(),
        0,
        1,
        POSTCODE,
        mint,
        SHARE_PRICE,
        SHARE_AMOUNT,
        u64::MAX,
    );
    list.data = marketplace::instruction::ListProperty {
        region_id: 1,
        postcode: POSTCODE.to_vec(),
        share_price: SHARE_PRICE,
        share_amount: SHARE_AMOUNT,
        tax_paid_by_developer,
        max_deposit: u64::MAX,
    }
    .data();
    ok(&mut svm, list, &developer, &[&developer]);
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let spn = sponsor();
    let gbp6 = mint == gbp6_mint();
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
    if approve {
        for lawyer in [&dl, &sl] {
            ok(
                &mut svm,
                confirm_docs_ix(&lawyer.pubkey(), 0, true, DOCS),
                lawyer,
                &[lawyer],
            );
        }
        assert_eq!(listing_of(&svm, 0).status, ListingStatus::Legal);
    }

    // Payment accounts for everyone settlement pays.
    for wallet in [
        &developer.pubkey(),
        &dl.pubkey(),
        &sl.pubkey(),
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
    (svm, admin, developer, investors, dl, sl, operator)
}

fn deal_ix(
    cranker: &Pubkey,
    developer: &Pubkey,
    dl: &Pubkey,
    sl: &Pubkey,
    operator: &Pubkey,
) -> anchor_lang::solana_program::instruction::Instruction {
    deal_ix_in(cranker, developer, dl, sl, operator, tgbp_mint())
}

fn deal_ix_in(
    cranker: &Pubkey,
    developer: &Pubkey,
    dl: &Pubkey,
    sl: &Pubkey,
    operator: &Pubkey,
    mint: Pubkey,
) -> anchor_lang::solana_program::instruction::Instruction {
    execute_deal_ix(cranker, 0, 1, developer, dl, sl, operator, &[mint])
}

#[test]
fn execute_deal_pays_everyone_and_finalizes() {
    let (mut svm, _admin, developer, _investors, dl, sl, operator) = setup_approved(false);
    let xcav_before = xcav_balance(&svm, &developer.pubkey());
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        deal_ix(
            &cranker.pubkey(),
            &developer.pubkey(),
            &dl.pubkey(),
            &sl.pubkey(),
            &operator.pubkey(),
        ),
        &cranker,
        &[&cranker],
    );

    // 100 shares: funds 500e9, buyer fee 5e9, tax 15e9, seller fee 1% = 5e9.
    // The tax rides to the SPV lawyer, who also draws their 1 GBP costs from
    // the 10e9 pot; the developer's own lawyer is paid off chain, so the 9e9
    // left splits 67/33 between region and treasury.
    assert_eq!(tgbp_balance(&svm, &developer.pubkey()), 495_000_000_000);
    assert_eq!(tgbp_balance(&svm, &dl.pubkey()), 0);
    assert_eq!(tgbp_balance(&svm, &sl.pubkey()), 15_000_000_000 + COSTS);
    assert_eq!(tgbp_balance(&svm, &operator.pubkey()), 6_030_000_000);

    // The vault drained to zero and the sale is final.
    assert_eq!(token_balance(&svm, &listing_payment_ata(0)), 0);
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.status, ListingStatus::Finalized);
    assert_eq!(listing.deposit, 0);
    assert!(property_of(&svm, 0).finalized);
    assert_eq!(
        xcav_balance(&svm, &developer.pubkey()) - xcav_before,
        LISTING_DEPOSIT
    );

    // Both lawyers are free to leave the registry.
    for lawyer in [&dl, &sl] {
        assert_eq!(lawyer_of(&svm, &lawyer.pubkey()).active_cases, 0);
        ok(
            &mut svm,
            unregister_lawyer_ix(&lawyer.pubkey()),
            lawyer,
            &[lawyer],
        );
    }

    // A settled sale can't settle twice.
    fails_with(
        &mut svm,
        deal_ix(
            &cranker.pubkey(),
            &developer.pubkey(),
            &dl.pubkey(),
            &sl.pubkey(),
            &operator.pubkey(),
        ),
        &cranker,
        &[&cranker],
        "AccountNotInitialized",
    );
}

// The same sale in the 6-decimal mint: every figure is the tGBP one over
// 1_000, including the lawyer's costs.
#[test]
fn execute_deal_settles_a_six_decimal_listing() {
    let (mut svm, _admin, developer, _investors, dl, sl, operator) =
        setup_engaged(false, true, gbp6_mint());
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        deal_ix_in(
            &cranker.pubkey(),
            &developer.pubkey(),
            &dl.pubkey(),
            &sl.pubkey(),
            &operator.pubkey(),
            gbp6_mint(),
        ),
        &cranker,
        &[&cranker],
    );

    assert_eq!(gbp6_balance(&svm, &developer.pubkey()), 495_000_000);
    assert_eq!(gbp6_balance(&svm, &dl.pubkey()), 0);
    assert_eq!(gbp6_balance(&svm, &sl.pubkey()), 15_000_000 + COSTS / 1_000);
    assert_eq!(gbp6_balance(&svm, &operator.pubkey()), 6_030_000);
    assert_eq!(
        token_balance(&svm, &payment_ata(&listing_vault_pda(0), &gbp6_mint())),
        0
    );
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Finalized);
}

#[test]
fn tax_rides_with_the_developer_side() {
    let (mut svm, _admin, developer, _investors, dl, sl, operator) = setup_approved(true);
    let cranker = funded(&mut svm);
    ok(
        &mut svm,
        deal_ix(
            &cranker.pubkey(),
            &developer.pubkey(),
            &dl.pubkey(),
            &sl.pubkey(),
            &operator.pubkey(),
        ),
        &cranker,
        &[&cranker],
    );

    // The developer covers the tax, so it comes out of their share and
    // lands with their own lawyer to remit; that stays the only money the
    // developer's lawyer ever sees here.
    assert_eq!(
        tgbp_balance(&svm, &developer.pubkey()),
        495_000_000_000 - 15_000_000_000
    );
    assert_eq!(tgbp_balance(&svm, &dl.pubkey()), 15_000_000_000);
    assert_eq!(tgbp_balance(&svm, &sl.pubkey()), COSTS);
}

#[test]
fn execute_deal_requires_approval() {
    let (mut svm, _admin, developer, _investors, dl, sl, operator) =
        setup_engaged(false, false, tgbp_mint());
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        deal_ix(
            &cranker.pubkey(),
            &developer.pubkey(),
            &dl.pubkey(),
            &sl.pubkey(),
            &operator.pubkey(),
        ),
        &cranker,
        &[&cranker],
        "ListingNotActive",
    );
}

#[test]
fn execute_deal_misses_the_deadline() {
    let (mut svm, _admin, developer, _investors, dl, sl, operator) = setup_approved(false);
    warp(&mut svm, 100_001);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        deal_ix(
            &cranker.pubkey(),
            &developer.pubkey(),
            &dl.pubkey(),
            &sl.pubkey(),
            &operator.pubkey(),
        ),
        &cranker,
        &[&cranker],
        "LegalProcessExpired",
    );
}

#[test]
fn execute_deal_rejects_foreign_payout_accounts() {
    let (mut svm, _admin, developer, _investors, dl, sl, operator) = setup_approved(false);
    let stranger = funded(&mut svm);
    give_tgbp(&mut svm, &stranger.pubkey(), 0);
    let cranker = funded(&mut svm);
    let mut ix = deal_ix(
        &cranker.pubkey(),
        &developer.pubkey(),
        &dl.pubkey(),
        &sl.pubkey(),
        &operator.pubkey(),
    );
    // Swap the developer's tGBP payout account for a stranger's.
    let fixed = ix.accounts.len() - 7;
    ix.accounts[fixed + 2].pubkey = tgbp_acc(&stranger.pubkey());
    fails_with(&mut svm, ix, &cranker, &[&cranker], "WrongPayee");
}

// A price update between a reservation and its claim must not skew the
// developer's tax obligation: it accrues from the funds the claim actually
// pays, not from the live price.
#[test]
fn price_update_cannot_skew_the_tax_obligation() {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    let mut list = list_ix(&developer.pubkey(), 0);
    list.data = marketplace::instruction::ListProperty {
        region_id: 1,
        postcode: POSTCODE.to_vec(),
        share_price: SHARE_PRICE,
        share_amount: SHARE_AMOUNT,
        tax_paid_by_developer: true,
        max_deposit: u64::MAX,
    }
    .data();
    ok(&mut svm, list, &developer, &[&developer]);
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let spn = sponsor();
    let early = new_investor(&mut svm, &admin);
    ok(
        &mut svm,
        reserve_ix(&early.pubkey(), &spn.pubkey(), 0, 40, u64::MAX),
        &spn,
        &[&spn, &early],
    );
    // The price doubles under the outstanding reservation.
    ok(
        &mut svm,
        upgrade_ix(&developer.pubkey(), 0, SHARE_PRICE * 2),
        &developer,
        &[&developer],
    );
    let late_a = new_investor(&mut svm, &admin);
    let late_b = new_investor(&mut svm, &admin);
    for (investor, shares) in [(&late_a, 30u32), (&late_b, 30u32)] {
        give_tgbp(&mut svm, &investor.pubkey(), 1_000_000_000_000_000);
        ok(
            &mut svm,
            reserve_ix(&investor.pubkey(), &spn.pubkey(), 0, shares, u64::MAX),
            &spn,
            &[&spn, investor],
        );
    }
    let confirmer = new_confirmer(&mut svm, &admin);
    ok(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), 0),
        &confirmer,
        &[&confirmer],
    );
    for investor in [&early, &late_a, &late_b] {
        ok(
            &mut svm,
            claim_ix(&investor.pubkey(), &spn.pubkey(), 0),
            &spn,
            &[&spn, investor],
        );
    }

    // Each claim's obligation is 3% of the funds it actually paid: 40 shares
    // at the old price, 60 at the doubled one.
    let funds = 40 * SHARE_PRICE + 60 * SHARE_PRICE * 2;
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.collected.len(), 1);
    assert_eq!(listing.collected[0].funds, funds);
    assert_eq!(listing.collected[0].tax, funds * 300 / 10_000);
}

// --- settled-vault rent recovery ---

#[test]
fn settled_vault_rent_returns_to_the_sponsor() {
    let (mut svm, _admin, _investors) = finalized_property();
    let cranker = funded(&mut svm);
    let tgbp_ata = payment_ata(&listing_vault_pda(0), &tgbp_mint());
    let rent = svm.get_account(&tgbp_ata).unwrap().lamports;
    let before = svm.get_account(&sponsor().pubkey()).unwrap().lamports;

    ok(
        &mut svm,
        close_settled_payment_accounts_ix(&cranker.pubkey(), 0, &[tgbp_mint()]),
        &cranker,
        &[&cranker],
    );

    assert!(svm
        .get_account(&tgbp_ata)
        .map(|a| a.data.is_empty())
        .unwrap_or(true));
    let after = svm.get_account(&sponsor().pubkey()).unwrap().lamports;
    assert_eq!(after - before, rent);
}

#[test]
fn a_post_settlement_donation_sweeps_to_the_treasury() {
    let (mut svm, _admin, _investors) = finalized_property();
    let cranker = funded(&mut svm);
    let vault = listing_vault_pda(0);
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        payment_ata(&vault, &tgbp_mint()),
        &vault,
        5,
    );
    let before = token_balance(&svm, &payment_ata(&treasury(), &tgbp_mint()));

    ok(
        &mut svm,
        close_settled_payment_accounts_ix(&cranker.pubkey(), 0, &[tgbp_mint()]),
        &cranker,
        &[&cranker],
    );

    let after = token_balance(&svm, &payment_ata(&treasury(), &tgbp_mint()));
    assert_eq!(after - before, 5);
}

#[test]
fn an_unsettled_listing_keeps_its_vault_accounts() {
    let (mut svm, _admin, _investors) = build_property(false);
    let cranker = funded(&mut svm);
    fails_with(
        &mut svm,
        close_settled_payment_accounts_ix(&cranker.pubkey(), 0, &[tgbp_mint()]),
        &cranker,
        &[&cranker],
        "PropertyNotFinalized",
    );
}

#[test]
fn a_decoy_vault_account_is_rejected() {
    let (mut svm, _admin, _investors) = finalized_property();
    let cranker = funded(&mut svm);
    let mut ix = close_settled_payment_accounts_ix(&cranker.pubkey(), 0, &[tgbp_mint()]);
    // The triple's vault slot follows the 7 struct accounts.
    ix.accounts[7].pubkey = payment_ata(&cranker.pubkey(), &tgbp_mint());
    fails_with(&mut svm, ix, &cranker, &[&cranker], "WrongTokenAccount");
}

#[test]
fn rerunning_the_settled_close_is_a_no_op() {
    let (mut svm, _admin, _investors) = finalized_property();
    let cranker = funded(&mut svm);
    let ix = close_settled_payment_accounts_ix(&cranker.pubkey(), 0, &[tgbp_mint()]);
    ok(&mut svm, ix.clone(), &cranker, &[&cranker]);
    let before = svm.get_account(&sponsor().pubkey()).unwrap().lamports;
    ok(&mut svm, ix, &cranker, &[&cranker]);
    assert_eq!(
        svm.get_account(&sponsor().pubkey()).unwrap().lamports,
        before
    );
}

#[test]
fn payout_groups_must_match_the_collected_mints() {
    let (mut svm, _admin, developer, _investors, dl, sl, operator) = setup_approved(false);
    let cranker = funded(&mut svm);

    // The payout group names an accepted mint the sale never collected.
    let mut ix = deal_ix(
        &cranker.pubkey(),
        &developer.pubkey(),
        &dl.pubkey(),
        &sl.pubkey(),
        &operator.pubkey(),
    );
    swap_account(&mut ix, tgbp_mint(), gbp6_mint());
    fails_with(&mut svm, ix, &cranker, &[&cranker], "InvalidMint");
}
