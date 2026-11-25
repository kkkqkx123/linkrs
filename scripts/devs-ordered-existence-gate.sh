#!/usr/bin/env bash
# Build, verify and measure the ordered existence scan on devs.
# Run from /data/kkkqkx/linkrs/repo with this script copied there.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

BIND="numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31"
export BENCH_CPU_BINDING="$BIND"
export BENCH_NUMA_POLICY="cpunodebind=1,membind=1"

ROOT=/data/kkkqkx/linkrs
OUT="$ROOT/ordered-existence"
DATA=/data/kkkqkx/graphdb-ben/data/social-network-sf0.1-projected-fk
DB="$ROOT/parity/lsqb-sf0.1-v5/db"
BIN=target/release/linkrs-parity-bench
SHELL_BIN=target/release/linkrs
mkdir -p "$OUT"

echo "=== build ==="
{
  cargo build --release --features embedded -p linkrs --bin linkrs-parity-bench
  cargo build --release -p linkrs_shell --bin linkrs
} > "$OUT/build.log" 2>&1
echo "BUILD_EXIT=$?"
tail -n 12 "$OUT/build.log"
if [ ! -x "$BIN" ] || [ ! -x "$SHELL_BIN" ]; then
  echo "BUILD_FAILED"
  echo "ORDERED_EXISTENCE_GATE_DONE"
  exit 1
fi

run_test() {
  local label="$1"
  shift
  "$@" > "$OUT/$label.log" 2>&1
  local code=$?
  echo "${label^^}_EXIT=$code"
  tail -n 3 "$OUT/$label.log"
}

echo "=== correctness gates ==="
run_test storage_ordered_existence cargo test -p linkrs-storage --lib ordered_existence
run_test query_ordered_existence cargo test -p linkrs-query --lib ordered_existence
run_test partitioning cargo test -p linkrs-query --lib -- partitioning
run_test local_topk cargo test -p linkrs-query --lib -- local_topk
run_test parallel cargo test -p linkrs-query --test parallel_partition_execution
run_test dql cargo test -p linkrs-query --test dql
run_test storage_lib cargo test -p linkrs-storage --lib

echo "=== fresh parity database with the primary-key index ==="
rm -rf "$DB"
mkdir -p "$(dirname "$DB")"
"$BIN" copy --dataset lsqb --csv-dir "$DATA" \
  --script benches/parity/schema/lsqb.cypher --db "$DB" --force \
  > "$OUT/copy.log" 2>&1
echo "COPY_EXIT=$?"
tail -n 8 "$OUT/copy.log"

echo "=== EXPLAIN proof ==="
"$SHELL_BIN" "$DB" -d lsqb -c \
  "EXPLAIN MATCH (:n)-[h:n]->(p:Person) RETURN DISTINCT p.id ORDER BY p.id LIMIT 10" \
  > "$OUT/explain.txt" 2>&1
echo "EXPLAIN_EXIT=$?"
grep -E "OrderedExistenceScan|ordered_existence|LocalTopK|Distinct|TopN" "$OUT/explain.txt" | head -n 20 || true

echo "=== rewrite path is now the only path (A/B switch removed) ==="
echo "Correctness for the former slow path is covered by the differential unit"
echo "tests and EXPLAIN proof above; no runtime A/B toggle remains."

echo "=== production default metrics ==="
$BIND "$BIN" run \
  --db "$DB" --space lsqb \
  --benchmark-dir benches/parity/queries/lsqb \
  --warmup 1 --run 5 --threads 16 \
  --no-snapshots --explain-dir "$OUT/explain-prod" --profile-runs 1 \
  --out "$OUT/prod" > "$OUT/prod.log" 2>&1
echo "PROD_EXIT=$?"
awk '/distinct_limit|group_by_count|join_two_hop|variable_path_count|resources:/' "$OUT/prod.log"
python3 - "$OUT" <<'PY'
import glob
import json
import os
import sys

out = sys.argv[1]
keys = (
    "name", "median_ms", "compile_median_ms", "status",
    "ordered_existence_candidates", "ordered_existence_matches",
    "ordered_existence_output_rows", "ordered_existence_drift",
    "spill_rows", "expand_output_rows", "scan_read_us",
)
print("\t".join(keys))
for path in sorted(glob.glob(os.path.join(out, "prod", "**", "*.json"), recursive=True)):
    with open(path, encoding="utf-8") as handle:
        row = json.load(handle)
    if row.get("name") in {
        "lsqb/operators/distinct_limit",
        "lsqb/operators/group_by_count",
        "lsqb/operators/join_two_hop",
        "lsqb/operators/variable_path_count",
    }:
        print("\t".join(str(row.get(key, "")) for key in keys))
PY

echo "ORDERED_EXISTENCE_GATE_DONE"
