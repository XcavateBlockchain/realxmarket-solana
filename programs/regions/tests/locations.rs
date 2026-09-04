//! Locations and region settings: postcode registration with its deposit,
//! the listing-duration, tax and fee setters, and how location deposits
//! follow the seat.

mod common;
use common::*;

use regions::state::Location;

fn created_region(svm: &mut LiteSVM, operator: &Keypair, authority: &Keypair) {
    reach_created(svm, operator, authority);
}

#[test]
fn create_location_works() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    let vault_before = vault_balance(&svm);
    let operator_before = xcav_balance(&svm, &operator.pubkey());

    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );

    let acc = svm.get_account(&location_pda(1, b"SW1A1AA")).unwrap();
    let location = Location::try_deserialize(&mut &acc.data[..]).unwrap();
    assert_eq!(location.region_id, 1);
    assert_eq!(location.postcode, b"SW1A1AA");

    // The deposit moved into the vault and joined the region's collateral.
    assert_eq!(vault_balance(&svm) - vault_before, LOCATION_DEPOSIT);
    assert_eq!(
        operator_before - xcav_balance(&svm, &operator.pubkey()),
        LOCATION_DEPOSIT
    );
    let region = region_of(&svm, 1);
    assert_eq!(region.collateral, DEPOSIT + LOCATION_DEPOSIT);
    assert_eq!(region.location_count, 1);
}

#[test]
fn create_location_twice_fails() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );
    // The location PDA already exists, so init refuses the duplicate.
    fails_with(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
        "already in use",
    );
}

#[test]
fn create_location_requires_region_owner() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    // Another compliant operator, but not this region's owner.
    let other = new_operator(&mut svm, &authority);
    fails_with(
        &mut svm,
        create_location_ix(&other.pubkey(), 1, b"SW1A1AA"),
        &other,
        &[&other],
        "NotRegionOwner",
    );
}

#[test]
fn create_location_rejects_bad_postcodes() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    let cases: [&[u8]; 4] = [b"", b"sw1a1aa", b"SW1A 1AA", b"ABCDEFGHIJK"];
    for postcode in cases {
        fails_with(
            &mut svm,
            create_location_ix(&operator.pubkey(), 1, postcode),
            &operator,
            &[&operator],
            "InvalidPostcode",
        );
    }
}

// Regions checks role possession only. Compliance gates the marketplace's
// investor-fund flows, not region administration, so a blocked operator keeps
// running their region.
#[test]
fn create_location_ignores_compliance() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    block_compliance(&mut svm, &authority, &operator.pubkey());
    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );
}

#[test]
fn adjust_listing_duration_works() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    assert_eq!(region_of(&svm, 1).listing_duration, LISTING_DURATION);

    ok(
        &mut svm,
        adjust_duration_ix(&operator.pubkey(), 1, 200_000),
        &operator,
        &[&operator],
    );
    assert_eq!(region_of(&svm, 1).listing_duration, 200_000);
}

#[test]
fn adjust_listing_duration_validates_bounds() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    // default max_listing_duration is 1_000_000
    for bad in [0i64, 1_000_001] {
        fails_with(
            &mut svm,
            adjust_duration_ix(&operator.pubkey(), 1, bad),
            &operator,
            &[&operator],
            "InvalidListingDuration",
        );
    }
}

#[test]
fn adjust_tax_works_and_validates() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    assert_eq!(region_of(&svm, 1).tax_bps, TAX_BPS);

    ok(
        &mut svm,
        adjust_tax_ix(&operator.pubkey(), 1, 500),
        &operator,
        &[&operator],
    );
    assert_eq!(region_of(&svm, 1).tax_bps, 500);

    // default max_tax_bps is 1_000
    fails_with(
        &mut svm,
        adjust_tax_ix(&operator.pubkey(), 1, 1_001),
        &operator,
        &[&operator],
        "TaxTooHigh",
    );
}

#[test]
fn adjust_fees_works_and_validates() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    let region = region_of(&svm, 1);
    assert_eq!(
        (region.seller_fee_bps, region.buyer_fee_bps),
        (SELLER_FEE_BPS, BUYER_FEE_BPS)
    );

    ok(
        &mut svm,
        adjust_fees_ix(&operator.pubkey(), 1, 500, 50),
        &operator,
        &[&operator],
    );
    let region = region_of(&svm, 1);
    assert_eq!((region.seller_fee_bps, region.buyer_fee_bps), (500, 50));

    // default max_fee_bps is 1_000, checked on each side
    for (seller, buyer) in [(1_001, 50), (500, 1_001)] {
        fails_with(
            &mut svm,
            adjust_fees_ix(&operator.pubkey(), 1, seller, buyer),
            &operator,
            &[&operator],
            "FeeTooHigh",
        );
    }
}

