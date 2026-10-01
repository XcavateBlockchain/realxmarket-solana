mod common;
use anchor_lang::solana_program::clock::Clock;
use common::*;
use realxhub::state::{HubReservation, PaymentReservation, ReservationSale};

fn sale(id: u64) -> Pubkey {
    address(&[b"reservation_sale", &id.to_le_bytes()])
}
fn reservation(id: u64, buyer: &Pubkey) -> Pubkey {
    address(&[b"hub_reservation", &id.to_le_bytes(), buyer.as_ref()])
}
fn payment_reservation(buyer: &Pubkey) -> Pubkey {
    address(&[b"payment_reservation", ata(buyer, &payment_mint()).as_ref()])
}

fn sale_of(svm: &LiteSVM, id: u64) -> ReservationSale {
    ReservationSale::try_deserialize(&mut &svm.get_account(&sale(id)).unwrap().data[..]).unwrap()
}
fn reservation_of(svm: &LiteSVM, id: u64, buyer: &Pubkey) -> HubReservation {
    HubReservation::try_deserialize(
        &mut &svm.get_account(&reservation(id, buyer)).unwrap().data[..],
    )
    .unwrap()
}
fn promised(svm: &LiteSVM, buyer: &Pubkey) -> u64 {
    PaymentReservation::try_deserialize(
        &mut &svm.get_account(&payment_reservation(buyer)).unwrap().data[..],
    )
    .unwrap()
    .amount
}

fn cancel_ix(buyer: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::CancelReservation { hub_id: id }.data(),
        realxhub::accounts::CancelReservation {
            buyer: *buyer,
            hub: hub(id),
            sale: sale(id),
            reservation: reservation(id, buyer),
            payment_reservation: payment_reservation(buyer),
        }
        .to_account_metas(None),
    )
}

fn open_ix(operator: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::OpenReservations { hub_id: id }.data(),
        realxhub::accounts::OpenReservations {
            operator: *operator,
            hub: hub(id),
            operator_role: region_common::role_pda(operator, Role::RegionalOperator),
            region: region_common::region_pda(1),
            sale: sale(id),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}
fn reserve_ix(buyer: &Pubkey, id: u64, amount: u64, max_cost: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::ReserveTokens {
            hub_id: id,
            amount,
            max_total_cost: max_cost,
        }
        .data(),
        realxhub::accounts::ReserveTokens {
            buyer: *buyer,
            config: config(),
            hub: hub(id),
            buyer_role: region_common::role_pda(buyer, Role::RealEstateInvestor),
            sale: sale(id),
            payment_mint: payment_mint(),
            buyer_payment: ata(buyer, &payment_mint()),
            reservation: reservation(id, buyer),
            payment_reservation: payment_reservation(buyer),
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

fn release_ix(cranker: &Pubkey, buyer: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::ReleaseReservation {
            hub_id: id,
            buyer: *buyer,
        }
        .data(),
        realxhub::accounts::ReleaseReservation {
            cranker: *cranker,
            hub: hub(id),
            sale: sale(id),
            reservation: reservation(id, buyer),
            payment_reservation: payment_reservation(buyer),
        }
        .to_account_metas(None),
    )
}

fn finalize_reservations_ix(cranker: &Pubkey, id: u64) -> Instruction {
    Instruction::new_with_bytes(
        realxhub::id(),
        &realxhub::instruction::FinalizeReservations { hub_id: id }.data(),
        realxhub::accounts::FinalizeReservations {
            cranker: *cranker,
            hub: hub(id),
            sale: sale(id),
        }
        .to_account_metas(None),
    )
}

#[test]
fn expired_reservations_can_be_released_without_the_buyers_signature() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, SUPPLY, SUPPLY * PRICE),
        &f.buyer,
    );
    fails_with(
        &mut f.svm,
        release_ix(&f.verifier.pubkey(), &f.buyer.pubkey(), 0),
        &f.verifier,
        "ReservationStillActive",
    );
    warp(&mut f.svm, 259_199);
    fails_with(
        &mut f.svm,
        release_ix(&f.verifier.pubkey(), &f.buyer.pubkey(), 0),
        &f.verifier,
        "ReservationStillActive",
    );
    warp(&mut f.svm, 1);
    ok(
        &mut f.svm,
        release_ix(&f.verifier.pubkey(), &f.buyer.pubkey(), 0),
        &f.verifier,
    );
    assert_eq!(sale_of(&f.svm, 0).reserved_tokens, 0);
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), 0);
    assert_eq!(reservation_of(&f.svm, 0, &f.buyer.pubkey()).amount, 0);
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    assert_eq!(
        balance(&f.svm, ata(&f.buyer.pubkey(), &payment_mint())),
        PAYMENT_BALANCE
    );
    fails_with(
        &mut f.svm,
        release_ix(&f.verifier.pubkey(), &f.buyer.pubkey(), 0),
        &f.verifier,
        "EmptyPosition",
    );
}

