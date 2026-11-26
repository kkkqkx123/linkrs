#!/usr/bin/env bash
# Rebuild the parity dataset and collect ordered-existence metrics on devs.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

BIND="numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31"
export BENCH_CPU_BINDING="$BIND"
export BENCH_NUMA_POLICY="cpunodebind=1,membind=1"

ROOT=/data/kkkqkx/linkrs
OUT="$ROOT/ordered-existence-remediation"
DATA=/data/kkkqkx/graphdb-ben/data/social-network-sf0.1-projected-fk
DB="$ROOT/parity/lsqb-sf0.1-v5/db"
BIN=target/release/linkrs-parity-bench
SHELL_BIN=target/release/linkrs
mkdir -p "$OUT"

echo "=== rebuild binaries ==="
{
  cargo build --release --features embedded -p linkrs --bin linkrs-parity-bench
  cargo build --release -p linkrs_shell --bin linkrs
} > "$OUT/build-measure.log" 2>&1
echo "BUILD_EXIT=$?"
tail -n 8 "$OUT/build-measure.log"
if [ ! -x "$BIN" ] || [ ! -x "$SHELL_BIN" ]; then
  echo "BUILD_FAILED"
  echo "ORDERED_EXISTENCE_MEASURE_DONE"
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

echo "=== fresh parity database with rebuilt primary-key index ==="
rm -rf "$DB"
mkdir -p "$(dirname "$DB")"
"$BIN" copy --dataset lsqb --csv-dir "$DATA" \
  --script benches/parity/schema/lsqb.cypher --db "$DB" --force \
  > "$OUT/copy.log" 2>&1
echo "COPY_EXIT=$?"
awk '/rebuild index|total|serialized/' "$OUT/copy.log" | tail -n 8

mkdir -p "$OUT/diff/lsqb/operators"
cp benches/parity/queries/lsqb/operators/distinct_limit.benchmark \
  "$OUT/diff/lsqb/operators/distinct_limit.benchmark"

echo "=== EXPLAIN proof ==="
"$SHELL_BIN" "$DB" -d lsqb -c \
  "EXPLAIN MATCH (:n)-[h:n]->(p:Person) RETURN DISTINCT p.id ORDER BY p.id LIMIT 10" \
  > "$OUT/explain.txt" 2>&1
echo "EXPLAIN_EXIT=$?"

echo "=== stage timing and profiling ==="
LINKRS_STAGE_TIMING=1 $BIND "$BIN" run \
  --db "$DB" --space lsqb --benchmark-dir "$OUT/diff" \
  --warmup 1 --run 1 --threads 16 \
  --no-snapshots --explain-dir "$OUT/stage-explain" --profile-runs 1 \
  --out "$OUT/stage" > "$OUT/stage.log" 2>&1
echo "STAGE_EXIT=$?"
awk '/\[stage\]|ordered_existence_open_us|distinct_limit:/' "$OUT/stage.log" | head -n 40

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
import pathlib
import re
import sys

out = pathlib.Path(sys.argv[1])

keys = (
    "name", "median_ms", "compile_median_ms", "status", "plan_cache_hits",
    "compiled_plan_memo_hits", "dml_plan_memo_hits",
    "parse_median_us", "plan_median_us", "optimize_median_us",
    "statistics_median_us", "cache_probe_median_us",
    "ordered_existence_candidates", "ordered_existence_matches",
    "ordered_existence_output_rows", "ordered_existence_drift", "spill_rows",
)
print("\t".join(keys))
for path in sorted(glob.glob(str(out / "prod" / "**" / "*.json"), recursive=True)):
    with open(path, encoding="utf-8") as handle:
        row = json.load(handle)
    if row.get("name") in {
        "lsqb/operators/distinct_limit",
        "lsqb/operators/group_by_count",
        "lsqb/operators/join_two_hop",
        "lsqb/operators/variable_path_count",
    }:
        print("\t".join(str(row.get(key, "")) for key in keys))

explain = out / "stage-explain" / "lsqb_operators_distinct_limit_r1.txt"
if explain.exists():
    text = explain.read_text(encoding="utf-8")
    open_us = int(re.search(r"ordered_existence_open_us=(\d+)", text).group(1))
    redundant_removed = "redundant_global_ops=eliminated" in text
    globals_left = [name for name in ("Distinct", "TopN") if re.search(rf"\| {name}\s+\|", text)]
    print("EXPLAIN_OPEN_US=", open_us, "OPEN_REASONABLE=", open_us < 100_000)
    print("EXPLAIN_REDUNDANT_REMOVED=", redundant_removed)
    print("EXPLAIN_GLOBALS_LEFT=", globals_left)
PY

echo "ORDERED_EXISTENCE_MEASURE_DONE"
