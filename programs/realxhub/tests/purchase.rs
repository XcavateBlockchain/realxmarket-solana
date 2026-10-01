mod common;
use common::*;

#[test]
fn multiple_buyers_can_only_claim_their_own_allocations() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    region_common::ok(
        &mut f.svm,
        region_common::roles_assign_ix(
            &f.admin.pubkey(),
            &f.verifier.pubkey(),
            Role::RealEstateInvestor,
        ),
        &f.admin,
        &[&f.admin],
    );
    set_payment(&mut f.svm, &f.verifier.pubkey(), PAYMENT_BALANCE);
    ok(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 3, PRICE * 3),
        &f.buyer,
    );
    ok(
        &mut f.svm,
        buy_ix(&f.verifier.pubkey(), 0, 7, PRICE * 7),
        &f.verifier,
    );
    let mut another_position = claim_ix(&f.buyer.pubkey(), 0);
    another_position.accounts[2].pubkey = position(0, &f.verifier.pubkey());
    assert!(process(&mut f.svm, another_position, &f.buyer).is_err());
    assert!(!position_of(&f.svm, 0, &f.verifier.pubkey()).settled);
    ok(&mut f.svm, claim_ix(&f.buyer.pubkey(), 0), &f.buyer);
    ok(&mut f.svm, claim_ix(&f.verifier.pubkey(), 0), &f.verifier);
    assert_eq!(balance(&f.svm, ata(&f.buyer.pubkey(), &hub_mint(0))), 3);
    assert_eq!(balance(&f.svm, ata(&f.verifier.pubkey(), &hub_mint(0))), 7);
    assert_eq!(balance(&f.svm, token_vault(0)), 0);
    assert_eq!(hub_of(&f.svm, 0).tokens_claimed, SUPPLY);
}

#[test]
fn complete_journey_escrows_payment_then_delivers_fixed_supply() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 4, PRICE * 4),
        &f.buyer,
    );
    assert_eq!(balance(&f.svm, payment_vault(0)), PRICE * 4);
    assert_eq!(balance(&f.svm, token_vault(0)), SUPPLY);
    assert_eq!(position_of(&f.svm, 0, &f.buyer.pubkey()).amount, 4);
    fails_with(
        &mut f.svm,
        claim_ix(&f.buyer.pubkey(), 0),
        &f.buyer,
        "InvalidStatus",
    );
    ok(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 6, PRICE * 6),
        &f.buyer,
    );
    assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Funded);
    assert_eq!(
        position_of(&f.svm, 0, &f.buyer.pubkey()).paid,
        PRICE * SUPPLY
    );
    ok(&mut f.svm, claim_ix(&f.buyer.pubkey(), 0), &f.buyer);
    assert_eq!(
        balance(&f.svm, ata(&f.buyer.pubkey(), &hub_mint(0))),
        SUPPLY
    );
    assert_eq!(balance(&f.svm, token_vault(0)), 0);
    assert_eq!(hub_of(&f.svm, 0).tokens_claimed, SUPPLY);
    assert_eq!(balance(&f.svm, payment_vault(0)), PRICE * SUPPLY);
    assert_eq!(balance(&f.svm, bond_vault(0)), BOND);
    fails_with(
        &mut f.svm,
        claim_ix(&f.buyer.pubkey(), 0),
        &f.buyer,
        "AlreadySettled",
    );
    fails_with(
        &mut f.svm,
        refund_ix(&f.buyer.pubkey(), 0),
        &f.buyer,
        "InvalidStatus",
    );
    fails_with(
        &mut f.svm,
        bond_refund_ix(&f.operator.pubkey(), 0),
        &f.operator,
        "InvalidStatus",
    );
}

#[test]
fn failed_sale_returns_payments_and_bond_once() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 3, PRICE * 3),
        &f.buyer,
    );
    fails_with(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
        "SaleStillOpen",
    );
    warp(&mut f.svm, DURATION);
    ok(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
    );
    assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Failed);
    fails_with(
        &mut f.svm,
        claim_ix(&f.buyer.pubkey(), 0),
        &f.buyer,
        "InvalidStatus",
    );
    ok(&mut f.svm, refund_ix(&f.buyer.pubkey(), 0), &f.buyer);
    assert_eq!(
        balance(&f.svm, ata(&f.buyer.pubkey(), &payment_mint())),
        PAYMENT_BALANCE
    );
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    assert_eq!(hub_of(&f.svm, 0).total_refunded, PRICE * 3);
    assert_eq!(balance(&f.svm, token_vault(0)), SUPPLY);
    ok(
        &mut f.svm,
        bond_refund_ix(&f.operator.pubkey(), 0),
        &f.operator,
    );
    assert_eq!(balance(&f.svm, bond_vault(0)), 0);
    fails_with(
        &mut f.svm,
        refund_ix(&f.buyer.pubkey(), 0),
        &f.buyer,
        "AlreadySettled",
    );
    fails_with(
        &mut f.svm,
        bond_refund_ix(&f.operator.pubkey(), 0),
        &f.operator,
        "BondAlreadyRefunded",
    );
    fails_with(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
        "InvalidStatus",
    );
}

#[test]
fn purchases_require_investor_role_and_compliance() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    fails_with(
        &mut f.svm,
        buy_ix(&f.verifier.pubkey(), 0, 1, PRICE),
        &f.verifier,
        "AccountNotInitialized",
    );
    region_common::set_permission(
        &mut f.svm,
        &f.authority,
        &f.buyer.pubkey(),
        Role::RealEstateInvestor,
        false,
    );
    fails_with(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "NotCompliant",
    );
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
}

