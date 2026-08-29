//! Primary listing: creation with its deposit and snapshots, the compliance
//! gate, and price updates.

mod common;
use common::*;

use anchor_lang::InstructionData;
use marketplace::state::ListingStatus;

/// Full setup plus region 1 with the default postcode registered, and a
/// compliant developer.
fn setup_listing() -> (LiteSVM, Keypair, Keypair, Keypair) {
    let (mut svm, admin, authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    (svm, admin, authority, developer)
}

#[test]
fn list_property_creates_listing_and_locks_deposit() {
    let (mut svm, _admin, _authority, developer) = setup_listing();

    let before = xcav_balance(&svm, &developer.pubkey());
    let vault_before = vault_balance(&svm);
    let now = svm.get_sysvar::<Clock>().unix_timestamp;
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    let listing = listing_of(&svm, 0);
    assert_eq!(listing.listing_id, 0);
    assert_eq!(listing.developer, developer.pubkey());
    assert_eq!(listing.share_price, SHARE_PRICE);
    assert_eq!(listing.listed_share_amount, SHARE_AMOUNT);
    assert_eq!(listing.sold_share_amount, 0);
    // Snapshots from the seeded region: 300 bps tax, 100 bps fees each
    // side, 100_000s duration.
    assert_eq!(listing.tax_bps, 300);
    assert_eq!(listing.seller_fee_bps, 100);
    assert_eq!(listing.buyer_fee_bps, 100);
    assert_eq!(listing.listing_expiry, now + LISTING_DURATION);
    assert_eq!(listing.deposit, LISTING_DEPOSIT);
    assert_eq!(listing.status, ListingStatus::PendingAssets);

    let property = property_of(&svm, 0);
    assert_eq!(property.region_id, 1);
    assert_eq!(property.location, POSTCODE.to_vec());
    assert_eq!(property.share_amount, SHARE_AMOUNT);
    // The Core asset and share mint don't exist until init_property_assets.
    assert_eq!(property.metadata_uri, "");
    assert_eq!(property.share_mint, Pubkey::default());

    assert_eq!(
        before - xcav_balance(&svm, &developer.pubkey()),
        LISTING_DEPOSIT
    );
    assert_eq!(vault_balance(&svm), vault_before + LISTING_DEPOSIT);
    assert_eq!(config_of(&svm).next_listing_id, 1);
}

#[test]
fn second_listing_gets_next_id() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 1),
        &developer,
        &[&developer],
    );

    assert_eq!(listing_of(&svm, 1).listing_id, 1);
    assert_eq!(config_of(&svm).next_listing_id, 2);
}

#[test]
fn list_requires_developer_role() {
    let (mut svm, _admin, _authority, _developer) = setup_listing();
    let stranger = actor(&mut svm);
    fails_with(
        &mut svm,
        list_ix(&stranger.pubkey(), 0),
        &stranger,
        &[&stranger],
        "AccountNotInitialized",
    );
}

// list_property starts the flow that takes investor funds, so it is one of the
// calls gated on the compliance flag, not just role possession.
#[test]
fn list_requires_a_compliant_developer() {
    let (mut svm, admin, _authority, developer) = setup_listing();
    set_compliance(&mut svm, &admin, &developer.pubkey(), false);
    fails_with(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
        "NotCompliant",
    );

    // Restored compliance lists fine again.
    set_compliance(&mut svm, &admin, &developer.pubkey(), true);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
}

#[test]
fn list_requires_registered_location() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    fails_with(
        &mut svm,
        list_property_ix(
            &developer.pubkey(),
            0,
            1,
            b"UNKNOWN1",
            SHARE_PRICE,
            SHARE_AMOUNT,
        ),
        &developer,
        &[&developer],
        "AccountNotInitialized",
    );
}

#[test]
fn list_rejects_share_amount_out_of_bounds() {
    let (mut svm, _admin, _authority, developer) = setup_listing();

    // Zero shares.
    fails_with(
        &mut svm,
        list_property_ix(&developer.pubkey(), 0, 1, POSTCODE, SHARE_PRICE, 0),
        &developer,
        &[&developer],
        "InvalidShareAmount",
    );
    // Above the configured maximum (100).
    fails_with(
        &mut svm,
        list_property_ix(&developer.pubkey(), 0, 1, POSTCODE, SHARE_PRICE, 101),
        &developer,
        &[&developer],
        "InvalidShareAmount",
    );
}

