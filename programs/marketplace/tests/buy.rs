//! Direct purchases, the post-claim-window market: funds held in the listing
//! vault with their fee/tax split, direct share delivery into locked
//! accounts, the ownership cap, and the sellout flip.

mod common;
use common::*;

use anchor_spl::token_2022::spl_token_2022::{
    extension::StateWithExtensions,
    state::{Account as TokenAccountState, AccountState},
};
use marketplace::state::ListingStatus;

/// Price components for `amount` shares at the default listing settings:
/// price, 1% investor fee, 3% region tax.
fn cost_of(amount: u32) -> (u64, u64, u64) {
    let price = SHARE_PRICE * amount as u64;
    (price, price / 100, price * 300 / 10_000)
}

fn total_of(amount: u32) -> u64 {
    let (price, fee, tax) = cost_of(amount);
    price + fee + tax
}

/// Full pipeline: region + location + compliant developer + listed property
/// with its share mint live, plus a sponsor wallet that fronts investor rent.
fn setup_listed() -> (LiteSVM, Keypair, Keypair) {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    // The sponsor is the configured rent collector; setup funded it.
    (svm, admin, sponsor())
}

/// `setup_listed` moved past the claim window, where direct purchases run:
/// filler investors reserve every share so the SPV can attest, the window
/// runs out unclaimed, and the crank releases them all.
fn setup_direct() -> (LiteSVM, Keypair, Keypair) {
    let (mut svm, admin, sponsor) = setup_listed();
    acquire_many(&mut svm, &admin, &[]);
    (svm, admin, sponsor)
}

fn buy(svm: &mut LiteSVM, investor: &Keypair, sponsor: &Keypair, amount: u32) {
    ok(
        svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, amount, u64::MAX),
        sponsor,
        &[sponsor, investor],
    );
}

#[test]
fn buy_delivers_shares_and_holds_funds() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);

    let tgbp_before = tgbp_balance(&svm, &investor.pubkey());
    buy(&mut svm, &investor, &sponsor, 10);

    // Funds: price + fee + tax landed in the listing vault, split recorded.
    let (price, fee, tax) = cost_of(10);
    assert_eq!(
        tgbp_before - tgbp_balance(&svm, &investor.pubkey()),
        total_of(10)
    );
    let vault_acc = svm.get_account(&listing_payment_ata(0)).unwrap();
    let vault_state = StateWithExtensions::<TokenAccountState>::unpack(&vault_acc.data).unwrap();
    assert_eq!(vault_state.base.amount, total_of(10));

    let position = position_of(&svm, 0, &investor.pubkey());
    assert_eq!(position.share_amount, 10);
    assert_eq!(position.paid_funds, price);
    assert_eq!(position.paid_fee, fee);
    assert_eq!(position.paid_tax, tax);
    assert_eq!(position.payment_mint, tgbp_mint());
    assert!(!position.cancelled);

    // Shares: delivered to the investor's account, which ends up locked.
    let share_acc = svm
        .get_account(&investor_share_ata(0, &investor.pubkey()))
        .unwrap();
    let share_state = StateWithExtensions::<TokenAccountState>::unpack(&share_acc.data).unwrap();
    assert_eq!(share_state.base.amount, 10);
    assert_eq!(share_state.base.state, AccountState::Frozen);

    assert_eq!(holding_of(&svm, 0, &investor.pubkey()).amount, 10);
    assert_eq!(property_of(&svm, 0).holder_count, 1);
    assert_eq!(listing_of(&svm, 0).sold_share_amount, 10);
    assert_eq!(listing_of(&svm, 0).status, ListingStatus::Listed);
}

#[test]
fn second_buy_accumulates() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);

    buy(&mut svm, &investor, &sponsor, 10);
    buy(&mut svm, &investor, &sponsor, 15);

    assert_eq!(position_of(&svm, 0, &investor.pubkey()).share_amount, 25);
    assert_eq!(holding_of(&svm, 0, &investor.pubkey()).amount, 25);
    // Same wallet, so still one holder.
    assert_eq!(property_of(&svm, 0).holder_count, 1);
    assert_eq!(listing_of(&svm, 0).sold_share_amount, 25);
}

