// Report the controller account's live cells (must be exactly ONE — the lock
// allows a single account input per tx) and the fnn ckb key's balance.
// Account lock + RPC come from controller.config.json / the manifest.
import { readFile } from "node:fs/promises";
import { ccc } from "@ckb-ccc/core";
import { RPC, accountLock } from "./controller-config.mjs";

const client = new ccc.ClientPublicTestnet({ url: RPC });
const { lock } = await accountLock();

let n = 0;
for await (const cell of client.findCellsByLock(lock, null, true)) {
  n++;
  console.log(
    `account cell ${n}: ${cell.outPoint.txHash}:${BigInt(cell.outPoint.index)} = ${
      cell.cellOutput.capacity / 100000000n
    } CKB`,
  );
}
console.log(n === 1 ? "OK: single account cell" : `PROBLEM: ${n} account cells (must be 1)`);

// fnn's ckb key (sighash) balance — it needs a reserve to accept the channel.
// NOTE: fnn ENCRYPTS this key file in place on first start, after which it can't
// be read here anymore — skip gracefully in that case.
const fnnKeyFile = process.argv[2] ?? "D:/projects/ckb-bin/work/fnn-testnet/ckb/key";
try {
  const priv = (await readFile(fnnKeyFile, "utf8")).trim();
  if (!/^(0x)?[0-9a-fA-F]{64}$/.test(priv)) throw new Error("key file is not plaintext hex (fnn has encrypted it)");
  const signer = new ccc.SignerCkbPrivateKey(client, "0x" + priv.replace(/^0x/, ""));
  const fnnLock = (await signer.getRecommendedAddressObj()).script;
  let total = 0n;
  for await (const cell of client.findCellsByLock(fnnLock, null, true)) total += cell.cellOutput.capacity;
  console.log(`fnn ckb key: ${await signer.getRecommendedAddress()}`);
  console.log(`fnn balance: ${ccc.fixedPointToString(total)} CKB`);
} catch (e) {
  console.log(`fnn balance: skipped (${e.message})`);
}
