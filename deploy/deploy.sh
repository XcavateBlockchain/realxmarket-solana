#!/usr/bin/env bash
# Deploys realXmarket to devnet (or a local validator) and bootstraps it.
#
#   ./deploy/deploy.sh              # devnet
#   CLUSTER=localnet ./deploy/deploy.sh
#
# Creates the team keypairs under deploy/keys/ on first run. Share that
# folder with the team over a private channel, it never goes into git.
# Re-running is safe: every step skips what already exists.
set -euo pipefail

cd "$(dirname "$0")/.."
KEYS=deploy/keys
CLUSTER=${CLUSTER:-devnet}
case $CLUSTER in
  devnet)   RPC_URL=${RPC_URL:-https://api.devnet.solana.com} ;;
  localnet) RPC_URL=${RPC_URL:-http://127.0.0.1:8899} ;;
  *) echo "unsupported cluster: $CLUSTER (this script is for devnet/localnet only)"; exit 1 ;;
esac
AUTH=$KEYS/authority.json

# --- keypairs ---------------------------------------------------------------
mkdir -p $KEYS
chmod 700 $KEYS
for name in authority admin treasury sponsor operator developer investor1 investor2 \
            lawyer1 lawyer2 spv-confirmer letting-agent \
            xcav-mint tgbp-mint tusdc-mint; do
  if [[ ! -f $KEYS/$name.json ]]; then
    solana-keygen new --no-bip39-passphrase -s -o $KEYS/$name.json > /dev/null
    echo "created key: $name"
  fi
done
pubkey() { solana-keygen pubkey "$KEYS/$1.json"; }

# --- SOL --------------------------------------------------------------------
# The five program deploys dominate (~3.9MB of binaries, rent scales with
# size); actors just need fees and account rent.
TARGET_SOL=30
balance() { solana balance -u "$RPC_URL" "$(pubkey authority)" | cut -d' ' -f1; }
if (( $(echo "$(balance) < $TARGET_SOL" | bc -l) )); then
  echo "funding authority ($(pubkey authority))"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    solana airdrop 5 -u "$RPC_URL" "$(pubkey authority)" > /dev/null 2>&1 || true
    (( $(echo "$(balance) >= $TARGET_SOL" | bc -l) )) && break
    sleep 2
  done
fi
if (( $(echo "$(balance) < $TARGET_SOL" | bc -l) )); then
  echo "airdrop rate-limited: fund $(pubkey authority) with ~$TARGET_SOL SOL"
  echo "(https://faucet.solana.com) and re-run"
  exit 1
fi
for name in admin sponsor operator developer investor1 investor2 \
            lawyer1 lawyer2 spv-confirmer letting-agent; do
  bal=$(solana balance -u "$RPC_URL" "$(pubkey $name)" 2>/dev/null | cut -d' ' -f1 || echo 0)
  if (( $(echo "$bal < 0.5" | bc -l) )); then
    solana transfer -u "$RPC_URL" -k $AUTH --allow-unfunded-recipient \
      "$(pubkey $name)" 0.7 > /dev/null
    echo "funded $name"
  fi
done

# --- mints ------------------------------------------------------------------
# The mint guard rejects freeze authorities, so none of these get one
# (spl-token only adds it with --enable-freeze). Devnet USDC would be
# rejected, which is why the test stablecoins exist.
create_mint() { # name decimals
  if ! solana account -u "$RPC_URL" "$(pubkey $1)" > /dev/null 2>&1; then
    spl-token create-token -u "$RPC_URL" --fee-payer $AUTH --mint-authority $AUTH \
      --decimals "$2" "$KEYS/$1.json" > /dev/null
    echo "created mint $1 ($2 decimals)"
  fi
}
create_mint xcav-mint 9
create_mint tgbp-mint 9
create_mint tusdc-mint 6
ata_of() { # mint owner-pubkey
  spl-token address --token "$1" --owner "$2" --verbose -u "$RPC_URL" \
    | grep -oP 'Associated token address: \K\S+'
}

