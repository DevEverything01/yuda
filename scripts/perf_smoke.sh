#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

cargo test --locked --features linux-runtime
cargo clippy --locked --features linux-runtime --all-targets -- -D warnings

printf '%s\n' '--- size ---'
if [[ -x target/release/yuda ]]; then
  wc -c target/release/yuda
fi
printf '%s\n' '--- config boundary ---'
cargo test --locked config::tests::defaults_are_chinese_first_and_round_trip -- --exact
printf '%s\n' '--- injection boundary ---'
cargo test --locked injection::tests::clipboard_sequence_restores_after_paste -- --exact
