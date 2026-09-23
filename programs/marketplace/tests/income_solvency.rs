//! Property-based income solvency: random walks over rent distributions,
//! holder claims, and share transfers (which settle both sides through the
//! CPI) must keep each stream's vault balance equal to what every holder is
//! owed plus the undistributed dust, and the share supply conserved across
//! the holdings. Any drift in the accumulator arithmetic on any path shows
//! up here.

mod common;
use common::*;

use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::{AnchorSerialize, Discriminator, InstructionData, ToAccountMetas};
use anchor_spl::token::spl_token::state::Account as SplAccount;
use anchor_spl::token::ID as TOKEN_PROGRAM_ID;
use proptest::prelude::*;
use solana_account::Account;

fn letting_pda() -> Pubkey {
    Pubkey::find_program_address(
        &[b"letting", &0u64.to_le_bytes()],
        &marketplace::PROPERTY_PROGRAM,
    )
    .0
}

fn income_vault_pda() -> Pubkey {
    Pubkey::find_program_address(
        &[b"income-vault", &0u64.to_le_bytes()],
        &marketplace::PROPERTY_PROGRAM,
    )
    .0
}

/// Write the property program's config directly. The income gates read the
/// treasury and rent sponsor from it and nothing else.
fn seed_property_config(svm: &mut LiteSVM) {
    let (address, bump) =
        Pubkey::find_program_address(&[b"config"], &marketplace::PROPERTY_PROGRAM);
    let config = property::state::Config {
        authority: Pubkey::new_unique(),
        pending_authority: None,
        xcav_mint: xcav_mint(),
        treasury: treasury(),
        rent_sponsor: sponsor().pubkey(),
        agent_deposit: 0,
        agent_voting_time: 10_000,
        min_voting_quorum_bps: 2_500,
        agent_notice_period: 10_000,
        proposal_voting_time: 10_000,
        low_proposal: 0,
        high_proposal: u64::MAX,
        high_threshold_bps: 6_000,
        auto_approval_cooldown: 0,
        challenge_deposit: 0,
        agent_slash_amount: 0,
        bump,
    };
    let mut data = property::state::Config::DISCRIMINATOR.to_vec();
    config.serialize(&mut data).unwrap();
    svm.set_account(
        address,
        Account {
            lamports: 100_000_000,
            data,
            owner: marketplace::PROPERTY_PROGRAM,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

fn mint_of(gbp6: bool) -> Pubkey {
    if gbp6 {
        gbp6_mint()
    } else {
        tgbp_mint()
    }
}

fn wallet_acc(owner: &Pubkey, gbp6: bool) -> Pubkey {
    if gbp6 {
        gbp6_acc(owner)
    } else {
        tgbp_acc(owner)
    }
}

fn token_balance(svm: &LiteSVM, addr: &Pubkey) -> u64 {
    svm.get_account(addr)
        .filter(|a| !a.data.is_empty())
        .map(|a| SplAccount::unpack(&a.data).unwrap().amount)
        .unwrap_or(0)
}

/// Seat the agent directly, exactly as an election win would leave it.
fn seed_letting(svm: &mut LiteSVM, agent: &Pubkey) {
    let (address, bump) = Pubkey::find_program_address(
        &[b"letting", &0u64.to_le_bytes()],
        &marketplace::PROPERTY_PROGRAM,
    );
    let letting = property::state::PropertyLetting {
        asset_id: 0,
        agent: *agent,
        election: property::state::AgentElection::default(),
        governance: property::state::GovState::default(),
        rent_payer: Pubkey::new_unique(),
        bump,
    };
    let mut data = property::state::PropertyLetting::DISCRIMINATOR.to_vec();
    letting.serialize(&mut data).unwrap();
    svm.set_account(
        address,
        Account {
            lamports: 100_000_000,
            data,
            owner: marketplace::PROPERTY_PROGRAM,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

fn distribute_income_ix(agent: &Pubkey, gbp6: bool, amount: u64) -> Instruction {
    let mint = mint_of(gbp6);
    Instruction::new_with_bytes(
        marketplace::PROPERTY_PROGRAM,
        &property::instruction::DistributeIncome {
            asset_id: 0,
            amount,
        }
        .data(),
        property::accounts::DistributeIncome {
            agent: *agent,
            payer: *agent,
            agent_role: role_pda(agent, Role::LettingAgent),
            letting: letting_pda(),
            property: property_pda(0),
            market_config: marketplace_config(),
            income: property_income_pda(0),
            payment_mint: mint,
            agent_payment: wallet_acc(agent, gbp6),
            income_vault: income_vault_pda(),
            vault_payment_account: payment_ata(&income_vault_pda(), &mint),
            payment_token_program: TOKEN_PROGRAM_ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

fn claim_income_ix(holder: &Pubkey, gbp6: bool) -> Instruction {
    let mint = mint_of(gbp6);
    Instruction::new_with_bytes(
        marketplace::PROPERTY_PROGRAM,
        &property::instruction::ClaimIncome { asset_id: 0 }.data(),
        property::accounts::ClaimIncome {
            holder: *holder,
            payer: sponsor().pubkey(),
            income: property_income_pda(0),
            checkpoint: property_checkpoint_pda(0, holder),
            holding: holding_pda(0, holder),
            payment_mint: mint,
            income_vault: income_vault_pda(),
            vault_payment_account: payment_ata(&income_vault_pda(), &mint),
            holder_payment: wallet_acc(holder, gbp6),
            payment_token_program: TOKEN_PROGRAM_ID,
            system_program: SYS,
        }
        .to_account_metas(None),
    )
}

/// A finalized property with its letting agent seated and funded, plus a
/// fifth investor holding nothing yet, so transfers also walk the
/// fresh-holding path.
fn income_world() -> (LiteSVM, Keypair, Vec<Keypair>) {
    let (mut svm, admin, mut wallets) = finalized_property();
    svm.add_program(marketplace::PROPERTY_PROGRAM, &program_bytes("property"))
        .unwrap();
    let agent = funded(&mut svm);
    ok(
        &mut svm,
        roles_assign_ix(&admin.pubkey(), &agent.pubkey(), Role::LettingAgent),
        &admin,
        &[&admin],
    );
    seed_property_config(&mut svm);
    seed_letting(&mut svm, &agent.pubkey());
    give_tgbp(&mut svm, &agent.pubkey(), u64::MAX / 2);
    give_gbp6(&mut svm, &agent.pubkey(), u64::MAX / 2);
    wallets.push(new_investor(&mut svm, &admin));
    // Empty accounts in both mints for every wallet, so any claim can land.
    for wallet in &wallets {
        give_tgbp(&mut svm, &wallet.pubkey(), 0);
        give_gbp6(&mut svm, &wallet.pubkey(), 0);
    }
    (svm, agent, wallets)
}

fn check_invariants(svm: &LiteSVM, wallets: &[Keypair]) -> Result<(), TestCaseError> {
    let holdings: Vec<u64> = wallets
        .iter()
        .map(|w| {
            svm.get_account(&holding_pda(0, &w.pubkey()))
                .filter(|a| !a.data.is_empty())
                .map(|_| holding_of(svm, 0, &w.pubkey()).amount as u64)
                .unwrap_or(0)
        })
        .collect();
    prop_assert_eq!(holdings.iter().sum::<u64>(), SHARE_AMOUNT as u64);

    let Some(acc) = svm
        .get_account(&property_income_pda(0))
        .filter(|a| !a.data.is_empty())
    else {
        return Ok(());
    };
    let income =
        property::state::PropertyIncome::try_deserialize(&mut acc.data.as_slice()).unwrap();
    for (index, stream) in income.streams.iter().enumerate() {
        let mut owed = stream.dust as u128;
        for (wallet, shares) in wallets.iter().zip(&holdings) {
            let entry = svm
                .get_account(&property_checkpoint_pda(0, &wallet.pubkey()))
                .filter(|a| !a.data.is_empty())
                .map(|_| checkpoint_of(svm, &wallet.pubkey()))
                .and_then(|cp| cp.entries.get(index).copied())
                .unwrap_or_default();
            owed += (stream.per_share - entry.per_share) * *shares as u128;
            owed += entry.pending as u128;
        }
        let vault = token_balance(svm, &payment_ata(&income_vault_pda(), &stream.mint));
        prop_assert_eq!(vault as u128, owed);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    #[test]
    fn random_income_walks_stay_solvent(
        ops in proptest::collection::vec((0u8..5u8, 0u8..5u8, 1u64..2_000_000_000u64), 1..14)
    ) {
        let (mut svm, agent, wallets) = income_world();
        let spn = sponsor();
        for (who, kind, amount) in ops {
            let wallet = &wallets[who as usize];
            // Failures (nothing to claim, no stream yet, not enough
            // transferable shares) are part of the exercise; the invariants
            // must hold either way.
            match kind {
                0 => {
                    let ix = distribute_income_ix(&agent.pubkey(), false, amount);
                    let _ = process(&mut svm, ix, &agent, &[&agent]);
                }
                1 => {
                    let ix = distribute_income_ix(&agent.pubkey(), true, amount);
                    let _ = process(&mut svm, ix, &agent, &[&agent]);
                }
                2 => {
                    let ix = claim_income_ix(&wallet.pubkey(), false);
                    let _ = process(&mut svm, ix, &spn, &[&spn, wallet]);
                }
                3 => {
                    let ix = claim_income_ix(&wallet.pubkey(), true);
                    let _ = process(&mut svm, ix, &spn, &[&spn, wallet]);
                }
                _ => {
                    // 1 + amount % 4 never lands back on the sender.
                    let other = (who as usize + 1 + (amount % 4) as usize) % wallets.len();
                    let shares = (amount % 40 + 1) as u32;
                    let ix = send_shares_ix(&wallet.pubkey(), &wallets[other].pubkey(), 0, shares);
                    let _ = process(&mut svm, ix, wallet, &[wallet]);
                }
            }
            check_invariants(&svm, &wallets)?;
        }
    }
}