# XCAV is fixed supply: top the mint up to 100M in the authority's wallet,
# then drop the mint authority. The region bond is 0.1% of live supply, so
# this pins it at 100k XCAV instead of drifting with every test top-up.
XCAV=$(pubkey xcav-mint)
XCAV_SUPPLY=100000000
supply=$(spl-token supply -u "$RPC_URL" "$XCAV")
if (( $(echo "$supply < $XCAV_SUPPLY" | bc -l) )); then
  auth_ata=$(ata_of "$XCAV" "$(pubkey authority)")
  if ! solana account -u "$RPC_URL" "$auth_ata" > /dev/null 2>&1; then
    spl-token create-account -u "$RPC_URL" --fee-payer $AUTH \
      --owner "$(pubkey authority)" "$XCAV" > /dev/null
  fi
  spl-token mint -u "$RPC_URL" --fee-payer $AUTH --mint-authority $AUTH \
    "$XCAV" "$(echo "$XCAV_SUPPLY - $supply" | bc -l)" "$auth_ata" > /dev/null
  echo "minted XCAV up to the full $XCAV_SUPPLY supply"
fi
if ! spl-token display -u "$RPC_URL" "$XCAV" | grep -q 'Mint authority: (not set)'; then
  spl-token authorize -u "$RPC_URL" --fee-payer $AUTH --authority $AUTH \
    "$XCAV" mint --disable > /dev/null
  echo "dropped the XCAV mint authority, supply is now fixed"
fi

fund_token() { # mint-name owner-name ui-amount
  local mint owner ata
  mint=$(pubkey "$1"); owner=$(pubkey "$2")
  ata=$(ata_of "$mint" "$owner")
  if ! solana account -u "$RPC_URL" "$ata" > /dev/null 2>&1; then
    spl-token create-account -u "$RPC_URL" --fee-payer $AUTH --owner "$owner" "$mint" > /dev/null
    spl-token mint -u "$RPC_URL" --fee-payer $AUTH --mint-authority $AUTH \
      "$mint" "$3" "$ata" > /dev/null
    echo "minted $3 $1 to $2"
  fi
}
# XCAV comes out of the authority's pile. Tops the balance up to the target
# so re-runs restore anything spent on bonds and deposits. The operator needs
# the 100k region bond with room left over to vote.
send_xcav() { # owner-name target-ui-amount
  local owner bal
  owner=$(pubkey "$1")
  bal=$(spl-token balance -u "$RPC_URL" --owner "$owner" "$XCAV" 2>/dev/null || echo 0)
  if (( $(echo "$bal < $2" | bc -l) )); then
    spl-token transfer -u "$RPC_URL" --fee-payer $AUTH --owner $AUTH \
      --fund-recipient "$XCAV" "$(echo "$2 - $bal" | bc -l)" "$owner" > /dev/null
    echo "topped up $1 to $2 XCAV"
  fi
}
send_xcav operator      110000
send_xcav developer     10000
send_xcav lawyer1       1000
send_xcav lawyer2       1000
send_xcav letting-agent 1000
send_xcav investor1     1000
send_xcav investor2     1000
# Purchase money for the investors.
fund_token tgbp-mint  investor1 1000000
fund_token tgbp-mint  investor2 1000000
fund_token tusdc-mint investor1 1000000
fund_token tusdc-mint investor2 1000000

# --- programs ---------------------------------------------------------------
# Always build so a stale .so can never ship; SKIP_BUILD=1 skips it when the
# binaries are known fresh.
[[ ${SKIP_BUILD:-0} = 1 ]] || NO_DNA=1 anchor build
# Reclaim rent stranded in buffers by any earlier interrupted deploy.
solana program close --buffers -u "$RPC_URL" -k $AUTH > /dev/null 2>&1 || true
deploy_program() { # so-name
  local id
  id=$(solana-keygen pubkey "target/deploy/$1-keypair.json")
  if solana program show -u "$RPC_URL" "$id" > /dev/null 2>&1; then
    echo "upgrading $1 ($id)"
  else
    echo "deploying $1 ($id)"
  fi
  solana program deploy -u "$RPC_URL" -k $AUTH \
    --program-id "target/deploy/$1-keypair.json" "target/deploy/$1.so" > /dev/null
}
deploy_program xcavate_whitelist
deploy_program regions
deploy_program marketplace
deploy_program property
deploy_program bucket

# --- protocol bootstrap -----------------------------------------------------
cargo run -q -p xcavate-setup -- --url "$RPC_URL"

echo
echo "IDLs for the frontend: target/idl/*.json"
echo "addresses + actors:    deploy/addresses.json"
echo "team keys to share:    deploy/keys/ (private channel only)"
