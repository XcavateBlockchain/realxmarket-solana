# realXhub

This workspace contains the existing Xcavate property programs and a new `realxhub` program for hub proposals and token purchases. The hub program keeps both flows under one program ID, with separate instruction files for proposals, activation and purchases.

An operator creates a draft, submits it for review, and an authorized verifier approves or rejects the submitted revision. An approved hub can lock XCAV, mint its fixed token supply and open a sale. Buyers pay into escrow and claim their tokens when the whole supply sells. If the deadline arrives before sellout, buyers can recover their payments and the operator can recover the XCAV bond.

## What changed

- Added `programs/realxhub`, following the workspace's handler, account, error and event conventions.
- Reused the whitelist's roles and the regions program's ownership records.
- Added per-hub token, payment and bond vaults, plus one purchase position per buyer and hub.
- Added optional unpaid reservations in `src/instructions/reservation.rs`, with separate accounts so existing paid positions and refunds remain readable.
- Added LiteSVM tests that run through actual role grants, region creation, proposal review, minting, purchases and refunds.
- Registered the hub program in `Anchor.toml` and aligned the whitelist address there with its existing Rust declaration.

The existing property programs and their deployment script keep their current behavior.

## Build and test

Use the workspace's Rust toolchain (`rust-toolchain.toml`), Anchor CLI and Solana SBF build tools. This change was checked with Rust 1.89.0, Anchor CLI 1.1.2 and Solana CLI 3.1.10. The committed lockfile resolves Anchor Rust crates to 1.1.2; the workspace's compatible `1.0.0` dependency declarations can still make the CLI print a version warning.

From the repository root:

```sh
anchor build --ignore-keys
cargo test -p realxhub --locked
```

The first command builds all workspace programs, including the whitelist and regions binaries used by the new tests. `--ignore-keys` keeps the build from rewriting source program IDs to match newly generated local deployment keys. It is for building and testing; it does not reconcile deployment identities.

Tests load `target/deploy/*.so` into LiteSVM. They do not need a running validator, a wallet file or SOL. Rebuild after changing program code so tests execute the current binary.

To run the full regression suite and check the new Rust files:

```sh
cargo test --workspace --locked
cargo fmt -p realxhub -- --check
cargo clippy -p realxhub --all-targets --locked -- -D warnings
```

## Configure and use the hub program

Initialize the hub config once, signed by the program's upgrade authority. Supply an XCAV mint, a different payment mint, a verifier public key and a positive bond amount. Amounts use the corresponding mint's smallest units. Both input mints must be classic SPL Token mints without freeze authorities; Token-2022 payment and bond mints are intentionally outside this first version.

Before proposing a hub, initialize the existing whitelist and regions programs, assign a compliant `RegionalOperator` role and create a region owned by that operator. Buyers use the existing compliant `RealEstateInvestor` role for now. Role assignment currently marks a wallet compliant; identity checks happen outside these programs and must precede the grant.

| Instruction | Purpose |
|---|---|
| `initialize_config` | Set the verifier, mints and bond amount |
| `create_proposal` | Create a draft for a region the operator owns |
| `update_proposal` | Edit a draft or rejected application, incrementing its revision |
| `submit_proposal` | Lock the proposal version for review |
| `review_proposal` | Approve/reject that exact revision and metadata hash |
| `activate_hub` | Deposit XCAV, create and mint the full supply, then open the sale |
| `buy_tokens` | Deposit payment and record a token allocation |
| `claim_tokens` | Deliver a buyer's allocation after sellout |
| `finalize_sale` | Mark an unsold sale failed at or after its deadline; anyone may call |
| `refund_purchase` | Return a failed-sale buyer's recorded payment once |
| `refund_bond` | Return a failed-sale operator's recorded XCAV bond once |
| `open_reservations` | Switch an unpurchased listed hub to unpaid reservations |
| `reserve_tokens` | Record quantity and payment quote without transferring assets |
| `cancel_reservation` | Let a buyer clear their promise before full reservation |
| `release_reservation` | Let anyone clear one expired unpaid promise |
| `finalize_reservations` | Fail an expired, entirely unpaid campaign so its bond can be returned |

`ProposalParams` carries the name, metadata URI/hash, token price, supply and sale duration. The reviewer must check the off-chain documents against that hash and the on-chain terms. The program verifies the reviewer's signature, revision and hash; it does not fetch documents or run AI. A reviewer cannot review their own hub. Rejected proposals require an edit and a new submission. Submitted or approved terms cannot be edited.

Each hub has a numeric ID from `Config.next_hub_id`. The hub PDA uses `["hub", hub_id_le_bytes]`; its mint and three vaults use their named seeds and that ID. Purchase positions use `["position", hub_id_le_bytes, buyer]`. See `programs/realxhub/tests/common/mod.rs` for complete instruction/account builders and `target/idl/realxhub.json` for the generated interface.

For local deployment, use a local validator and deployment keypairs whose public keys match the declared program IDs. Do not deploy using unrelated generated keys or run `anchor keys sync` without reviewing its source changes. This checkout's new hub deployment key is under ignored `target/deploy`; preserve it if keeping that address. A fresh clone needs its own deployment identity, with the source and Anchor configuration updated together. `deploy/deploy.sh` still bootstraps the property system, not the new hub config.

## Unpaid reservations

