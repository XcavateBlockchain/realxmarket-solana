//! The protocol end to end through the real instructions of all four
//! programs: region vote, listing, sale, SPV and legal phase, settlement,
//! agent election, rent, governance, secondary trades, then a second rent
//! round to prove the income ledger followed the shares. Nothing is seeded
//! by hand, so a mismatch between what one program writes and what the
//! next expects fails here.
mod common;
mod e2e_support;
use common::*;
use e2e_support::*;

use anchor_lang::Discriminator;
use property::instructions::governance::ProposalExecuted;

const ASSET: u64 = 0;
const SHARES: [u32; 4] = [34, 33, 24, 9];
/// What `new_investor` funds each wallet with.
const INVESTOR_FUNDS: u64 = 1_000_000_000_000;
/// One rent round: 100 tGBP over 100 shares, one tGBP per share.
const RENT: u64 = 100_000_000_000;
const ASK: u64 = 6_000_000_000;
const BID: u64 = 5_000_000_000;
/// Between the property config's low and high tiers, so it goes to a vote.
const WORKS: u64 = 500_000_000_000;

fn bps(amount: u64, rate: u16) -> u64 {
    (amount as u128 * rate as u128 / 10_000) as u64
}

/// Every tGBP account the run touches, against what was minted into it.
#[derive(Default)]
struct Ledger {
    accounts: Vec<Pubkey>,
    minted: u64,
}

impl Ledger {
    fn fund(&mut self, svm: &mut LiteSVM, owner: &Pubkey, amount: u64) {
        give_tgbp(svm, owner, amount);
        self.accounts.push(tgbp_acc(owner));
        self.minted += amount;
    }
    fn track(&mut self, address: Pubkey) {
        self.accounts.push(address);
    }
    fn investor(&mut self, svm: &mut LiteSVM, admin: &Keypair) -> Keypair {
        let kp = new_investor(svm, admin);
        self.accounts.push(tgbp_acc(&kp.pubkey()));
        self.minted += INVESTOR_FUNDS;
        kp
    }
    fn total(&self, svm: &LiteSVM) -> u64 {
        self.accounts.iter().map(|a| balance_at(svm, a)).sum()
    }
}

