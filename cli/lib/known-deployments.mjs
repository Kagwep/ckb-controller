// Public deployments of the controller code cells, shipped with the CLI so a
// fresh project reuses them instead of redeploying ~314k CKB of binaries: a new
// game then only needs its own game cell + account cell (~1k CKB, one faucet
// claim covers hundreds). `ckb-controller deploy` verifies these are still live
// and deploys fresh copies only if not (or if you prefer your own, delete the
// entries from your manifest and re-run deploy with the binaries built).
export const KNOWN_DEPLOYMENTS = {
  testnet: {
    lock: {
      codeHash: "0x9d3ce3e29c65467fdff3ece23883e54a5fb03e677d9da80879691a9823034a9c",
      hashType: "data2",
      dep: { txHash: "0x2d754da027c1c90dad7169c55cdef666644258c1e5bf02f49b112bf525fc9b93", index: "0x0" },
      depType: "code",
    },
    auth: {
      dep: { txHash: "0x539e202c058680b1945352800ad8d6edaaf2ec2034d6b2d575aad423bf1a401c", index: "0x0" },
      depType: "code",
    },
    secp256k1Sighash: {
      dep: { txHash: "0xf8de3bb47d055cdf460d93a2a6e1b05f7432f9777c8c474abf4eec1d4aee5d37", index: "0x0" },
      depType: "depGroup",
    },
    game: {
      codeHash: "0x81fa44f5eb7209d4ef5b2c5b10679eac1ff8d76b18ee8006af48b2c76e330d6c",
      hashType: "data2",
      dep: { txHash: "0x2d3cda90d8b348ab28a6f55d87e11b580eec00419b6d67318a3ba92b52bca17b", index: "0x0" },
      depType: "code",
    },
  },
};