#[test]
fn list_rejects_zero_price() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    fails_with(
        &mut svm,
        list_property_ix(&developer.pubkey(), 0, 1, POSTCODE, 0, SHARE_AMOUNT),
        &developer,
        &[&developer],
        "InvalidSharePrice",
    );
}

#[test]
fn list_rejects_developer_tax_that_swallows_the_price() {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    // 99.5% tax next to the 1% marketplace fee leaves nothing to pay the
    // developer from, so the dev-pays-tax listing must not open.
    seed_region_taxed(&mut svm, 1, &operator.pubkey(), 9_950);
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
    fails_with(
        &mut svm,
        list.clone(),
        &developer,
        &[&developer],
        "TaxExceedsProceeds",
    );

    // The buyer-pays form of the same listing is fine: its tax rides on top
    // of the price instead of coming out of it.
    list.data = marketplace::instruction::ListProperty {
        region_id: 1,
        postcode: POSTCODE.to_vec(),
        share_price: SHARE_PRICE,
        share_amount: SHARE_AMOUNT,
        tax_paid_by_developer: false,
        max_deposit: u64::MAX,
    }
    .data();
    ok(&mut svm, list, &developer, &[&developer]);
}

#[test]
fn upgrade_object_updates_price() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    ok(
        &mut svm,
        upgrade_ix(&developer.pubkey(), 0, 2 * SHARE_PRICE),
        &developer,
        &[&developer],
    );
    assert_eq!(listing_of(&svm, 0).share_price, 2 * SHARE_PRICE);
}

#[test]
fn upgrade_object_requires_listing_developer() {
    let (mut svm, admin, _authority, developer) = setup_listing();
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    // Another developer holds the role but doesn't own the listing.
    let other = new_developer(&mut svm, &admin);
    fails_with(
        &mut svm,
        upgrade_ix(&other.pubkey(), 0, 2 * SHARE_PRICE),
        &other,
        &[&other],
        "NotListingDeveloper",
    );
}

#[test]
fn upgrade_object_rejects_after_expiry() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    warp(&mut svm, LISTING_DURATION + 1);
    fails_with(
        &mut svm,
        upgrade_ix(&developer.pubkey(), 0, 2 * SHARE_PRICE),
        &developer,
        &[&developer],
        "ListingExpired",
    );
}

#[test]
fn upgrade_object_requires_compliance() {
    let (mut svm, admin, _authority, developer) = setup_listing();
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    set_compliance(&mut svm, &admin, &developer.pubkey(), false);
    fails_with(
        &mut svm,
        upgrade_ix(&developer.pubkey(), 0, 2 * SHARE_PRICE),
        &developer,
        &[&developer],
        "NotCompliant",
    );
}

#[test]
fn upgrade_object_rejects_zero_price() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    fails_with(
        &mut svm,
        upgrade_ix(&developer.pubkey(), 0, 0),
        &developer,
        &[&developer],
        "InvalidSharePrice",
    );
}

#[test]
fn list_rejects_deposit_above_cap() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    fails_with(
        &mut svm,
        list_ix_capped(&developer.pubkey(), 0, LISTING_DEPOSIT - 1),
        &developer,
        &[&developer],
        "DepositTooHigh",
    );
}

// A price below one base unit of the lowest-decimal accepted mint would floor
// to a zero charge, so listing refuses it.
#[test]
fn list_rejects_price_below_min_scale() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    fails_with(
        &mut svm,
        list_property_ix(&developer.pubkey(), 0, 1, POSTCODE, 999, SHARE_AMOUNT),
        &developer,
        &[&developer],
        "InvalidSharePrice",
    );
}

// A listing whose ownership cap admits no purchase must never be created.
#[test]
fn list_rejects_unreachable_ownership_cap() {
    let (mut svm, _admin, _authority, developer) = setup_listing();
    // At the 50% cap, three shares floor to a max holding of one, and the
    // strict bound turns that into zero allowed buys.
    fails_with(
        &mut svm,
        list_property_ix(&developer.pubkey(), 0, 1, POSTCODE, SHARE_PRICE, 3),
        &developer,
        &[&developer],
        "OwnershipCapTooTight",
    );
    // Four shares admit a one-share buy, so listing works.
    ok(
        &mut svm,
        list_property_ix(&developer.pubkey(), 0, 1, POSTCODE, SHARE_PRICE, 4),
        &developer,
        &[&developer],
    );
}
