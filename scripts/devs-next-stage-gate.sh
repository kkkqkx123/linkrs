#!/usr/bin/env bash
# Build and measure the next-stage parity gate on devs.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

ROOT=/data/kkkqkx/linkrs
OUT="$ROOT/next-stage-results"
rm -rf "$OUT"
mkdir -p "$OUT"

echo "=== build parity bench ==="
cargo build --release --features embedded --bin linkrs-parity-bench > "$ROOT/next-stage-build.log" 2>&1
BUILD_EXIT=$?
echo "BUILD_EXIT=$BUILD_EXIT"
tail -n 10 "$ROOT/next-stage-build.log"
if [ "$BUILD_EXIT" -ne 0 ] || [ ! -x target/release/linkrs-parity-bench ]; then
  echo "BUILD_FAILED"
  echo "NEXT_STAGE_GATE_DONE"
  exit 1
fi

echo "=== correctness gates ==="
cargo test -p linkrs-query --lib -- typed_group_count > "$OUT/unit-typed-group-count.log" 2>&1
echo "TYPED_GROUP_COUNT_EXIT=$?"
tail -n 5 "$OUT/unit-typed-group-count.log"
cargo test -p linkrs-query --lib -- local_topk > "$OUT/unit-local-topk.log" 2>&1
echo "LOCAL_TOPK_EXIT=$?"
tail -n 5 "$OUT/unit-local-topk.log"
cargo test -p linkrs-query --test parallel_partition_execution > "$OUT/parallel.log" 2>&1
echo "PARALLEL_EXIT=$?"
tail -n 5 "$OUT/parallel.log"
cargo test -p linkrs-query --test predicate_comparator_proof > "$OUT/predicate-proof.log" 2>&1
echo "PREDICATE_PROOF_EXIT=$?"
tail -n 5 "$OUT/predicate-proof.log"
cargo test -p linkrs-query --test order_limit_pushdown_proof > "$OUT/order-limit-proof.log" 2>&1
echo "ORDER_LIMIT_PROOF_EXIT=$?"
tail -n 5 "$OUT/order-limit-proof.log"
cargo test -p linkrs-query --test dql > "$OUT/dql.log" 2>&1
echo "DQL_EXIT=$?"
tail -n 5 "$OUT/dql.log"
cargo test -p linkrs-storage --lib -- checkpoint_retention > "$OUT/checkpoint-retention.log" 2>&1
echo "CHECKPOINT_RETENTION_EXIT=$?"
tail -n 5 "$OUT/checkpoint-retention.log"
cargo test -p linkrs-transaction --lib -- wal::writer::local > "$OUT/wal-writer.log" 2>&1
echo "WAL_WRITER_EXIT=$?"
tail -n 5 "$OUT/wal-writer.log"
cargo test -p linkrs-api --lib -- embedded::config > "$OUT/api-config.log" 2>&1
echo "API_CONFIG_EXIT=$?"
tail -n 5 "$OUT/api-config.log"
cargo test -p linkrs-storage --lib > "$OUT/storage-all.log" 2>&1
echo "STORAGE_EXIT=$?"
tail -n 5 "$OUT/storage-all.log"

DB="$ROOT/parity/lsqb-sf0.1-v4/db"
echo "=== database size before ==="
du -sb "$DB"
find "$DB/wal" -maxdepth 1 -type f | wc -l
find "$DB/checkpoint" -mindepth 1 -maxdepth 1 -type d -name 'checkpoint_*' | wc -l

BIN=target/release/linkrs-parity-bench
SKIP="$OUT/skip-gate.txt"
printf '%s\n' \
  'lsqb/operators/shortest_path Linkrs-only dialect is rejected by Ladybug and Neug' \
  > "$SKIP"

for round in 1 2; do
  echo "=== fixed gate round $round ==="
  taskset -c 16-31 "$BIN" run \
    --db "$DB" --space lsqb \
    --benchmark-dir benches/parity/queries/lsqb \
    --warmup 1 --run 5 --threads 16 --parallel-workers 16 --max-partitions 10 \
    --ordered-parallel-expand on \
    --require-parallel-explain lsqb/operators/group_by_count,lsqb/operators/distinct_limit \
    --no-snapshots \
    --skip-file "$SKIP" \
    --explain-dir "$OUT/explain$round" --profile-runs 1 \
    --out "$OUT/round$round" > "$OUT/round$round.log" 2>&1
  echo "ROUND${round}_EXIT=$?"
  awk '/group_by_count|distinct_limit|resources:|VmHWM|not a parallel plan/' "$OUT/round$round.log"
done

echo "=== database size after ==="
du -sb "$DB"
find "$DB/wal" -maxdepth 1 -type f | wc -l
find "$DB/checkpoint" -mindepth 1 -maxdepth 1 -type d -name 'checkpoint_*' | wc -l

echo "=== resource table ==="
python3 - "$OUT" <<'PY'
import glob
import json
import os
import sys

out = sys.argv[1]
print("name\tmedian_ms\tdb_size_bytes\trss_peak_kb\tbuffer_pool_bytes\tspill_rows\tstatus")
for path in sorted(glob.glob(os.path.join(out, "round*", "*.json"))):
    with open(path, encoding="utf-8") as handle:
        row = json.load(handle)
    print("\t".join(str(row.get(key, "")) for key in (
        "name", "median_ms", "db_size_bytes", "rss_peak_kb",
        "buffer_pool_bytes", "spill_rows", "status",
    )))
PY

echo "=== parallel plan check ==="
python3 - "$OUT" <<'PY'
import os
import re
import sys

out = sys.argv[1]
for round in (1, 2):
    for name in ("lsqb_operators_group_by_count", "lsqb_operators_distinct_limit"):
        path = os.path.join(out, f"explain{round}", f"{name}_r1.txt")
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        fields = re.findall(r"actual(?:_partitions)?=\d+", text)
        print(f"round{round} {name}: {' '.join(fields)}")
PY

echo "NEXT_STAGE_GATE_DONE"