#[test]
fn full_lifecycle_across_all_four_programs() {
    let (mut svm, admin, _authority) = setup_all();
    let mut ledger = Ledger::default();
    let spn = sponsor();
    let cranker = funded(&mut svm);

    // --- regions: propose, vote, finalize, create, register a postcode ---
    let (voter, proposal_id) = live_region(&mut svm, &admin);
    let operator = region_operator();
    let region = region_of(&svm, REGION);
    assert_eq!(region.owner, operator.pubkey());
    assert_eq!(region.location_count, 1);
    assert_eq!(
        (region.tax_bps, region.seller_fee_bps, region.buyer_fee_bps),
        (TAX_BPS, SELLER_FEE_BPS, BUYER_FEE_BPS)
    );
    // The bond plus the location deposit, with the deposit also tracked
    // on its own.
    assert_eq!(region.collateral, REGION_BOND + LOCATION_DEPOSIT);
    assert_eq!(region.location_collateral, LOCATION_DEPOSIT);
    assert_eq!(
        balance_at(&svm, &xcav_ata(&operator.pubkey())),
        FUND_XCAV - REGION_BOND - LOCATION_DEPOSIT
    );
    // The voter's stake comes back once the window closed.
    ok(
        &mut svm,
        unlock_region_vote_ix(&voter.pubkey(), proposal_id),
        &voter,
        &[&voter],
    );
    assert_eq!(balance_at(&svm, &xcav_ata(&voter.pubkey())), FUND_XCAV);
    assert_eq!(
        balance_at(&svm, &regions_vault()),
        REGION_BOND + LOCATION_DEPOSIT
    );

    // --- marketplace: list against the live region and location ---
    let developer = new_developer(&mut svm, &admin);
    ok(
        &mut svm,
        list_ix(&developer.pubkey(), ASSET),
        &developer,
        &[&developer],
    );
    ok(
        &mut svm,
        init_assets_ix(&developer.pubkey(), ASSET),
        &developer,
        &[&developer],
    );
    let listing = listing_of(&svm, ASSET);
    // The listing froze the region's live tax and fees, not fixture values.
    assert_eq!(
        (
            listing.tax_bps,
            listing.seller_fee_bps,
            listing.buyer_fee_bps
        ),
        (TAX_BPS, SELLER_FEE_BPS, BUYER_FEE_BPS)
    );
    assert_eq!(
        balance_at(&svm, &token_acc(&developer.pubkey())),
        FUND_XCAV - LISTING_DEPOSIT
    );
    let property = property_of(&svm, ASSET);
    assert_eq!(property.region_id, REGION);
    assert_eq!(property.location, POSTCODE.to_vec());

    // Four investors take the whole supply; the SPV confirmer opens claiming.
    let investors: Vec<Keypair> = SHARES
        .iter()
        .map(|_| ledger.investor(&mut svm, &admin))
        .collect();
    for (investor, shares) in investors.iter().zip(SHARES) {
        ok(
            &mut svm,
            reserve_ix(&investor.pubkey(), &spn.pubkey(), ASSET, shares, u64::MAX),
            &spn,
            &[&spn, investor],
        );
    }
    let confirmer = new_confirmer(&mut svm, &admin);
    ok(
        &mut svm,
        create_spv_ix(&confirmer.pubkey(), ASSET),
        &confirmer,
        &[&confirmer],
    );
    for investor in &investors {
        ok(
            &mut svm,
            claim_ix(&investor.pubkey(), &spn.pubkey(), ASSET),
            &spn,
            &[&spn, investor],
        );
    }
    let listing = listing_of(&svm, ASSET);
    assert_eq!(listing.status, ListingStatus::SoldOut);
    assert_eq!(listing.collected.len(), 1);
    let collected = listing.collected[0];
    assert_eq!(collected.mint, tgbp_mint());
    assert_eq!(collected.funds, SHARE_AMOUNT as u64 * SHARE_PRICE);
    assert_eq!(collected.fee, bps(collected.funds, BUYER_FEE_BPS));
    assert_eq!(collected.tax, bps(collected.funds, TAX_BPS));
    // What the investors paid is exactly what the listing recorded, and
    // all of it sits in the listing's vault.
    let paid: u64 = investors
        .iter()
        .map(|i| INVESTOR_FUNDS - tgbp_balance(&svm, &i.pubkey()))
        .sum();
    assert_eq!(paid, collected.funds + collected.fee + collected.tax);
    ledger.track(listing_payment_ata(ASSET));
    assert_eq!(balance_at(&svm, &listing_payment_ata(ASSET)), paid);
    for (investor, shares) in investors.iter().zip(SHARES) {
        assert_eq!(holding_of(&svm, ASSET, &investor.pubkey()).amount, shares);
        assert_eq!(
            token_balance(&svm, &investor_share_ata(ASSET, &investor.pubkey())),
            shares as u64
        );
    }
    assert_eq!(property_of(&svm, ASSET).holder_count, 4);

    // --- legal: lawyers, the SPV election, documents, settlement ---
    let dev_lawyer = new_registered_lawyer(&mut svm, &admin, REGION);
    ok(
        &mut svm,
        assign_dev_lawyer_ix(&developer.pubkey(), ASSET, &dev_lawyer.pubkey()),
        &developer,
        &[&developer],
    );
    let spv_lawyer = new_registered_lawyer(&mut svm, &admin, REGION);
    ok(
        &mut svm,
        claim_spv_ix(&spv_lawyer.pubkey(), ASSET, 1, COSTS),
        &spv_lawyer,
        &[&spv_lawyer, &spn],
    );
    ok(
        &mut svm,
        vote_spv_ix(
            &investors[0].pubkey(),
            ASSET,
            1,
            &spv_lawyer.pubkey(),
            None,
            SHARES[0],
        ),
        &spn,
        &[&spn, &investors[0]],
    );
    assert_eq!(
        holding_of(&svm, ASSET, &investors[0].pubkey()).locked(),
        SHARES[0]
    );
    warp(&mut svm, 10_001);
    ok(
        &mut svm,
        finalize_spv_ix(
            &cranker.pubkey(),
            ASSET,
            1,
            Some(&spv_lawyer.pubkey()),
            &[spv_lawyer.pubkey()],
        ),
        &cranker,
        &[&cranker],
    );
    ok(
        &mut svm,
        unlock_votes_ix(&investors[0].pubkey(), ASSET, 1),
        &investors[0],
        &[&investors[0]],
    );
    assert_eq!(holding_of(&svm, ASSET, &investors[0].pubkey()).locked(), 0);
    for lawyer in [&dev_lawyer, &spv_lawyer] {
        ok(
            &mut svm,
            confirm_docs_ix(&lawyer.pubkey(), ASSET, true, DOCS),
            lawyer,
            &[lawyer],
        );
    }
    assert_eq!(listing_of(&svm, ASSET).status, ListingStatus::Legal);

    for wallet in [
        &developer.pubkey(),
        &dev_lawyer.pubkey(),
        &spv_lawyer.pubkey(),
        &operator.pubkey(),
    ] {
        ledger.fund(&mut svm, wallet, 0);
    }
    set_token_account_for(
        &mut svm,
        tgbp_mint(),
        treasury_payment_ata(),
        &treasury(),
        0,
    );
    ledger.track(treasury_payment_ata());
    ok(
        &mut svm,
        execute_deal_ix(
            &cranker.pubkey(),
            ASSET,
            REGION,
            &developer.pubkey(),
            &dev_lawyer.pubkey(),
            &spv_lawyer.pubkey(),
            &operator.pubkey(),
            &[tgbp_mint()],
        ),
        &cranker,
        &[&cranker],
    );
    assert_eq!(listing_of(&svm, ASSET).status, ListingStatus::Finalized);
    assert!(property_of(&svm, ASSET).finalized);
    // The split: the developer nets the principal less the seller fee, the
    // tax rides with the SPV lawyer along with their costs drawn from the
    // fee pot, the region's operator takes the configured share of what is
    // left, and the treasury sweeps the rest. Vault empty, deposit back.
    let seller_fee = bps(collected.funds, SELLER_FEE_BPS);
    let pot = collected.fee + seller_fee;
    let spv_cut = COSTS.min(pot);
    let region_share = bps(pot - spv_cut, 6_700);
    let developer_share = collected.funds - seller_fee;
    let spv_share = collected.tax + spv_cut;
    assert_eq!(tgbp_balance(&svm, &developer.pubkey()), developer_share);
    assert_eq!(tgbp_balance(&svm, &dev_lawyer.pubkey()), 0);
    assert_eq!(tgbp_balance(&svm, &spv_lawyer.pubkey()), spv_share);
    assert_eq!(tgbp_balance(&svm, &operator.pubkey()), region_share);
    assert_eq!(
        token_balance(&svm, &treasury_payment_ata()),
        paid - developer_share - spv_share - region_share
    );
    assert_eq!(balance_at(&svm, &listing_payment_ata(ASSET)), 0);
    assert_eq!(balance_at(&svm, &token_acc(&developer.pubkey())), FUND_XCAV);

    // --- property: a letting agent is elected by the new holders ---
    let agent = role_holder(&mut svm, &admin, Role::LettingAgent);
    ok(
        &mut svm,
        add_agent_ix(&agent.pubkey(), REGION, POSTCODE),
        &agent,
        &[&agent],
    );
    assert_eq!(
        balance_at(&svm, &xcav_ata(&agent.pubkey())),
        FUND_XCAV - AGENT_DEPOSIT
    );
    ok(
        &mut svm,
        claim_property_ix(&agent.pubkey(), ASSET, 1),
        &agent,
        &[&agent],
    );
    for (investor, shares) in investors.iter().zip(SHARES).take(2) {
        ok(
            &mut svm,
            vote_agent_ix(&investor.pubkey(), ASSET, 1, &agent.pubkey(), shares),
            investor,
            &[investor, &spn],
        );
        assert_eq!(holding_of(&svm, ASSET, &investor.pubkey()).locked(), shares);
    }
    warp(&mut svm, AGENT_VOTING_TIME + 1);
    ok(
        &mut svm,
        finalize_election_ix(&cranker.pubkey(), ASSET, 1, &agent.pubkey()),
        &cranker,
        &[&cranker],
    );
    assert_eq!(letting_of(&svm, ASSET).agent, agent.pubkey());
    assert_eq!(
        agent_of(&svm, &agent.pubkey()).locations[0].assigned_count,
        1
    );
    for investor in investors.iter().take(2) {
        ok(
            &mut svm,
            unlock_agent_votes_ix(&investor.pubkey(), ASSET, 1),
            investor,
            &[investor],
        );
        assert_eq!(holding_of(&svm, ASSET, &investor.pubkey()).locked(), 0);
    }

    // --- income round one: rent lands pro rata, no dust at 100 shares ---
    ledger.fund(&mut svm, &agent.pubkey(), 10 * RENT);
    ledger.track(income_vault_ata(ASSET, &tgbp_mint()));
    ok(
        &mut svm,
        distribute_ix(&agent.pubkey(), ASSET, RENT),
        &agent,
        &[&agent],
    );
    let stream = &income_of(&svm, ASSET).streams[0];
    assert_eq!(stream.mint, tgbp_mint());
    assert_eq!(stream.per_share, (RENT / 100) as u128);
    assert_eq!(stream.dust, 0);
    for (investor, shares) in investors.iter().zip(SHARES) {
        let before = tgbp_balance(&svm, &investor.pubkey());
        ok(
            &mut svm,
            claim_income_ix(&investor.pubkey(), ASSET),
            investor,
            &[investor, &spn],
        );
        assert_eq!(
            tgbp_balance(&svm, &investor.pubkey()) - before,
            shares as u64 * (RENT / 100)
        );
    }
    assert_eq!(balance_at(&svm, &income_vault_ata(ASSET, &tgbp_mint())), 0);

    // --- governance: a mid-tier works proposal passes by share vote ---
    let logs = logs_of(
        &mut svm,
        propose_ix(&agent.pubkey(), ASSET, 1, WORKS),
        &agent,
        &[&agent],
    );
    assert!(!emitted(&logs, ProposalExecuted::DISCRIMINATOR));
    assert_eq!(letting_of(&svm, ASSET).governance.active_proposal, 1);
    for (investor, shares) in investors.iter().zip(SHARES).take(2) {
        ok(
            &mut svm,
            vote_proposal_ix(&investor.pubkey(), ASSET, 1, shares),
            investor,
            &[investor, &spn],
        );
    }
    warp(&mut svm, PROPOSAL_VOTING_TIME + 1);
    let logs = logs_of(
        &mut svm,
        finalize_proposal_ix(&cranker.pubkey(), &agent.pubkey(), ASSET, 1),
        &cranker,
        &[&cranker],
    );
    assert!(emitted(&logs, ProposalExecuted::DISCRIMINATOR));
    assert_eq!(letting_of(&svm, ASSET).governance.active_proposal, 0);
    for investor in investors.iter().take(2) {
        ok(
            &mut svm,
            unlock_proposal_votes_ix(&investor.pubkey(), ASSET, 1),
            investor,
            &[investor],
        );
        assert_eq!(holding_of(&svm, ASSET, &investor.pubkey()).locked(), 0);
    }

    // --- secondary: a relist bought in part, the rest taken by offer ---
    let seller = &investors[1];
    ok(
        &mut svm,
        relist_ix(&seller.pubkey(), ASSET, 0, 20, ASK),
        seller,
        &[seller],
    );
    let share_listing = share_listing_of(&svm, 0);
    // Secondary fees come from the live region too.
    assert_eq!(
        (share_listing.seller_fee_bps, share_listing.buyer_fee_bps),
        (SELLER_FEE_BPS, BUYER_FEE_BPS)
    );
    let buyer = ledger.investor(&mut svm, &admin);
    ledger.track(payment_ata(&seller.pubkey(), &tgbp_mint()));
    ledger.track(payment_ata(&operator.pubkey(), &tgbp_mint()));
    let treasury_before = token_balance(&svm, &treasury_payment_ata());
    ok_with_budget(
        &mut svm,
        buy_relisted_ix(&buyer.pubkey(), ASSET, 0, &seller.pubkey(), 12, u64::MAX),
        &buyer,
        &[&buyer],
    );
    let gross = 12 * ASK;
    let buyer_fee = bps(gross, BUYER_FEE_BPS);
    let seller_fee = bps(gross, SELLER_FEE_BPS);
    let operator_cut = bps(buyer_fee + seller_fee, 6_700);
    assert_eq!(
        tgbp_balance(&svm, &buyer.pubkey()),
        INVESTOR_FUNDS - gross - buyer_fee
    );
    assert_eq!(
        balance_at(&svm, &payment_ata(&seller.pubkey(), &tgbp_mint())),
        gross - seller_fee
    );
    assert_eq!(
        balance_at(&svm, &payment_ata(&operator.pubkey(), &tgbp_mint())),
        operator_cut
    );
    assert_eq!(
        token_balance(&svm, &treasury_payment_ata()) - treasury_before,
        buyer_fee + seller_fee - operator_cut
    );
    assert_eq!(share_listing_of(&svm, 0).amount, 8);

    let offeror = ledger.investor(&mut svm, &admin);
    ok(
        &mut svm,
        make_offer_ix(
            &offeror.pubkey(),
            0,
            8,
            BID,
            tgbp_mint(),
            tgbp_acc(&offeror.pubkey()),
        ),
        &offeror,
        &[&offeror],
    );
    let offer_vault = payment_ata(&offer_vault_pda(0, &offeror.pubkey()), &tgbp_mint());
    ledger.track(offer_vault);
    let bid_total = 8 * BID;
    assert_eq!(
        balance_at(&svm, &offer_vault),
        bid_total + bps(bid_total, BUYER_FEE_BPS)
    );
    ok_with_budget(
        &mut svm,
        accept_offer_ix(
            &seller.pubkey(),
            ASSET,
            0,
            &offeror.pubkey(),
            0,
            tgbp_mint(),
        ),
        seller,
        &[seller],
    );
    assert!(svm.get_account(&share_listing_pda(0)).is_none());
    assert_eq!(balance_at(&svm, &offer_vault), 0);
    assert_eq!(holding_of(&svm, ASSET, &seller.pubkey()).listed, 0);

    // And a plain transfer between two of the original investors.
    ok(
        &mut svm,
        send_shares_ix(&investors[2].pubkey(), &investors[3].pubkey(), ASSET, 4),
        &investors[2],
        &[&investors[2]],
    );

    let holders: Vec<&Keypair> = investors.iter().chain([&buyer, &offeror]).collect();
    let expected = [34u32, 13, 20, 13, 12, 8];
    for (holder, shares) in holders.iter().zip(expected) {
        let holding = holding_of(&svm, ASSET, &holder.pubkey());
        assert_eq!(holding.amount, shares);
        assert_eq!(holding.locked(), 0);
        assert_eq!(holding.listed, 0);
        assert_eq!(
            token_balance(&svm, &investor_share_ata(ASSET, &holder.pubkey())),
            shares as u64
        );
    }
    assert_eq!(expected.iter().sum::<u32>(), SHARE_AMOUNT);
    assert_eq!(property_of(&svm, ASSET).holder_count, 6);

    // --- income round two: the ledger followed the shares ---
    ok(
        &mut svm,
        distribute_ix(&agent.pubkey(), ASSET, RENT),
        &agent,
        &[&agent],
    );
    for (holder, shares) in holders.iter().zip(expected) {
        let before = tgbp_balance(&svm, &holder.pubkey());
        ok(
            &mut svm,
            claim_income_ix(&holder.pubkey(), ASSET),
            holder,
            &[holder, &spn],
        );
        assert_eq!(
            tgbp_balance(&svm, &holder.pubkey()) - before,
            shares as u64 * (RENT / 100)
        );
    }
    assert_eq!(balance_at(&svm, &income_vault_ata(ASSET, &tgbp_mint())), 0);

    // --- nothing leaked: every tGBP minted is still on the books, and the
    // three XCAV vaults hold exactly the live stakes ---
    assert_eq!(ledger.total(&svm), ledger.minted);
    assert_eq!(
        balance_at(&svm, &regions_vault()),
        REGION_BOND + LOCATION_DEPOSIT
    );
    assert_eq!(vault_balance(&svm), 2 * LAWYER_DEPOSIT);
    assert_eq!(balance_at(&svm, &property_vault()), AGENT_DEPOSIT);
}
