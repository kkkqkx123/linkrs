#!/usr/bin/env bash
# Build, verify, and measure the group_by/join_two_hop remediation work.
# A/B against the resource-aligned baseline; keeps the smallest verified set.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

BIND="numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31"
export BENCH_CPU_BINDING="$BIND"
export BENCH_NUMA_POLICY="cpunodebind=1,membind=1"

ROOT=/data/kkkqkx/linkrs
OUT_NAME="${GATE_OUT:-remediation-gate}"
OUT="$ROOT/$OUT_NAME"
mkdir -p "$OUT"

echo "=== build parity bench ==="
cargo build --release --features embedded --bin linkrs-parity-bench > "$OUT/build.log" 2>&1
BUILD_EXIT=$?
echo "BUILD_EXIT=$BUILD_EXIT"
tail -n 15 "$OUT/build.log"
if [ "$BUILD_EXIT" -ne 0 ] || [ ! -x target/release/linkrs-parity-bench ]; then
  echo "BUILD_FAILED"
  echo "REMEDIATION_GATE_DONE"
  exit 1
fi

echo "=== correctness gates ==="
run_test() {
  local label="$1"
  shift
  "$@" > "$OUT/$label.log" 2>&1
  local code=$?
  echo "${label^^}_EXIT=$code"
  tail -n 4 "$OUT/$label.log"
}

run_test config_defaults cargo test -p linkrs-query --lib -- default_config_releases_ordered_expand_and_count_frontier
run_test partitioning cargo test -p linkrs-query --lib -- partitioning
run_test typed_group cargo test -p linkrs-query --lib -- typed_group_count
run_test local_topk cargo test -p linkrs-query --lib -- local_topk
run_test degree_sketch cargo test -p linkrs-query --lib -- degree_sketch
run_test stats_lib cargo test -p linkrs-query --lib -- stats::
run_test parallel cargo test -p linkrs-query --test parallel_partition_execution
run_test dql cargo test -p linkrs-query --test dql
run_test storage cargo test -p linkrs-storage --lib
run_test dangling_token cargo test -p linkrs-storage --lib -- dangling_token
run_test column_batch cargo test -p linkrs-storage --test column_batch_test
run_test api_lib cargo test -p linkrs-api --lib

DB="$ROOT/parity/lsqb-sf0.1-v4/db"
BIN=target/release/linkrs-parity-bench
SKIP="$OUT/skip-gate.txt"
printf '%s\n' \
  'lsqb/operators/shortest_path Linkrs-only dialect is rejected by Ladybug and Neug' \
  > "$SKIP"

echo "=== fixed gate (explicit benchmark parallelism + count frontier + proof) ==="
for round in 1 2; do
  $BIND "$BIN" run \
    --db "$DB" --space lsqb \
    --benchmark-dir benches/parity/queries/lsqb \
    --warmup 1 --run 5 --threads 16 --parallel-workers 16 --max-partitions 10 \
    --ordered-parallel-expand on --count-frontier on \
    --prove-dangling-free lsqb \
    --require-parallel-explain lsqb/operators/group_by_count,lsqb/operators/distinct_limit \
    --no-snapshots --skip-file "$SKIP" \
    --explain-dir "$OUT/explain$round" --profile-runs 1 \
    --out "$OUT/round$round" > "$OUT/round$round.log" 2>&1
  echo "ROUND${round}_EXIT=$?"
  awk '/group_by_count|distinct_limit|join_two_hop|variable_path_count|resources:|dangling_proof/' "$OUT/round$round.log"
done

echo "=== production default gate with proof ==="
$BIND "$BIN" run \
  --db "$DB" --space lsqb \
  --benchmark-dir benches/parity/queries/lsqb \
  --warmup 1 --run 5 --threads 16 \
  --prove-dangling-free lsqb \
  --no-snapshots --skip-file "$SKIP" \
  --explain-dir "$OUT/explain-prod" --profile-runs 1 \
  --out "$OUT/prod" > "$OUT/prod.log" 2>&1
echo "PROD_EXIT=$?"
awk '/group_by_count|distinct_limit|join_two_hop|variable_path_count|resources:|dangling_proof/' "$OUT/prod.log"

echo "=== production default gate without proof (old path A/B) ==="
$BIND "$BIN" run \
  --db "$DB" --space lsqb \
  --benchmark-dir benches/parity/queries/lsqb \
  --warmup 1 --run 5 --threads 16 \
  --no-snapshots --skip-file "$SKIP" \
  --explain-dir "$OUT/explain-prod-off" --profile-runs 1 \
  --out "$OUT/prod_off" > "$OUT/prod_off.log" 2>&1
echo "PROD_OFF_EXIT=$?"
awk '/group_by_count|distinct_limit|join_two_hop|variable_path_count|resources:/' "$OUT/prod_off.log"

echo "=== memo counters ==="
for q in group_by_count distinct_limit join_two_hop; do
  for dir in prod prod_off; do
    f="$OUT/$dir/lsqb_operators_${q}.json"
    if [ -f "$f" ]; then
      python3 -c "
import json,sys
d = json.load(open('$f'))
print('$dir/$q', {k: d.get(k) for k in ('median_ms','compile_median_ms','optimize_median_us','plan_cache_hits','compiled_plan_memo_hits','dml_plan_memo_hits','dangling_repaired') if k in d})
"
    fi
  done
done

echo "REMEDIATION_GATE_DONE"
