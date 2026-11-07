#!/bin/sh
# Zodl driver.
#
# Zodl's sync engine is Slipstream (zodl-inc/slipstream, AGPL). It runs here as
# its own `slipstream` binary and is never linked into this repo's code.
#
#   sync.sh sync <lightwalletd-url> <birthday> <mnemonic> <wallet-dir>
#       Restore from the seed and sync to the tip, then exit. The runner times
#       this whole command. Slipstream takes a viewing key, not a seed
#       (HOSTING.md), so the seed is turned into a UFVK first, the same way the
#       Zodl app hands keys to the engine; that counts as part of the restore.
#
#   sync.sh report <wallet-dir>
#       Untimed. Print what the sync reached as JSON.
set -eu
case ${1:-} in
sync)
    [ $# -eq 5 ] || { echo 'usage: sync.sh sync <lightwalletd-url> <birthday> <mnemonic> <wallet-dir>' >&2; exit 2; }
    server=$2 birthday=$3 mnemonic=$4 dir=$5
    mkdir -p "$dir/wallet"
    ufvk=$(slipstream derive-ufvk --network main --account 0 --mnemonic "$mnemonic" | grep -o 'uview1[0-9a-z]*')
    slipstream sync --network main --server "$server" --wallet-dir "$dir/wallet" --ufvk "$ufvk" \
        --birthday "$birthday" > "$dir/sync.log" 2>&1 || { cat "$dir/sync.log" >&2; exit 1; }
    ;;
report)
    [ $# -eq 2 ] || { echo 'usage: sync.sh report <wallet-dir>' >&2; exit 2; }
    log=$2/sync.log
    tip=$(sed -n 's/^synced to tip \([0-9]*\).*/\1/p' "$log")
    notes=$(sed -n 's/^notes found: //p' "$log")
    printf '{"wallet":"zodl","synced_height":%s,"notes_found":"%s"}\n' "${tip:-null}" "$notes"
    ;;
*)
    echo 'usage: sync.sh sync|report ...' >&2; exit 2 ;;
esac
