# realXhub

This Anchor workspace contains the existing Xcavate property programs and a new `realxhub` program that combines hub proposals and token purchases.

Operators create a draft and submit a specific revision for review. An authorized verifier approves or rejects it. Approved hubs can bond XCAV, mint a fixed token supply and open a sale. Buyers pay into escrow and claim tokens after sellout. An expired partial sale refunds buyers and returns the operator's bond.

The reviewed reservation flow is now available as an optional, unpaid first step. Before any purchases, an operator can open reservations. Buyers record a token quantity and stablecoin quote without transferring either asset. The last reservation starts a three-day window on chain. This implements reservation accounting and expiry recovery; paid claims and milestone funding still need the settlement decisions listed in the program guide.

## Changes

- Added proposal, activation and purchase handlers in `programs/realxhub`, following the existing Rust modules, account constraints, errors and events.
- Reused whitelist roles and region ownership, with separate vaults and purchase records for each hub.
- Added LiteSVM tests for the full journey and rejected actions.
- Added separate unpaid reservation records, balance checks across hub reservations, cancellation and permissionless expiry cleanup. Existing paid positions keep their original layout and refund behavior.
- Added comments explaining the accounting and deadline rules, plus integration tests for rollback, account isolation, eligibility and bond recovery after an unpaid campaign expires.
- Registered the program in `Anchor.toml` and corrected the whitelist address there to match its Rust declaration.
- Gave the existing secondary-sale income test an explicit compute budget so random PDA derivations cannot intermittently exhaust its default budget.

## Build and test

Use the toolchain in `rust-toolchain.toml`, Anchor CLI and Solana SBF build tools. This change was checked with Rust 1.89.0, Anchor CLI 1.1.2 and Solana CLI 3.1.10.

Run from the repository root:

```sh
anchor build --ignore-keys
cargo test -p realxhub --locked
cargo test --workspace --locked
cargo fmt -p realxhub -- --check
cargo clippy -p realxhub --all-targets --locked -- -D warnings
```

Tests load the built `.so` files into LiteSVM; they do not need a validator or wallet. Rebuild after changing program code. `--ignore-keys` prevents build-time program ID rewrites; deployment still requires keys matching the declared IDs. The CLI may warn about the compatible `1.0.0` dependency declarations, although the lockfile resolves Anchor crates to 1.1.2.

## Configuration and scope

Initialize the hub config using the program's upgrade authority. Choose a verifier, positive XCAV bond amount and two different classic SPL Token mints without freeze authorities: XCAV and the payment token. Prices and bonds use each mint's smallest units.

Operators need a compliant `RegionalOperator` role and must own the chosen region. Buyers currently reuse the compliant `RealEstateInvestor` role. Identity screening and document review happen off chain; the program records an authorized review tied to the submitted revision and hash.

This first version bonds XCAV after approval and uses fixed-price, all-or-nothing sales. Tokens are standard transferable SPL tokens after claiming; their economic and governance rights are not defined here.

Successful-sale payments and bonds remain in escrow. Milestone payouts, automated evidence assessment, buybacks, slashing, verifier rotation and a frontend are later work. The existing deployment script does not initialize the new hub program.

The new reservation flow cannot collect payment or deliver tokens yet. `Claiming` records the three-day window; it does not enable the existing paid-sale `claim_tokens` instruction. An unpaid campaign can expire, release its promises and return its recorded bond. The XCAV bond remains the configured fixed amount, not a calculated 30% value.

See [the hub program guide](programs/realxhub/README.md) for instructions, account derivations, deployment notes and the detailed rules.
