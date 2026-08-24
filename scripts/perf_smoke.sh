#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

cargo test --features linux-runtime
cargo clippy --features linux-runtime --all-targets -- -D warnings

printf '%s\n' '--- size ---'
if [[ -x target/release/yuda ]]; then
  wc -c target/release/yuda
fi
printf '%s\n' '--- config boundary ---'
cargo test config::tests::defaults_are_chinese_first_and_round_trip -- --exact
printf '%s\n' '--- injection boundary ---'
cargo test injection::tests::clipboard_sequence_restores_after_paste -- --exact