#[test]
fn unpaid_campaign_timeout_returns_the_recorded_bond_and_releases_promises() {
    for fully_reserved in [false, true] {
        let mut f = setup();
        reach_listed(&mut f, 0);
        ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
        let amount = if fully_reserved { SUPPLY } else { 4 };
        ok(
            &mut f.svm,
            reserve_ix(&f.buyer.pubkey(), 0, amount, amount * PRICE),
            &f.buyer,
        );
        fails_with(
            &mut f.svm,
            finalize_reservations_ix(&f.verifier.pubkey(), 0),
            &f.verifier,
            "ReservationStillActive",
        );
        warp(&mut f.svm, if fully_reserved { 259_200 } else { DURATION });
        ok(
            &mut f.svm,
            finalize_reservations_ix(&f.verifier.pubkey(), 0),
            &f.verifier,
        );
        assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Failed);
        let before = balance(&f.svm, region_common::token_acc(&f.operator.pubkey()));
        ok(
            &mut f.svm,
            bond_refund_ix(&f.operator.pubkey(), 0),
            &f.operator,
        );
        assert_eq!(
            balance(&f.svm, region_common::token_acc(&f.operator.pubkey())),
            before + BOND
        );
        assert_eq!(balance(&f.svm, bond_vault(0)), 0);
        ok(
            &mut f.svm,
            release_ix(&f.verifier.pubkey(), &f.buyer.pubkey(), 0),
            &f.verifier,
        );
        assert_eq!(promised(&f.svm, &f.buyer.pubkey()), 0);
        fails_with(
            &mut f.svm,
            finalize_reservations_ix(&f.verifier.pubkey(), 0),
            &f.verifier,
            "InvalidStatus",
        );
        fails_with(
            &mut f.svm,
            bond_refund_ix(&f.operator.pubkey(), 0),
            &f.operator,
            "BondAlreadyRefunded",
        );
    }
}

#[test]
fn opening_reservations_requires_a_current_compliant_operator() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    region_common::set_permission(
        &mut f.svm,
        &f.authority,
        &f.operator.pubkey(),
        Role::RegionalOperator,
        false,
    );
    fails_with(
        &mut f.svm,
        open_ix(&f.operator.pubkey(), 0),
        &f.operator,
        "NotCompliant",
    );
    assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Listed);
    assert!(f.svm.get_account(&sale(0)).is_none());
}

