mod common;
use anchor_lang::solana_program::program_option::COption;
use common::*;

#[test]
fn configuration_requires_upgrade_authority() {
    let mut f = setup_uninitialized();
    fails_with(
        &mut f.svm,
        initialize_ix(&f.buyer.pubkey(), f.verifier.pubkey()),
        &f.buyer,
        "NotUpgradeAuthority",
    );
    assert!(f.svm.get_account(&config()).is_none());
}

#[test]
fn configuration_rejects_mints_that_can_freeze_refunds() {
    let mut f = setup_uninitialized();
    set_mint(&mut f.svm, COption::Some(f.authority.pubkey()));
    fails_with(
        &mut f.svm,
        initialize_ix(&f.authority.pubkey(), f.verifier.pubkey()),
        &f.authority,
        "UnsupportedMintAuthority",
    );
    assert!(f.svm.get_account(&config()).is_none());
}

#[test]
fn configuration_rejects_default_verifier() {
    let mut f = setup_uninitialized();
    fails_with(
        &mut f.svm,
        initialize_ix(&f.authority.pubkey(), Pubkey::default()),
        &f.authority,
        "InvalidConfig",
    );
}

#[test]
fn drafts_are_owned_versioned_and_do_not_take_xcav() {
    let mut f = setup();
    let before = balance(&f.svm, region_common::token_acc(&f.operator.pubkey()));
    ok(
        &mut f.svm,
        create_ix(&f.operator.pubkey(), 0, params()),
        &f.operator,
    );
    let draft = hub_of(&f.svm, 0);
    assert_eq!(draft.operator, f.operator.pubkey());
    assert_eq!(draft.status, HubStatus::Draft);
    assert_eq!(draft.bond_amount, BOND);
    assert_eq!(draft.revision, 1);
    assert_eq!(
        balance(&f.svm, region_common::token_acc(&f.operator.pubkey())),
        before
    );
    let mut revised = params();
    revised.name = "Revised cold storage".into();
    revised.metadata_hash = [2; 32];
    ok(
        &mut f.svm,
        edit_ix(&f.operator.pubkey(), 0, revised),
        &f.operator,
    );
    let draft = hub_of(&f.svm, 0);
    assert_eq!(draft.revision, 2);
    assert_eq!(draft.name, "Revised cold storage");
    assert_eq!(draft.metadata_hash, [2; 32]);
}

#[test]
fn a_general_operator_role_does_not_grant_region_ownership() {
    let mut f = setup();
    let other = region_common::actor(&mut f.svm);
    region_common::ok(
        &mut f.svm,
        region_common::roles_assign_ix(&f.admin.pubkey(), &other.pubkey(), Role::RegionalOperator),
        &f.admin,
        &[&f.admin],
    );
    fails_with(
        &mut f.svm,
        create_ix(&other.pubkey(), 0, params()),
        &other,
        "NotRegionOwner",
    );
}

#[test]
fn draft_creation_requires_an_operator_role() {
    let mut f = setup();
    fails_with(
        &mut f.svm,
        create_ix(&f.buyer.pubkey(), 0, params()),
        &f.buyer,
        "AccountNotInitialized",
    );
}

#[test]
fn revoked_operator_cannot_submit() {
    let mut f = setup();
    ok(
        &mut f.svm,
        create_ix(&f.operator.pubkey(), 0, params()),
        &f.operator,
    );
    region_common::set_permission(
        &mut f.svm,
        &f.authority,
        &f.operator.pubkey(),
        Role::RegionalOperator,
        false,
    );
    fails_with(
        &mut f.svm,
        submit_ix(&f.operator.pubkey(), 0),
        &f.operator,
        "NotCompliant",
    );
    assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Draft);
}

#[test]
fn proposal_terms_reject_overflow_and_empty_metadata() {
    let mut f = setup();
    let mut invalid = params();
    invalid.token_price = u64::MAX;
    fails_with(
        &mut f.svm,
        create_ix(&f.operator.pubkey(), 0, invalid),
        &f.operator,
        "Overflow",
    );
    let mut invalid = params();
    invalid.metadata_hash = [0; 32];
    fails_with(
        &mut f.svm,
        create_ix(&f.operator.pubkey(), 0, invalid),
        &f.operator,
        "InvalidProposal",
    );
    assert!(f.svm.get_account(&hub(0)).is_none());
}

#[test]
fn another_wallet_cannot_edit_or_submit_the_operators_draft() {
    let mut f = setup();
    region_common::ok(
        &mut f.svm,
        region_common::roles_assign_ix(
            &f.admin.pubkey(),
            &f.buyer.pubkey(),
            Role::RegionalOperator,
        ),
        &f.admin,
        &[&f.admin],
    );
    ok(
        &mut f.svm,
        create_ix(&f.operator.pubkey(), 0, params()),
        &f.operator,
    );
    fails_with(
        &mut f.svm,
        edit_ix(&f.buyer.pubkey(), 0, params()),
        &f.buyer,
        "NotOperator",
    );
    fails_with(
        &mut f.svm,
        submit_ix(&f.buyer.pubkey(), 0),
        &f.buyer,
        "NotOperator",
    );
}

#[test]
fn submitted_terms_cannot_change() {
    let mut f = setup();
    reach_submitted(&mut f, 0);
    fails_with(
        &mut f.svm,
        edit_ix(&f.operator.pubkey(), 0, params()),
        &f.operator,
        "InvalidStatus",
    );
    fails_with(
        &mut f.svm,
        submit_ix(&f.operator.pubkey(), 0),
        &f.operator,
        "InvalidStatus",
    );
}

