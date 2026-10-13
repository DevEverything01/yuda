#!/usr/bin/env bash
# Linux 开发机全量冒烟：完整 daemon 的 clippy + 测试 + 产物体积 + 关键边界用例。
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

cargo clippy --locked --features daemon --all-targets -- -D warnings
cargo test --locked --features daemon

printf '%s\n' '--- size ---'
if [[ -x target/release/yuda ]]; then
  wc -c target/release/yuda
fi
printf '%s\n' '--- config boundary ---'
cargo test --locked --lib config::tests::example_config_documents_the_defaults -- --exact
printf '%s\n' '--- session boundary ---'
cargo test --locked --lib session::tests::exact_minimum_hold_is_accepted -- --exact
printf '%s\n' '--- injection contract ---'
cargo test --locked --test injection_mock