#[test]
fn unpaid_reservations_cannot_mix_with_existing_paid_purchases() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 0, 2, PRICE * 2),
        &f.buyer,
    );
    fails_with(
        &mut f.svm,
        open_ix(&f.operator.pubkey(), 0),
        &f.operator,
        "PurchasesOutstanding",
    );
    assert!(f.svm.get_account(&sale(0)).is_none());
    assert_eq!(balance(&f.svm, payment_vault(0)), PRICE * 2);
    warp(&mut f.svm, DURATION);
    ok(
        &mut f.svm,
        finalize_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
    );
    ok(&mut f.svm, refund_ix(&f.buyer.pubkey(), 0), &f.buyer);
    assert_eq!(
        balance(&f.svm, ata(&f.buyer.pubkey(), &payment_mint())),
        PAYMENT_BALANCE
    );

    reach_listed(&mut f, 1);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 1), &f.operator);
    fails_with(
        &mut f.svm,
        buy_ix(&f.buyer.pubkey(), 1, 1, PRICE),
        &f.buyer,
        "InvalidStatus",
    );
    assert_eq!(balance(&f.svm, payment_vault(1)), 0);
    assert!(f.svm.get_account(&position(1, &f.buyer.pubkey())).is_none());
}

#[test]
fn promises_across_hubs_share_the_payment_accounts_balance_limit() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    reach_listed(&mut f, 1);
    for id in [0, 1] {
        ok(&mut f.svm, open_ix(&f.operator.pubkey(), id), &f.operator);
    }
    set_payment(&mut f.svm, &f.buyer.pubkey(), PRICE * 5);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 3, PRICE * 3),
        &f.buyer,
    );
    fails_with(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 1, 3, PRICE * 3),
        &f.buyer,
        "ReservationBalanceTooLow",
    );
    assert!(f
        .svm
        .get_account(&reservation(1, &f.buyer.pubkey()))
        .is_none());
    assert_eq!(sale_of(&f.svm, 1).reserved_tokens, 0);
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), PRICE * 3);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 1, 2, PRICE * 2),
        &f.buyer,
    );
    ok(&mut f.svm, cancel_ix(&f.buyer.pubkey(), 0), &f.buyer);
    assert_eq!(sale_of(&f.svm, 1).reserved_tokens, 2);
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), PRICE * 2);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 3, PRICE * 3),
        &f.buyer,
    );
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), PRICE * 5);
    assert_eq!(
        balance(&f.svm, ata(&f.buyer.pubkey(), &payment_mint())),
        PRICE * 5
    );
}

#[test]
fn rejected_quantities_and_quotes_leave_no_reservation_or_payment() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    for (amount, cap, error) in [
        (0, PRICE, "InvalidAmount"),
        (SUPPLY + 1, u64::MAX, "InvalidAmount"),
        (1, PRICE - 1, "CostTooHigh"),
    ] {
        fails_with(
            &mut f.svm,
            reserve_ix(&f.buyer.pubkey(), 0, amount, cap),
            &f.buyer,
            error,
        );
    }
    set_payment(&mut f.svm, &f.buyer.pubkey(), PRICE - 1);
    fails_with(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "ReservationBalanceTooLow",
    );
    assert!(f
        .svm
        .get_account(&reservation(0, &f.buyer.pubkey()))
        .is_none());
    assert!(f
        .svm
        .get_account(&payment_reservation(&f.buyer.pubkey()))
        .is_none());
    assert_eq!(sale_of(&f.svm, 0).reserved_tokens, 0);
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
}

#[test]
fn full_round_cannot_be_cancelled_or_reset_by_another_reservation() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, SUPPLY, PRICE * SUPPLY),
        &f.buyer,
    );
    let deadline = sale_of(&f.svm, 0).claim_deadline;
    fails_with(
        &mut f.svm,
        cancel_ix(&f.buyer.pubkey(), 0),
        &f.buyer,
        "InvalidStatus",
    );
    fails_with(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "InvalidStatus",
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
        finalize_reservations_ix(&f.verifier.pubkey(), 0),
        &f.verifier,
        "ReservationStillActive",
    );
    assert_eq!(sale_of(&f.svm, 0).claim_deadline, deadline);
    assert_eq!(sale_of(&f.svm, 0).reserved_tokens, SUPPLY);
}

