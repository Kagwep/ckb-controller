// Report testnet balances for the deploy key: sighash lock + CCC EVM Omnilock (flag 0x12).
// usage: node check-balance.mjs
import { ccc } from "@ckb-ccc/core";
import { secp256k1 } from "@noble/curves/secp256k1";
import { keccak_256 } from "@noble/hashes/sha3";
import { readFile } from "node:fs/promises";
import { KEYFILE, makeSigner } from "./controller-config.mjs";

const priv = (await readFile(KEYFILE, "utf8")).trim();
const { client, signer } = await makeSigner();

async function report(name, lock) {
  let total = 0n, plain = 0n, n = 0;
  for await (const cell of client.findCellsByLock(lock, null, true)) {
    total += cell.cellOutput.capacity;
    if (cell.outputData === "0x" && !cell.cellOutput.type) plain += cell.cellOutput.capacity;
    n++;
  }
  const addr = ccc.Address.fromScript(lock, client).toString();
  console.log(`${name}: ${addr}`);
  console.log(`  cells=${n} total=${ccc.fixedPointToString(total)} CKB plain=${ccc.fixedPointToString(plain)} CKB\n`);
}

// 1) plain sighash (what the deploy scripts spend)
await report("sighash", (await signer.getRecommendedAddressObj()).script);

// 2) Omnilock ETH (flag 0x12) — where faucet claims land when using the CCC EVM address
const pub = secp256k1.getPublicKey(priv.replace(/^0x/, ""), false);
const ethAddr = keccak_256(pub.slice(1)).slice(-20);
const ethHex = Array.from(ethAddr, (b) => b.toString(16).padStart(2, "0")).join("");
console.log(`eth address: 0x${ethHex}`);
const omni = ccc.Script.from({
  // Omnilock testnet code cell
  codeHash: "0xf329effd1c475a2978453c8600e1eaf0bc2087ee093c3ee64cc96ec6847752cb",
  hashType: "type",
  args: "0x12" + ethHex + "00",
});
await report("omnilock-eth", omni);
