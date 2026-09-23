//! Config lifecycle: initialization validation, parameter updates, and
//! authority rotation.

mod common;
use common::*;

#[test]
fn init_rejects_bad_threshold() {
    let mut svm = LiteSVM::new();
    svm.add_program(rid(), &program_bytes("regions")).unwrap();
    set_mint(&mut svm);
    let authority = funded(&mut svm);
    bind_upgrade_authority(&mut svm, &rid(), &authority.pubkey());

    let mut params = default_params();
    params.threshold_bps = 10_001; // above 100%
    fails_with(
        &mut svm,
        regions_init_ix_with(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "InvalidConfig",
    );
}

#[test]
fn update_config_by_authority_works() {
    let (mut svm, _operator, authority) = setup();

    let mut params = default_params();
    params.minimum_voting_amount = 250_000_000;
    ok(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
    );

    let cfg =
        RegionsConfig::try_deserialize(&mut &svm.get_account(&regions_config()).unwrap().data[..])
            .unwrap();
    assert_eq!(cfg.minimum_voting_amount, 250_000_000);
}

#[test]
fn update_config_by_non_authority_fails() {
    let (mut svm, _operator, _authority) = setup();
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
    let (mut svm, _operator, authority) = setup();
    let mut params = default_params();
    params.quorum = 0;
    fails_with(
        &mut svm,
        update_config_ix(&authority.pubkey(), params),
        &authority,
        &[&authority],
        "InvalidConfig",
    );
    // A zero bond share would let anyone take a region for free.
    let mut params = default_params();
    params.operator_bond_bps = 0;
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
    let (mut svm, _operator, authority) = setup();
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
    let (mut svm, _operator, authority) = setup();

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
    let (mut svm, _operator, authority) = setup();
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

#[test]
fn init_requires_upgrade_authority() {
    let mut svm = LiteSVM::new();
    svm.add_program(rid(), &program_bytes("regions")).unwrap();
    set_mint(&mut svm);
    let deployer = funded(&mut svm);
    bind_upgrade_authority(&mut svm, &rid(), &deployer.pubkey());

    // Someone other than the deployer cannot claim the config.
    let imposter = funded(&mut svm);
    fails_with(
        &mut svm,
        regions_init_ix(&imposter.pubkey()),
        &imposter,
        &[&imposter],
        "NotUpgradeAuthority",
    );
}
