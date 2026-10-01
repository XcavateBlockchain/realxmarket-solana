// Changes single settings in the marketplace config, the rest stays as it is.
//
//   npx ts-node deploy/update-config.ts

import * as anchor from "@anchor-lang/core";
import fs from "fs";
import path from "path";

const RPC = "https://api.devnet.solana.com";
const AUTHORITY_KEYPAIR = "keys/authority.json";
// devnet still runs on an older build, so this is the IDL of that build
const IDL = "idl/marketplace-devnet.json";
const CHANGES = { claimingTime: new anchor.BN(1_800) }; // 30 minutes

const readJson = (file: string) =>
  JSON.parse(fs.readFileSync(path.join(__dirname, file), "utf8"));

async function main() {
  const authority = anchor.web3.Keypair.fromSecretKey(
    Uint8Array.from(readJson(AUTHORITY_KEYPAIR))
  );
  const provider = new anchor.AnchorProvider(
    new anchor.web3.Connection(RPC, "confirmed"),
    new anchor.Wallet(authority),
    { commitment: "confirmed" }
  );
  const program = new anchor.Program(readJson(IDL), provider);
  const accounts = program.account as any;
  const [config] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("config")],
    program.programId
  );

  // update_config replaces every setting, so the current ones go back in.
  // What isn't a setting (authority, bump, ...) just doesn't get encoded.
  const before = await accounts.config.fetch(config);
  const params = { ...before, ...CHANGES };

  const signature = await program.methods
    .updateConfig(params)
    .accounts({ authority: authority.publicKey, config })
    .remainingAccounts(
      params.acceptedPaymentMints.map((pubkey: anchor.web3.PublicKey) => ({
        pubkey,
        isSigner: false,
        isWritable: false,
      }))
    )
    .rpc();
  console.log("sent", signature);

  const after = await accounts.config.fetch(config);
  for (const name of Object.keys(CHANGES))
    console.log(name, String(before[name]), "->", String(after[name]));
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
