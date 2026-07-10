// Phase-2 invoice-flow check (no test infra in this package): run after `npm run
// build`, plain node, no chain. Proves the mock invoice surface end-to-end:
// two MockRails on one page-equivalent (module registry), B issues an invoice,
// A pays it, B's waitInvoicePaid resolves, and the budget accounting is exact.
// Loads the built dist submodules directly (avoids the live.ts fiber-js chain)
// and the canonical wasm build at wasm/pkg.
import { readFile } from "node:fs/promises";
import { MockRail } from "../dist/mock.js";

const ROOT = new URL("../../", import.meta.url); // repo root

const wasm = await import("../../wasm/pkg/controller.js");
await wasm.default({ module_or_path: await readFile(new URL("wasm/pkg/controller_bg.wasm", ROOT)) });

const LOCK = "0x" + "cd".repeat(32); // placeholder code hash (mock-only flavour)
const BUDGET = 500n;
const AMOUNT = 25n;

let pass = 0, fail = 0;
const check = (name, ok, got, exp) =>
  ok ? (pass++, console.log("  PASS", name)) : (fail++, console.log("  FAIL", name, "\n    got:", got, "\n    exp:", exp));
const rejects = async (p, re) => {
  try { await p; return false; } catch (e) { return re.test(String(e?.message ?? e)); }
};

const a = new MockRail(wasm, LOCK, BUDGET); // payer
const b = new MockRail(wasm, LOCK, BUDGET); // payee
await a.open(BUDGET);
await b.open(BUDGET);

const invoice = await b.newInvoice(AMOUNT, "p2p demo");
console.log("invoice:", invoice);
check("newInvoice returns a non-empty invoice string", typeof invoice === "string" && invoice.length > 0, invoice, "non-empty string");

// recipient starts waiting BEFORE the payer pays (the real-world order)
const paidPromise = b.waitInvoicePaid(invoice, 5000);
await a.payInvoice(invoice, { trampolineHops: ["0x02hub"], maxFeeCkb: 10n });
await paidPromise;
check("waitInvoicePaid resolves after payInvoice", true);

check("payer spent == invoice amount", a.spentCkb() === AMOUNT, a.spentCkb(), AMOUNT);
check("payer remaining == budget - amount", a.remainingCkb() === BUDGET - AMOUNT, a.remainingCkb(), BUDGET - AMOUNT);
check("payee budget untouched", b.spentCkb() === 0n && b.remainingCkb() === BUDGET, `${b.spentCkb()}/${b.remainingCkb()}`, `0/${BUDGET}`);

check("double-pay rejects", await rejects(a.payInvoice(invoice), /already paid/i), "resolved or wrong error", "rejects with 'already paid'");
check("unknown invoice rejects", await rejects(a.payInvoice("mockfibt-bogus"), /unknown invoice/i), "resolved or wrong error", "rejects with 'unknown invoice'");
check("waitInvoicePaid on unknown invoice rejects", await rejects(b.waitInvoicePaid("mockfibt-bogus", 200), /unknown invoice/i), "resolved or wrong error", "rejects with 'unknown invoice'");

console.log(`\n${fail === 0 ? "ALL GREEN" : "FAILURES"}: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
