// The controller ACCOUNT: derive the registered-model lock (owner_hash ‖
// session_params) from config + manifest — byte-identical to the deployed cell
// (the same derivation the Rust CLI, the Node drivers, and test-reconstruct use).
import type { Artifact, ControllerConfig, ControllerWasm, JsonScript, NetworkManifest } from "./types.js";
import { keyFromHex, type KeyPair } from "./keys.js";
import { CKB } from "./types.js";

/**
 * Per-user keypair override. When passed to deriveAccount/Controller.load these
 * replace config.session.{owner,session}Privkey; the rest of the session policy
 * (expiresAt, policiesRoot, spendCapCkb, guardian) is unchanged — so each user
 * gets a distinct account/lock/address under the same policy.
 */
export interface UserKeys {
  owner: KeyPair;
  session: KeyPair;
}

export interface Account {
  address: string;
  /** The account lock in JSON-RPC form (fiber-js `funding_lock_script` shape). */
  lockScript: JsonScript;
  /** Molecule-serialized lock (what wasm ChannelSession/script() produce). */
  lockMolecule: string;
  args: string;
  /** [lock dep, auth dep] in fiber-js cell-dep form. */
  lockCellDeps: { dep_type: string; out_point: { tx_hash: string; index: string } }[];
  owner: KeyPair;
  session: KeyPair;
  spendCapShannons: bigint;
}

const HT_DATA2 = 0x04;

function art(net: NetworkManifest, name: string): Artifact {
  const a = net[name];
  if (!a) throw new Error(`manifest has no "${name}" entry for this network — deploy first (ckb-controller deploy)`);
  return a;
}

/**
 * Derive the account from config session policy + the network's lock deploy.
 * `keys` overrides only the two config privkeys (per-user identity); absent, the
 * output is byte-identical to the fixed-key path Node drivers/tests rely on.
 */
export function deriveAccount(config: ControllerConfig, net: NetworkManifest, wasm: ControllerWasm, keys?: UserKeys): Account {
  const lockArt = art(net, "lock");
  const authArt = art(net, "auth");
  const s = config.session;

  const owner = keys?.owner ?? keyFromHex(s.ownerPrivkey);
  const session = keys?.session ?? keyFromHex(s.sessionPrivkey);
  const expires = s.expiresAt === "never" ? wasm.no_expiry() : BigInt(s.expiresAt);
  const root = s.policiesRoot === "wildcard" ? wasm.wildcard_root() : s.policiesRoot;
  const spendCapShannons = BigInt(s.spendCapCkb) * CKB;
  const guardian = s.guardian ?? "0x" + "00".repeat(20);

  const params = wasm.session_params(session.pubHash, expires, root, spendCapShannons.toString(), guardian);
  const args = wasm.registered_args(owner.pubHash, params);
  const address = wasm.controller_address(lockArt.codeHash!, HT_DATA2, args, true /* ckt prefix */);

  return {
    address,
    lockScript: { code_hash: lockArt.codeHash!, hash_type: "data2", args },
    lockMolecule: wasm.script(lockArt.codeHash!, HT_DATA2, args),
    args,
    lockCellDeps: [lockArt, authArt].map((a) => ({
      dep_type: a.depType === "depGroup" ? "dep_group" : "code",
      out_point: { tx_hash: a.dep.txHash, index: a.dep.index },
    })),
    owner,
    session,
    spendCapShannons,
  };
}