After `activate_hub`, a compliant operator who still owns the region may call `open_reservations` before the sale deadline. It succeeds only if no tokens have been purchased or claimed and no payments have been recorded. Existing paid sales continue using `buy_tokens`, `claim_tokens` and their original refunds. Reservation hubs use `Reserving` and `Claiming`, so those paid-sale instructions cannot accidentally collect money in the new flow.

`reserve_tokens` requires a compliant investor, a positive quantity within the remaining supply and a caller-supplied maximum cost. It checks that the buyer's payment account covers the combined quoted payments for that account's unpaid **hub** reservations. It does not transfer stablecoins, deliver hub tokens or change `Hub.total_paid`, `Hub.tokens_sold` or `Hub.tokens_claimed`. Creating accounts still costs SOL rent and transaction fees.

| Account | PDA seeds | What it records |
|---|---|---|
| `ReservationSale` | `["reservation_sale", hub_id_le_bytes]` | Reserved supply and the three-day window |
| `HubReservation` | `["hub_reservation", hub_id_le_bytes, buyer]` | One buyer's quantity, quoted payment and bound payment account |
| `PaymentReservation` | `["payment_reservation", payment_token_account]` | Total unpaid hub quotes against that payment account |

The buyer may add to the same reservation using its original payment account. Cancellation clears the whole reservation and its quote; an eligible buyer can reserve again while the sale is open. These records stay allocated, so cancellation does not recover account rent. The hub ledger does not include reservations held by the separate marketplace program.

The last available token being reserved sets `Claiming`, `claim_started_at` and `claim_deadline` in that transaction. The deadline is exactly 259,200 seconds later, even if the reservation arrives just before the original sale deadline. Further reservations and cancellation cannot reset or reopen that round. Deadline boundaries use Solana's on-chain clock: entry requires `now < deadline`, expiry allows `now >= deadline`.

Anyone can call `release_reservation` after the sale deadline for a partially reserved hub, or after the three-day deadline for a fully reserved hub. It deducts only that buyer's recorded quantity and quote, without moving funds. Cancellation and cleanup have no role gate, so eligibility revocation cannot strand an old promise. `finalize_reservations` marks an expired campaign failed only when no paid or delivered allocations exist. The existing `refund_bond` then returns the operator's recorded bond once. Cleanup may run before or after finalization.

Reservation quotes are promises, not escrow or proof of future payment. Buyers can spend or close their payment accounts after reserving. Any future paid claim must recheck the account and collect payment and deliver tokens atomically. Reserving all tokens currently opens the window **without a paid claiming instruction**. This is a testable reservation foundation, not the completed funding lifecycle.

### Decisions needed for paid claims and milestone funding

The review leaves financial rules open. These need to be settled before collecting reservation payments:

- If some claims remain unpaid after three days, should the sale fail or reopen? If it fails after tokens were delivered, define how paid buyers recover funds and surrender those tokens.
- For a milestone default, should current holders or original buyers receive the remaining payment and XCAV? Define token surrender and rounding rules.
- Choose the XCAV price source and valuation time for a bond worth 30% of the proposal. The current fixed `Config.bond_amount` does not implement that valuation.
- Confirm when the 60-day clock starts, the assessment/appeal grace for evidence submitted on time, how service outages are handled and when a successful operator recovers the bond.

The 50% tranche releases, milestone evidence/assessment, 60-day default settlement and 30% bond valuation are not enabled. No automated assessment worker, frontend or backend exists in this workspace. The proposal verifier records a signed proposal review; that is not automated milestone assessment.

To check this part alone after building the workspace binaries:

```sh
anchor build -p realxhub --ignore-keys
cargo test -p realxhub --test reservation --locked
```

The reservation tests exercise the compiled program, including unchanged wallet balances, automatic window timing, several buyers, quotes across hubs, payment-account binding, failed-transaction rollback, role revocation, expiry cleanup and bond recovery.

## Rules and current scope

- XCAV is bonded **after approval**. A rejected application has not paid XCAV and has nothing to refund. The bond amount is recorded when its draft is created, and activation accepts a caller-supplied maximum.
- Sales are fixed price and all-or-nothing. The funding target is `token_price * token_supply`, computed with checked arithmetic. `buy_tokens` accepts a maximum cost and rejects buys at the exact deadline.
- Tokens have zero decimals and a fixed supply. Minting authority is removed at activation, and there is no freeze authority. They are standard transferable SPL tokens once claimed; no ownership, revenue or governance rights are implied by this implementation.
- A legacy paid purchase allocates tokens; it does not deliver transferable tokens before sellout. Failed-sale refunds do not require recovering tokens from buyers. Transaction fees and account rent are not part of payment refunds.
- Payments and bonds use separate vaults for each hub. Refund amounts come from recorded liabilities, not the current vault balance. Eligibility revocation does not block an existing refund or funded-sale token claim.
- Successful-sale payments and XCAV bonds stay in escrow. Tranche releases, automated milestone evidence assessment, buybacks, slashing and successful-hub bond release are not implemented yet. There is no successful-sale withdrawal instruction. The reviewed milestone design uses automated assessment rather than community voting.
- Config is fixed after initialization. Verifier rotation, cancelled-sale token cleanup and account-rent recovery are also future work. There is no frontend in this change.

This is the first proposal and purchase milestone. Define the release conditions and hub token rights before using it for real funding.