#[test]
fn adjust_setters_require_region_owner() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    let other = new_operator(&mut svm, &authority);
    fails_with(
        &mut svm,
        adjust_duration_ix(&other.pubkey(), 1, 200_000),
        &other,
        &[&other],
        "NotRegionOwner",
    );
    fails_with(
        &mut svm,
        adjust_tax_ix(&other.pubkey(), 1, 500),
        &other,
        &[&other],
        "NotRegionOwner",
    );
    fails_with(
        &mut svm,
        adjust_fees_ix(&other.pubkey(), 1, 500, 50),
        &other,
        &[&other],
        "NotRegionOwner",
    );
}

#[test]
fn create_region_validates_initial_settings() {
    let (mut svm, operator, authority) = setup();
    reach_passed(&mut svm, &operator, &authority);
    fails_with(
        &mut svm,
        create_region_ix_with(
            &operator.pubkey(),
            1,
            0,
            TAX_BPS,
            SELLER_FEE_BPS,
            BUYER_FEE_BPS,
        ),
        &operator,
        &[&operator],
        "InvalidListingDuration",
    );
    fails_with(
        &mut svm,
        create_region_ix_with(
            &operator.pubkey(),
            1,
            LISTING_DURATION,
            1_001,
            SELLER_FEE_BPS,
            BUYER_FEE_BPS,
        ),
        &operator,
        &[&operator],
        "TaxTooHigh",
    );
    // Either fee above the configured maximum is rejected.
    fails_with(
        &mut svm,
        create_region_ix_with(
            &operator.pubkey(),
            1,
            LISTING_DURATION,
            TAX_BPS,
            1_001,
            BUYER_FEE_BPS,
        ),
        &operator,
        &[&operator],
        "FeeTooHigh",
    );
    fails_with(
        &mut svm,
        create_region_ix_with(
            &operator.pubkey(),
            1,
            LISTING_DURATION,
            TAX_BPS,
            SELLER_FEE_BPS,
            1_001,
        ),
        &operator,
        &[&operator],
        "FeeTooHigh",
    );
    // Valid settings still claim the passed region.
    ok(
        &mut svm,
        create_region_ix_with(
            &operator.pubkey(),
            1,
            LISTING_DURATION,
            TAX_BPS,
            SELLER_FEE_BPS,
            BUYER_FEE_BPS,
        ),
        &operator,
        &[&operator],
    );
}

#[test]
fn create_location_rejects_deposit_above_cap() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    fails_with(
        &mut svm,
        create_location_ix_capped(&operator.pubkey(), 1, b"SW1A1AA", LOCATION_DEPOSIT - 1),
        &operator,
        &[&operator],
        "DepositTooHigh",
    );
}

// A lame-duck operator must not reprice the seat during the takeover window.
#[test]
fn create_location_blocked_when_seat_open() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    ok(
        &mut svm,
        resign_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );
    warp(&mut svm, 6_000);
    fails_with(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
        "SeatOpen",
    );
}

#[test]
fn remove_location_returns_recorded_deposit() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );

    // The configured deposit doubles after the operator already paid theirs;
    // removal still refunds what was actually locked.
    let mut params = default_params();
    params.location_deposit = 2 * LOCATION_DEPOSIT;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    let before = xcav_balance(&svm, &operator.pubkey());
    ok(
        &mut svm,
        remove_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );
    assert_eq!(
        xcav_balance(&svm, &operator.pubkey()) - before,
        LOCATION_DEPOSIT
    );
    let region = region_of(&svm, 1);
    assert_eq!(region.collateral, DEPOSIT);
    assert_eq!(region.location_count, 0);
    assert!(svm
        .get_account(&location_pda(1, b"SW1A1AA"))
        .is_none_or(|a| a.data.is_empty()));

    // The postcode can be registered again, now at the new rate.
    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );
    assert_eq!(
        region_of(&svm, 1).collateral,
        DEPOSIT + 2 * LOCATION_DEPOSIT
    );
}

#[test]
fn remove_location_requires_owner() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );

    let other = new_operator(&mut svm, &authority);
    fails_with(
        &mut svm,
        remove_location_ix(&other.pubkey(), 1, b"SW1A1AA"),
        &other,
        &[&other],
        "NotRegionOwner",
    );
}

#[test]
fn remove_location_blocked_when_seat_open() {
    let (mut svm, operator, authority) = setup();
    created_region(&mut svm, &operator, &authority);
    ok(
        &mut svm,
        create_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
    );
    ok(
        &mut svm,
        resign_ix(&operator.pubkey(), 1),
        &operator,
        &[&operator],
    );
    warp(&mut svm, 6_000);
    fails_with(
        &mut svm,
        remove_location_ix(&operator.pubkey(), 1, b"SW1A1AA"),
        &operator,
        &[&operator],
        "SeatOpen",
    );
}
