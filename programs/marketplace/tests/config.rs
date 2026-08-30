//! Config lifecycle: initialization validation, parameter updates, and
//! authority rotation.

mod common;
use common::*;

/// A fresh SVM with the marketplace program loaded and an upgrade-authority
/// keypair bound, but no config yet.
fn pre_init() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    svm.add_program(mid(), &program_bytes("marketplace"))
        .unwrap();
    set_mint(&mut svm);
    let authority = funded(&mut svm);
    bind_upgrade_authority(&mut svm, &mid(), &authority.pubkey());
    (svm, authority)
}

#[test]
fn init_sets_fields() {
    let (mut svm, authority) = pre_init();
    ok(
        &mut svm,
        init_ix(&authority.pubkey()),
        &authority,
        &[&authority],
    );

    let cfg = config_of(&svm);
    assert_eq!(cfg.authority, authority.pubkey());
    assert_eq!(cfg.pending_authority, None);
    assert_eq!(cfg.xcav_mint, xcav_mint());
    assert_eq!(cfg.accepted_payment_mints, vec![tgbp_mint(), gbp6_mint()]);
    assert_eq!(cfg.listing_deposit, LISTING_DEPOSIT);
    assert_eq!(cfg.lawyer_deposit, LAWYER_DEPOSIT);
    assert_eq!(cfg.max_property_shares, 100);
    assert_eq!(cfg.next_listing_id, 0);
    assert_eq!(vault_balance(&svm), 0);
}

#[test]
fn init_requires_upgrade_authority() {
    let (mut svm, _deployer) = pre_init();

    // Someone other than the deployer cannot claim the config.
    let imposter = funded(&mut svm);
    fails_with(
        &mut svm,
        init_ix(&imposter.pubkey()),
        &imposter,
        &[&imposter],
        "NotUpgradeAuthority",
    );
}

#[test]
fn init_rejects_fee_above_100_percent() {
    let (mut svm, authority) = pre_init();
    let mut params = default_params();
    params.operator_fee_share_bps = 10_001;
    fails_with(
        &mut svm,
        init_ix_with(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "InvalidConfig",
    );
}

#[test]
fn init_rejects_bad_payment_mints() {
    let (mut svm, authority) = pre_init();

    // No payment mints at all.
    let mut params = default_params();
    params.accepted_payment_mints = vec![];
    fails_with(
        &mut svm,
        init_ix_with(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "InvalidConfig",
    );

    // The same mint twice.
    let mut params = default_params();
    params.accepted_payment_mints = vec![tgbp_mint(), tgbp_mint()];
    fails_with(
        &mut svm,
        init_ix_with(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "InvalidConfig",
    );
}

#[test]
fn init_rejects_bad_share_bounds() {
    let (mut svm, authority) = pre_init();

    // min above max.
    let mut params = default_params();
    params.min_property_shares = 200;
    params.max_property_shares = 100;
    fails_with(
        &mut svm,
        init_ix_with(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "InvalidConfig",
    );

    // max above the hard supply ceiling.
    let mut params = default_params();
    params.max_property_shares = 101;
    fails_with(
        &mut svm,
        init_ix_with(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "InvalidConfig",
    );
}

#[test]
fn update_config_by_authority_works() {
    let (mut svm, _admin, authority) = setup();

    let mut params = default_params();
    params.listing_deposit = 2_000_000_000;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    assert_eq!(config_of(&svm).listing_deposit, 2_000_000_000);
}

#[test]
fn update_config_by_non_authority_fails() {
    let (mut svm, _admin, _authority) = setup();
    let stranger = funded(&mut svm);
    fails_with(
        &mut svm,
        update_config_ix(&stranger.pubkey(), default_params()),
        &stranger,
        &[&stranger],
        "NotAuthority",
    );
}

#[test]
fn update_config_rejects_bad_params() {
    let (mut svm, _admin, authority) = setup();
    let mut params = default_params();
    params.lawyer_voting_time = 0;
    fails_with(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "InvalidConfig",
    );
}

#[test]
fn update_authority_is_two_step() {
    let (mut svm, _admin, authority) = setup();
    let new_auth = funded(&mut svm);
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), &new_auth.pubkey()),
        &authority,
        &[&authority],
    );

    // Proposing alone changes nothing: the current authority stays in charge
    // and the proposed key has no power yet.
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), default_params()),
        &authority,
        &[&authority],
    );
    fails_with(
        &mut svm,
        update_config_ix(&new_auth.pubkey(), default_params()),
        &new_auth,
        &[&new_auth],
        "NotAuthority",
    );

    // Accepting completes the handover.
    ok(
        &mut svm,
        accept_authority_ix(&new_auth.pubkey()),
        &new_auth,
        &[&new_auth],
    );
    fails_with(
        &mut svm,
        update_config_ix(&authority.pubkey(), default_params()),
        &authority,
        &[&authority],
        "NotAuthority",
    );
    ok(
        &mut svm,
        update_config_ix(&new_auth.pubkey(), default_params()),
        &new_auth,
        &[&new_auth],
    );
}