#[test]
fn cost_caps_and_supply_limits_leave_no_partial_purchase() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    fails_with(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 1, PRICE - 1),
        &f.buyer,
        "CostTooHigh",
    );
    fails_with(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 0, PRICE),
        &f.buyer,
        "InvalidAmount",
    );
    fails_with(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, SUPPLY + 1, u64::MAX),
        &f.buyer,
        "InvalidAmount",
    );
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    assert_eq!(hub_of(&f.svm, 0).tokens_sold, 0);
    assert!(f.svm.get_account(&position(0, &f.buyer.pubkey())).is_none());
}

#[test]
fn insufficient_payment_rolls_back_position_and_hub_state() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    set_payment(&mut f.svm, &f.buyer.pubkey(), PRICE - 1);
    fails_with(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "insufficient funds",
    );
    assert_eq!(hub_of(&f.svm, 0).total_paid, 0);
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    assert!(f.svm.get_account(&position(0, &f.buyer.pubkey())).is_none());
}

#[test]
fn exact_deadline_blocks_purchases_and_opens_refunds() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    warp(&mut f.svm, DURATION);
    fails_with(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "SaleExpired",
    );
    ok(&mut f.svm, finalize_ix(&f.buyer.pubkey(), 0), &f.buyer);
    assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Failed);
}

#[test]
fn sellout_cannot_be_expired_or_overpurchased() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, SUPPLY, PRICE * SUPPLY),
        &f.buyer,
    );
    warp(&mut f.svm, DURATION);
    fails_with(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
        "InvalidStatus",
    );
    fails_with(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "InvalidStatus",
    );
    ok(&mut f.svm, claim_ix(&f.buyer.pubkey(), 0), &f.buyer);
}

#[test]
fn revocation_does_not_strand_existing_buyer_or_operator_refunds() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 2, PRICE * 2),
        &f.buyer,
    );
    region_common::set_permission(
        &mut f.svm,
        &f.authority,
        &f.buyer.pubkey(),
        Role::RealEstateInvestor,
        false,
    );
    region_common::set_permission(
        &mut f.svm,
        &f.authority,
        &f.operator.pubkey(),
        Role::RegionalOperator,
        false,
    );
    warp(&mut f.svm, DURATION);
    ok(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
    );
    ok(&mut f.svm, refund_ix(&f.buyer.pubkey(), 0), &f.buyer);
    ok(
        &mut f.svm,
        bond_refund_ix(&f.operator.pubkey(), 0),
        &f.operator,
    );
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    assert_eq!(balance(&f.svm, bond_vault(0)), 0);
}

#[test]
fn refund_recreates_a_closed_buyer_token_account() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    set_payment(&mut f.svm, &f.buyer.pubkey(), PRICE);
    ok(&mut f.svm, buy_ix(&f.buyer.pubkey(), 0, 1, PRICE), &f.buyer);
    let token = ata(&f.buyer.pubkey(), &payment_mint());
    let close = anchor_spl::token::spl_token::instruction::close_account(
        &TOKEN,
        &token,
        &f.buyer.pubkey(),
        &f.buyer.pubkey(),
        &[],
    )
    .unwrap();
    ok(&mut f.svm, close, &f.buyer);
    warp(&mut f.svm, DURATION);
    ok(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
    );
    ok(&mut f.svm, refund_ix(&f.buyer.pubkey(), 0), &f.buyer);
    assert_eq!(balance(&f.svm, token), PRICE);
}

#[test]
fn hubs_cannot_spend_each_others_payment_vaults() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    reach_listed(&mut f, 1);
    ok(&mut f.svm, buy_ix(&f.buyer.pubkey(), 0, 1, PRICE), &f.buyer);
    let mut wrong_vault = buy_ix(&f.buyer.pubkey(), 1, 1, PRICE);
    wrong_vault.accounts[6].pubkey = payment_vault(0);
    fails_with(&mut f.svm, wrong_vault, &f.buyer, "ConstraintSeeds");
    assert_eq!(balance(&f.svm, payment_vault(0)), PRICE);
    assert_eq!(balance(&f.svm, payment_vault(1)), 0);
    ok(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 1, SUPPLY, PRICE * SUPPLY),
        &f.buyer,
    );
    warp(&mut f.svm, DURATION);
    ok(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
    );
    ok(&mut f.svm, refund_ix(&f.buyer.pubkey(), 0), &f.buyer);
    assert_eq!(balance(&f.svm, payment_vault(1)), PRICE * SUPPLY);
    assert_eq!(hub_of(&f.svm, 1).status, HubStatus::Funded);
}

#[test]
fn wrong_payment_mint_and_refund_recipient_are_rejected() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    let mut wrong_mint = buy_ix(&f.buyer.pubkey(), 0, 1, PRICE);
    wrong_mint.accounts[4].pubkey = region_common::xcav_mint();
    fails_with(&mut f.svm, wrong_mint, &f.buyer, "InvalidMint");
    ok(&mut f.svm, buy_ix(&f.buyer.pubkey(), 0, 1, PRICE), &f.buyer);
    set_payment(&mut f.svm, &f.verifier.pubkey(), 0);
    warp(&mut f.svm, DURATION);
    ok(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
    );
    let mut wrong_recipient = refund_ix(&f.buyer.pubkey(), 0);
    wrong_recipient.accounts[6].pubkey = ata(&f.verifier.pubkey(), &payment_mint());
    assert!(process(&mut f.svm, wrong_recipient, &f.buyer).is_err());
    assert_eq!(balance(&f.svm, payment_vault(0)), PRICE);
    assert_eq!(
        balance(&f.svm, ata(&f.verifier.pubkey(), &payment_mint())),
        0
    );
}
