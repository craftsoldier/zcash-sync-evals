# zcash-sync-evals

A fair, attested benchmark of Zcash wallet sync. Each wallet restores the same
seed and syncs the same range of mainnet blocks, from a birthday to a fixed tip,
against an unmodified lightwalletd. The benchmark runs as a GitHub workflow,
directly on the runner with no containers, and GitHub attests to the results, so
anyone can check which commit, data and runner produced them.

Wallets today: **zodl** (the Slipstream sync engine, run as its own binary) and
**vizor**.

## How a run works

- lightwalletd serves compact blocks from its own cache and asks a node for
  everything else. That node is `chain-index serve`, frozen at the range's tip.
- `chain-index` rebuilds tree states, subtree roots and transparent UTXOs from
  the compact blocks themselves, so no full node is needed. lightwalletd's
  startup questions are answered from `node.json`, replies a real node gave once,
  rewritten to describe the tip.
- `chain-index verify` checks the index against answers a real node gave
  (`answers.jsonl`, published with the chain data), once, when the chain data
  is built.
- A question `chain-index` cannot answer exactly is refused and logged as MISS,
  never guessed. A run with any MISS is invalid.
- Each wallet driver has two steps: `sync` (restore from the seed and sync to
  the tip, then exit) and `report` (what the sync reached, as JSON). The runner
  times `sync` from launch to exit, the same way for every wallet; `report` is
  untimed and offline.
- A run counts only if every wallet reports the tip as its synced height.

## Fairness rules

- **Same inputs:** the same chain bytes, server, seed and birthday for every
  wallet. The seed is the public BIP-39 test seed ("abandon" x23 + "art"): the
  index tracks exactly its transparent addresses, so every question a wallet
  asks about its own coins has an exact answer.
- **Same machine:** one standard GitHub runner per range. lightwalletd and
  `chain-index` run on CPU 0; the wallet under test gets CPUs 1-3. Wallets run
  one at a time.
- **Same network:** every reply from lightwalletd is delayed 50 ms and capped
  at 100 Mbit/s.
- **Cold start:** the page cache is dropped before each wallet runs.
- **Same clock:** wallets are timed from outside, launch to exit.
- **Balanced order:** 4 rounds in ABBA order, so each wallet goes first twice.
- **As shipped:** default settings and the key type the wallet normally uses,
  with no per-wallet tuning.
- **Per range:** results are reported per range (median, spread and ratio),
  never as one combined score.

## Running it

Actions -> **bench** -> Run workflow. It is never run on push. Each range runs
on its own runner; a final job summarises every range and attests the summary
and the full results.

## Chain data

Each range's data is one release asset, `<range>.tar.zst`, with its SHA-256
pinned in `bench.yml`. It holds:

- `lwd/db/main/`: lightwalletd's cache (`lengths` and `blocks`) up to the
  range's tip. `blocks` is sparse: only block 0, the blocks from the last
  `chain-index` checkpoint below the birthday through the tip, and the last
  block of every completed subtree (wallets ask for all subtree roots, and
  lightwalletd reads those blocks to answer) are present. Every other block
  is a hole, so a read of it fails lightwalletd's checksum.
- `index/`: the `chain-index` index, built from the full cache, with the test
  seed's watched addresses.

The archives are built and extracted with GNU `tar --sparse` on Linux.
`answers.jsonl`, the real node's answers that `chain-index verify` checks the
index against, is published in the same release.

| Range | Birthday | Tip | Release |
|---|---|---|---|
| ironwood | 3,428,143 | 3,543,404 | `data-ironwood-v1` |

## Layout

```
.github/workflows/bench.yml   the benchmark
chain-index/                  Rust: builds the index from a lightwalletd cache,
                              serves it as the node, verifies it against a
                              real node's answers
wallets/                      one driver per wallet: sync, report
node.json                     a real node's replies to lightwalletd's startup
                              questions
```
