# zcash-sync-evals

A benchmark of Zcash wallet sync, run on GitHub runners with attested results.

Each wallet restores the public BIP-39 test seed and syncs a range of mainnet
blocks against lightwalletd. The node behind lightwalletd is `chain-index`,
which answers from the compact blocks themselves, frozen at the range's tip.
It refuses any question it cannot answer exactly, and a refusal fails the run.

Wallets are run one at a time on the same runner, with the same network limits
and a cold page cache, in 4 rounds of alternating order. Each is timed from
launch to exit of its `sync` step, and must end at the tip.

To run it: Actions → bench → Run workflow.

Each range's chain data is a release asset, pinned by SHA-256 in `bench.yml`.