#[test]
fn buy_requires_a_compliant_investor() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);
    set_compliance(&mut svm, &admin, &investor.pubkey(), false);

    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 10, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "NotCompliant",
    );
}

#[test]
fn buy_rejects_unaccepted_mint() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);
    give_xcav(&mut svm, &investor.pubkey(), FUND_XCAV);

    // Paying in XCAV: a real mint, but not on the accepted list.
    let vault_ata = Pubkey::find_program_address(
        &[
            listing_vault_pda(0).as_ref(),
            anchor_spl::token::ID.as_ref(),
            xcav_mint().as_ref(),
        ],
        &anchor_spl::associated_token::ID,
    )
    .0;
    fails_with(
        &mut svm,
        buy_ix_with_mint(
            &investor.pubkey(),
            &sponsor.pubkey(),
            0,
            10,
            u64::MAX,
            xcav_mint(),
            token_acc(&investor.pubkey()),
            vault_ata,
        ),
        &sponsor,
        &[&sponsor, &investor],
        "MintNotAccepted",
    );
}

#[test]
fn buy_rejects_more_than_remaining() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);
    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 101, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "InvalidShareAmount",
    );
}

// Holdings stay strictly below the 50% cap from the listing snapshot, so 49
// of 100 shares is the most one investor can reach.
#[test]
fn buy_enforces_ownership_cap() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);

    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 50, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "MaxOwnershipExceeded",
    );

    // The cap is cumulative across buys.
    buy(&mut svm, &investor, &sponsor, 30);
    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 20, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "MaxOwnershipExceeded",
    );
    buy(&mut svm, &investor, &sponsor, 19);
    assert_eq!(holding_of(&svm, 0, &investor.pubkey()).amount, 49);
}

#[test]
fn buy_rejects_cost_above_cap() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);
    fails_with(
        &mut svm,
        buy_ix(
            &investor.pubkey(),
            &sponsor.pubkey(),
            0,
            10,
            total_of(10) - 1,
        ),
        &sponsor,
        &[&sponsor, &investor],
        "CostTooHigh",
    );
}

#[test]
fn last_share_flips_to_sold_out() {
    let (mut svm, admin, sponsor) = setup_direct();
    let alice = new_investor(&mut svm, &admin);
    let bob = new_investor(&mut svm, &admin);
    let carol = new_investor(&mut svm, &admin);

    buy(&mut svm, &alice, &sponsor, 34);
    buy(&mut svm, &bob, &sponsor, 33);
    buy(&mut svm, &carol, &sponsor, 33);

    assert_eq!(listing_of(&svm, 0).status, ListingStatus::SoldOut);
    assert_eq!(property_of(&svm, 0).holder_count, 3);

    // The sale is locked in; nobody can buy any more.
    let late = new_investor(&mut svm, &admin);
    fails_with(
        &mut svm,
        buy_ix(&late.pubkey(), &sponsor.pubkey(), 0, 1, u64::MAX),
        &sponsor,
        &[&sponsor, &late],
        "ListingNotActive",
    );
}

#[test]
fn buy_requires_listed_status() {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );

    // Still PendingAssets: the share mint and vault account don't even exist
    // yet, so validation refuses before the status check is ever reached.
    let sponsor = funded(&mut svm);
    let investor = new_investor(&mut svm, &admin);
    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 10, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "AccountNotInitialized",
    );
}

#[test]
fn buy_after_expiry_fails() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);
    warp(&mut svm, LISTING_DURATION + 1);
    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &sponsor.pubkey(), 0, 10, u64::MAX),
        &sponsor,
        &[&sponsor, &investor],
        "ListingExpired",
    );
}