#[test]
fn only_configured_verifier_can_review() {
    let mut f = setup();
    reach_submitted(&mut f, 0);
    fails_with(
        &mut f.svm,
        review_ix(&f.buyer.pubkey(), 0, 1, [1; 32], true),
        &f.buyer,
        "NotVerifier",
    );
}

#[test]
fn operator_cannot_review_their_own_proposal() {
    let mut f = setup_uninitialized();
    ok(
        &mut f.svm,
        initialize_ix(&f.authority.pubkey(), f.operator.pubkey()),
        &f.authority,
    );
    reach_submitted(&mut f, 0);
    fails_with(
        &mut f.svm,
        review_ix(&f.operator.pubkey(), 0, 1, [1; 32], true),
        &f.operator,
        "SelfReview",
    );
}

#[test]
fn review_binds_both_revision_and_hash() {
    let mut f = setup();
    reach_submitted(&mut f, 0);
    fails_with(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), 0, 2, [1; 32], true),
        &f.verifier,
        "ReviewMismatch",
    );
    fails_with(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), 0, 1, [2; 32], true),
        &f.verifier,
        "ReviewMismatch",
    );
    ok(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), 0, 1, [1; 32], true),
        &f.verifier,
    );
    assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Approved);
    fails_with(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), 0, 1, [1; 32], false),
        &f.verifier,
        "InvalidStatus",
    );
}

#[test]
fn rejected_proposal_requires_a_new_revision_and_review() {
    let mut f = setup();
    reach_submitted(&mut f, 0);
    ok(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), 0, 1, [1; 32], false),
        &f.verifier,
    );
    fails_with(
        &mut f.svm,
        activate_ix(&f.operator.pubkey(), 0, BOND),
        &f.operator,
        "InvalidStatus",
    );
    assert!(f.svm.get_account(&hub_mint(0)).is_none());
    let mut revised = params();
    revised.metadata_hash = [2; 32];
    ok(
        &mut f.svm,
        edit_ix(&f.operator.pubkey(), 0, revised),
        &f.operator,
    );
    assert_eq!(hub_of(&f.svm, 0).reviewed_by, Pubkey::default());
    ok(&mut f.svm, submit_ix(&f.operator.pubkey(), 0), &f.operator);
    fails_with(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), 0, 1, [1; 32], true),
        &f.verifier,
        "ReviewMismatch",
    );
    ok(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), 0, 2, [2; 32], true),
        &f.verifier,
    );
}

#[test]
fn activation_is_approval_gated_and_bond_capped() {
    let mut f = setup();
    reach_submitted(&mut f, 0);
    fails_with(
        &mut f.svm,
        activate_ix(&f.operator.pubkey(), 0, BOND),
        &f.operator,
        "InvalidStatus",
    );
    ok(
        &mut f.svm,
        review_ix(&f.verifier.pubkey(), 0, 1, [1; 32], true),
        &f.verifier,
    );
    let before = balance(&f.svm, region_common::token_acc(&f.operator.pubkey()));
    fails_with(
        &mut f.svm,
        activate_ix(&f.operator.pubkey(), 0, BOND - 1),
        &f.operator,
        "BondTooHigh",
    );
    assert_eq!(
        balance(&f.svm, region_common::token_acc(&f.operator.pubkey())),
        before
    );
    assert!(f.svm.get_account(&hub_mint(0)).is_none());
    ok(
        &mut f.svm,
        activate_ix(&f.operator.pubkey(), 0, BOND),
        &f.operator,
    );
    assert_eq!(balance(&f.svm, bond_vault(0)), BOND);
    assert_eq!(balance(&f.svm, token_vault(0)), SUPPLY);
    assert_eq!(hub_of(&f.svm, 0).status, HubStatus::Listed);
}

#[test]
fn activation_rechecks_operator_eligibility() {
    let mut f = setup();
    reach_approved(&mut f, 0);
    region_common::set_permission(
        &mut f.svm,
        &f.authority,
        &f.operator.pubkey(),
        Role::RegionalOperator,
        false,
    );
    fails_with(
        &mut f.svm,
        activate_ix(&f.operator.pubkey(), 0, BOND),
        &f.operator,
        "NotCompliant",
    );
}

#[test]
fn activation_survives_a_prefunded_mint_and_cannot_mint_twice() {
    use anchor_lang::solana_program::program_pack::Pack;
    use anchor_spl::token::spl_token::state::Mint;
    let mut f = setup();
    reach_approved(&mut f, 0);
    f.svm.airdrop(&hub_mint(0), 1).unwrap();
    ok(
        &mut f.svm,
        activate_ix(&f.operator.pubkey(), 0, BOND),
        &f.operator,
    );
    let mint = Mint::unpack(&f.svm.get_account(&hub_mint(0)).unwrap().data).unwrap();
    assert_eq!(mint.supply, SUPPLY);
    assert_eq!(mint.decimals, 0);
    assert!(mint.mint_authority.is_none());
    assert!(mint.freeze_authority.is_none());
    assert!(process(
        &mut f.svm,
        activate_ix(&f.operator.pubkey(), 0, BOND),
        &f.operator
    )
    .is_err());
    assert_eq!(balance(&f.svm, bond_vault(0)), BOND);
    assert_eq!(balance(&f.svm, token_vault(0)), SUPPLY);
}