#[test]
fn exact_sale_deadline_blocks_reservation_and_allows_cleanup() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
    );
    warp(&mut f.svm, DURATION);
    fails_with(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "SaleExpired",
    );
    ok(
        &mut f.svm,
        release_ix(&f.verifier.pubkey(), &f.buyer.pubkey(), 0),
        &f.verifier,
    );
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), 0);
}

#[test]
fn reservation_entry_requires_compliance_but_expiry_cleanup_does_not() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
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
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "NotCompliant",
    );
    warp(&mut f.svm, DURATION);
    ok(
        &mut f.svm,
        release_ix(&f.verifier.pubkey(), &f.buyer.pubkey(), 0),
        &f.verifier,
    );
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), 0);
}

#[test]
fn reservation_and_payment_records_cannot_be_substituted_across_hubs_or_buyers() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    reach_listed(&mut f, 1);
    for id in [0, 1] {
        ok(&mut f.svm, open_ix(&f.operator.pubkey(), id), &f.operator);
        ok(
            &mut f.svm,
            reserve_ix(&f.buyer.pubkey(), id, 1, PRICE),
            &f.buyer,
        );
    }
    let mut wrong_sale = cancel_ix(&f.buyer.pubkey(), 0);
    wrong_sale.accounts[2].pubkey = sale(1);
    fails_with(&mut f.svm, wrong_sale, &f.buyer, "ConstraintSeeds");
    let mut wrong_position = cancel_ix(&f.buyer.pubkey(), 0);
    wrong_position.accounts[3].pubkey = reservation(1, &f.buyer.pubkey());
    fails_with(&mut f.svm, wrong_position, &f.buyer, "ConstraintSeeds");
    let mut wrong_buyer = cancel_ix(&f.verifier.pubkey(), 0);
    wrong_buyer.accounts[3].pubkey = reservation(0, &f.buyer.pubkey());
    wrong_buyer.accounts[4].pubkey = payment_reservation(&f.buyer.pubkey());
    fails_with(&mut f.svm, wrong_buyer, &f.verifier, "ConstraintSeeds");
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), PRICE * 2);
    assert_eq!(sale_of(&f.svm, 0).reserved_tokens, 1);
    assert_eq!(sale_of(&f.svm, 1).reserved_tokens, 1);
}

#[test]
fn a_reservation_stays_bound_to_its_original_payment_account() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
    );
    let another_account = Pubkey::new_unique();
    let original = f
        .svm
        .get_account(&ata(&f.buyer.pubkey(), &payment_mint()))
        .unwrap();
    f.svm.set_account(another_account, original).unwrap();
    let mut changed_account = reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE);
    changed_account.accounts[6].pubkey = another_account;
    let another_ledger = address(&[b"payment_reservation", another_account.as_ref()]);
    changed_account.accounts[8].pubkey = another_ledger;
    fails_with(
        &mut f.svm,
        changed_account,
        &f.buyer,
        "ReservationAccountMismatch",
    );
    assert!(f.svm.get_account(&another_ledger).is_none());
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), PRICE);
    assert_eq!(sale_of(&f.svm, 0).reserved_tokens, 1);
}

#[test]
fn several_buyers_share_the_fixed_supply_without_resetting_the_window() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
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
        reserve_ix(&f.buyer.pubkey(), 0, 3, PRICE * 3),
        &f.buyer,
    );
    fails_with(
        &mut f.svm,
        reserve_ix(&f.verifier.pubkey(), 0, 8, PRICE * 8),
        &f.verifier,
        "InvalidAmount",
    );
    ok(
        &mut f.svm,
        reserve_ix(&f.verifier.pubkey(), 0, 7, PRICE * 7),
        &f.verifier,
    );
    assert_eq!(sale_of(&f.svm, 0).reserved_tokens, SUPPLY);
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), PRICE * 3);
    assert_eq!(promised(&f.svm, &f.verifier.pubkey()), PRICE * 7);
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    let deadline = sale_of(&f.svm, 0).claim_deadline;
    fails_with(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "InvalidStatus",
    );
    assert_eq!(sale_of(&f.svm, 0).claim_deadline, deadline);
}

