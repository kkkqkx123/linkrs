#!/usr/bin/env bash
# Compile and validate the read-only compiled-plan memo on devs.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

LOG=/data/kkkqkx/linkrs/compiled-plan-memo.log
: > "$LOG"

step() {
  local label="$1"
  shift
  echo "=== $label ===" | tee -a "$LOG"
  "$@" >> "$LOG" 2>&1
  local code=$?
  echo "${label}_EXIT=$code" | tee -a "$LOG"
  tail -n 8 "$LOG"
  return "$code"
}

step check cargo check -p linkrs-query -p linkrs-api -p linkrs || true
step memo_tests cargo test -p linkrs-query --test compiled_plan_memo -- --nocapture || true
step dql cargo test -p linkrs-query --test dql || true
step parallel_partition cargo test -p linkrs-query --test parallel_partition_execution || true
step dml_memo cargo test -p linkrs-query --test dml || true
step optimizer_lib cargo test -p linkrs-query --lib -- partitioning local_topk feedback || true
step cache_lib cargo test -p linkrs-query --lib -- cache || true
step api_lib cargo test -p linkrs-api --lib || true
step bench_build cargo build --release --features embedded -p linkrs --bin linkrs-parity-bench || true

echo "COMPILED_PLAN_MEMO_DONE"