// Rent sponsorship is pinned to the configured rent collector, so the wallet
// fronting rent and the wallet refunded at close are always the same one.
#[test]
fn buy_rejects_foreign_sponsor() {
    let (mut svm, admin, _sponsor) = setup_listed();
    let investor = new_investor(&mut svm, &admin);
    let stranger = funded(&mut svm);
    fails_with(
        &mut svm,
        buy_ix(&investor.pubkey(), &stranger.pubkey(), 0, 10, u64::MAX),
        &stranger,
        &[&stranger, &investor],
        "NotRentCollector",
    );
}

// Paying in the 6-decimal GBP mint runs the rescale at a real factor: every
// component divides by 1_000.
#[test]
fn buy_scales_to_six_decimal_mint() {
    let (mut svm, admin, sponsor) = setup_direct();
    let investor = new_investor(&mut svm, &admin);
    give_gbp6(&mut svm, &investor.pubkey(), 1_000_000_000);

    let vault_ata = Pubkey::find_program_address(
        &[
            listing_vault_pda(0).as_ref(),
            anchor_spl::token::ID.as_ref(),
            gbp6_mint().as_ref(),
        ],
        &anchor_spl::associated_token::ID,
    )
    .0;
    ok(
        &mut svm,
        buy_ix_with_mint(
            &investor.pubkey(),
            &sponsor.pubkey(),
            0,
            10,
            u64::MAX,
            gbp6_mint(),
            gbp6_acc(&investor.pubkey()),
            vault_ata,
        ),
        &sponsor,
        &[&sponsor, &investor],
    );

    let (price, fee, tax) = cost_of(10);
    let position = position_of(&svm, 0, &investor.pubkey());
    assert_eq!(position.payment_mint, gbp6_mint());
    assert_eq!(position.paid_funds, price / 1_000);
    assert_eq!(position.paid_fee, fee / 1_000);
    assert_eq!(position.paid_tax, tax / 1_000);
    let vault_acc = svm.get_account(&vault_ata).unwrap();
    let vault_state = StateWithExtensions::<TokenAccountState>::unpack(&vault_acc.data).unwrap();
    assert_eq!(vault_state.base.amount, total_of(10) / 1_000);
}

// The recorded fee quote is the cap on the SPV lawyer's costs, and on a
// cancellation the retained fee is all they can be paid from. So it must
// state what the vault actually holds: the round trip through the mint's
// decimals, not the unfloored quote.
#[test]
fn direct_buy_records_only_the_fee_actually_held() {
    let (mut svm, admin, _authority) = setup();
    let operator = funded(&mut svm);
    seed_region(&mut svm, 1, &operator.pubkey());
    seed_location(&mut svm, 1, POSTCODE);
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), 0),
        &developer,
        &[&developer],
    );
    // A price that doesn't divide into six-decimal units, so the 1% fee
    // floors when it scales to the mint.
    ok(
        &mut svm,
        upgrade_ix(&developer.pubkey(), 0, 5_000_000_333),
        &developer,
        &[&developer],
    );
    acquire_many(&mut svm, &admin, &[]);

    let sponsor = sponsor();
    let investor = new_investor(&mut svm, &admin);
    give_gbp6(&mut svm, &investor.pubkey(), 1_000_000_000);
    let vault_ata = payment_ata(&listing_vault_pda(0), &gbp6_mint());
    ok(
        &mut svm,
        buy_ix_with_mint(
            &investor.pubkey(),
            &sponsor.pubkey(),
            0,
            1,
            u64::MAX,
            gbp6_mint(),
            gbp6_acc(&investor.pubkey()),
            vault_ata,
        ),
        &sponsor,
        &[&sponsor, &investor],
    );

    // Fee at 9 decimals is 50_000_003; in the vault it is 50_000 six-decimal
    // units, which restates as 50_000_000. The trailing 3 never arrived.
    let listing = listing_of(&svm, 0);
    assert_eq!(listing.collected[0].fee, 50_000);
    assert_eq!(listing.collected_fee_quote, 50_000_000);
}
