#!/usr/bin/env bash
# Build, verify, and measure the stage 1/2 convergence work on devs.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

ROOT=/data/kkkqkx/linkrs
OUT_NAME="${GATE_OUT:-scan-visibility-baseline}"
OUT="$ROOT/$OUT_NAME"
mkdir -p "$OUT"

echo "=== build parity bench ==="
cargo build --release --features embedded --bin linkrs-parity-bench > "$ROOT/$OUT_NAME-build.log" 2>&1
BUILD_EXIT=$?
echo "BUILD_EXIT=$BUILD_EXIT"
tail -n 15 "$ROOT/$OUT_NAME-build.log"
if [ "$BUILD_EXIT" -ne 0 ] || [ ! -x target/release/linkrs-parity-bench ]; then
  echo "BUILD_FAILED"
  echo "STAGE12_GATE_DONE"
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

# The config crate's dev dependency `serial_test` is not available offline on
# devs; the default-value assertion is covered by the partitioning lib test.
run_test config_defaults cargo test -p linkrs-query --lib -- default_config_releases_only_ordered_expand_shape
run_test partitioning cargo test -p linkrs-query --lib -- partitioning
run_test typed_group cargo test -p linkrs-query --lib -- typed_group_count
run_test local_topk cargo test -p linkrs-query --lib -- local_topk
run_test parallel cargo test -p linkrs-query --test parallel_partition_execution
run_test dql cargo test -p linkrs-query --test dql
run_test storage cargo test -p linkrs-storage --lib
run_test column_batch cargo test -p linkrs-storage --test column_batch_test

DB="$ROOT/parity/lsqb-sf0.1-v4/db"
BIN=target/release/linkrs-parity-bench
SKIP="$OUT/skip-gate.txt"
printf '%s\n' \
  'lsqb/operators/shortest_path Linkrs-only dialect is rejected by Ladybug and Neug' \
  > "$SKIP"

echo "=== fixed gate (explicit benchmark parallelism) ==="
for round in 1 2; do
  taskset -c 16-31 "$BIN" run \
    --db "$DB" --space lsqb \
    --benchmark-dir benches/parity/queries/lsqb \
    --warmup 1 --run 5 --threads 16 --parallel-workers 16 --max-partitions 10 \
    --ordered-parallel-expand on \
    --require-parallel-explain lsqb/operators/group_by_count,lsqb/operators/distinct_limit \
    --no-snapshots --skip-file "$SKIP" \
    --explain-dir "$OUT/explain$round" --profile-runs 1 \
    --out "$OUT/round$round" > "$OUT/round$round.log" 2>&1
  echo "ROUND${round}_EXIT=$?"
  awk '/group_by_count|distinct_limit|join_two_hop|variable_path_count|resources:/' "$OUT/round$round.log"
done

echo "=== production default gate (no benchmark parallel flags) ==="
taskset -c 16-31 "$BIN" run \
  --db "$DB" --space lsqb \
  --benchmark-dir benches/parity/queries/lsqb \
  --warmup 1 --run 5 --threads 16 \
  --no-snapshots --skip-file "$SKIP" \
  --explain-dir "$OUT/explain-prod" --profile-runs 1 \
  --out "$OUT/prod" > "$OUT/prod.log" 2>&1
echo "PROD_EXIT=$?"
awk '/group_by_count|distinct_limit|q1:|q2:|resources:/' "$OUT/prod.log"

echo "=== result table ==="
python3 - "$OUT" <<'PY'
import glob
import json
import os
import sys

out = sys.argv[1]
keys = (
    "name", "median_ms", "compile_median_ms", "typed_group_hits",
    "partition_skew_ratio", "parallel_wall_us", "parallel_work_us",
    "local_topk_pushed", "scan_read_us", "scan_enumerate_us",
    "scan_visibility_us", "scan_decode_us", "scan_assemble_us",
    "scan_predicate_us", "expand_read_us",
    "expand_topology_read_us", "expand_destination_read_us",
    "expand_tag_resolve_us", "expand_view_us", "expand_output_us",
    "expand_parent_batches", "expand_parents", "expand_events",
    "expand_unique_destinations", "expand_missing_destinations",
    "expand_output_rows", "spill_rows", "result", "status",
)
print("\t".join(keys))
for group in ("round1", "round2", "prod"):
    for path in sorted(glob.glob(os.path.join(out, group, "*.json"))):
        with open(path, encoding="utf-8") as handle:
            row = json.load(handle)
        print(group + "\t" + "\t".join(str(row.get(key, "")) for key in keys))
PY

echo "=== parallel plan check ==="
python3 - "$OUT" <<'PY'
import os
import sys

out = sys.argv[1]
for group in ("explain1", "explain2", "explain-prod"):
    for name in ("lsqb_operators_group_by_count", "lsqb_operators_distinct_limit"):
        path = os.path.join(out, group, f"{name}_r1.txt")
        if not os.path.exists(path):
            print(f"{group} {name}: missing")
            continue
        with open(path, encoding="utf-8") as handle:
            lines = [line for line in handle if "Parallel Workers:" in line or "Columnar:" in line]
        print(f"{group} {name}: " + " | ".join(line.strip() for line in lines))
PY

echo "=== microbenchmark attribution ==="
taskset -c 16 cargo bench -p linkrs-storage --bench scan_division_bench > "$OUT/microbench-scan.log" 2>&1
echo "SCAN_BENCH_EXIT=$?"
taskset -c 16 cargo bench -p linkrs-storage --bench topology_read_bench > "$OUT/microbench-topology.log" 2>&1
echo "TOPO_BENCH_EXIT=$?"
taskset -c 16 cargo bench -p linkrs-storage --bench visibility_decode_bench > "$OUT/microbench-visibility-decode.log" 2>&1
echo "VISIBILITY_BENCH_EXIT=$?"
tail -n 8 "$OUT/microbench-scan.log"
tail -n 10 "$OUT/microbench-topology.log"
tail -n 8 "$OUT/microbench-visibility-decode.log"

echo "STAGE12_GATE_DONE"