#[test]
fn accept_authority_fails_without_matching_proposal() {
    let (mut svm, _admin, authority) = setup();

    // No proposal at all.
    let hopeful = funded(&mut svm);
    fails_with(
        &mut svm,
        accept_authority_ix(&hopeful.pubkey()),
        &hopeful,
        &[&hopeful],
        "NotPendingAuthority",
    );

    // A proposal for someone else doesn't help either.
    let intended = funded(&mut svm);
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), &intended.pubkey()),
        &authority,
        &[&authority],
    );
    fails_with(
        &mut svm,
        accept_authority_ix(&hopeful.pubkey()),
        &hopeful,
        &[&hopeful],
        "NotPendingAuthority",
    );
}

#[test]
fn update_authority_reproposal_overwrites_pending() {
    let (mut svm, _admin, authority) = setup();
    let first = funded(&mut svm);
    let second = funded(&mut svm);
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), &first.pubkey()),
        &authority,
        &[&authority],
    );
    ok(
        &mut svm,
        update_authority_ix(&authority.pubkey(), &second.pubkey()),
        &authority,
        &[&authority],
    );

    // The superseded proposal is dead; only the latest can accept.
    fails_with(
        &mut svm,
        accept_authority_ix(&first.pubkey()),
        &first,
        &[&first],
        "NotPendingAuthority",
    );
    ok(
        &mut svm,
        accept_authority_ix(&second.pubkey()),
        &second,
        &[&second],
    );
}

// The mint guard runs on every accepted payment mint at config time, so a
// fee-bearing entry can't silently break vault accounting later.
#[test]
fn init_rejects_unsupported_payment_mint() {
    let (mut svm, authority) = pre_init();
    let fee_mint = Pubkey::new_from_array([21u8; 32]);
    set_fee_bearing_mint(&mut svm, fee_mint);

    let mut params = default_params();
    params.accepted_payment_mints = vec![fee_mint];
    fails_with(
        &mut svm,
        init_ix_with(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "UnsupportedMintExtension",
    );
}

#[test]
fn init_requires_payment_mint_accounts() {
    let (mut svm, authority) = pre_init();
    // Drop the trailing payment-mint account from the instruction.
    let mut ix = init_ix(&authority.pubkey());
    ix.accounts.pop();
    fails_with(&mut svm, ix, &authority, &[&authority], "InvalidConfig");
}

#[test]
fn init_rejects_xcav_with_lock_authority() {
    let (mut svm, authority) = pre_init();
    set_mint_with_lock_authority(&mut svm, xcav_mint());
    fails_with(
        &mut svm,
        init_ix(&authority.pubkey()),
        &authority,
        &[&authority],
        "UnsupportedMintAuthority",
    );
}