#[test]
fn spending_promised_tokens_does_not_prevent_cancellation() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
    );
    // Simulate an unrelated wallet transfer after the initial balance check.
    set_payment(&mut f.svm, &f.buyer.pubkey(), 0);
    fails_with(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "ReservationBalanceTooLow",
    );
    ok(&mut f.svm, cancel_ix(&f.buyer.pubkey(), 0), &f.buyer);
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), 0);
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    assert_eq!(balance(&f.svm, ata(&f.buyer.pubkey(), &payment_mint())), 0);
}

#[test]
fn reserving_tokens_does_not_collect_payment_or_deliver_tokens() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 4, 800_000),
        &f.buyer,
    );

    assert_eq!(
        balance(&f.svm, ata(&f.buyer.pubkey(), &payment_mint())),
        PAYMENT_BALANCE
    );
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    assert_eq!(balance(&f.svm, token_vault(0)), 10);
    assert_eq!(hub_of(&f.svm, 0).total_paid, 0);
    assert_eq!(hub_of(&f.svm, 0).tokens_sold, 0);
    assert!(f
        .svm
        .get_account(&reservation(0, &f.buyer.pubkey()))
        .is_some());
    assert!(f.svm.get_account(&position(0, &f.buyer.pubkey())).is_none());
}

#[test]
fn cancelling_an_unpaid_reservation_releases_the_quote_without_a_refund() {
    let mut f = setup();
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 4, 800_000),
        &f.buyer,
    );
    region_common::set_permission(
        &mut f.svm,
        &f.authority,
        &f.buyer.pubkey(),
        Role::RealEstateInvestor,
        false,
    );
    ok(&mut f.svm, cancel_ix(&f.buyer.pubkey(), 0), &f.buyer);

    assert_eq!(sale_of(&f.svm, 0).reserved_tokens, 0);
    assert_eq!(reservation_of(&f.svm, 0, &f.buyer.pubkey()).amount, 0);
    assert_eq!(promised(&f.svm, &f.buyer.pubkey()), 0);
    assert_eq!(
        balance(&f.svm, ata(&f.buyer.pubkey(), &payment_mint())),
        PAYMENT_BALANCE
    );
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
    fails_with(
        &mut f.svm,
        cancel_ix(&f.buyer.pubkey(), 0),
        &f.buyer,
        "EmptyPosition",
    );
}

#[test]
fn last_reservation_opens_three_full_days_near_the_sale_deadline() {
    let mut f = setup();
    let mut clock = f.svm.get_sysvar::<Clock>();
    clock.unix_timestamp = 1_000;
    f.svm.set_sysvar(&clock);
    reach_listed(&mut f, 0);
    ok(&mut f.svm, open_ix(&f.operator.pubkey(), 0), &f.operator);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 4, 800_000),
        &f.buyer,
    );
    assert_eq!(sale_of(&f.svm, 0).claim_deadline, 0);
    warp(&mut f.svm, 99);
    ok(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 6, 1_200_000),
        &f.buyer,
    );
    let full = sale_of(&f.svm, 0);
    assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Claiming);
    assert_eq!(full.claim_started_at, 1_099);
    assert_eq!(full.claim_deadline, 260_299);
    assert_eq!(
        reservation_of(&f.svm, 0, &f.buyer.pubkey()).quoted_payment,
        2_000_000
    );
    fails_with(
        &mut f.svm,
        reserve_ix(&f.buyer.pubkey(), 0, 1, PRICE),
        &f.buyer,
        "InvalidStatus",
    );
    assert_eq!(sale_of(&f.svm, 0).claim_deadline, 260_299);
    assert_eq!(balance(&f.svm, payment_vault(0)), 0);
}
