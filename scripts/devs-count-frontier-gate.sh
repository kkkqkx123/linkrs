#!/usr/bin/env bash
# Remote gate for the parallel count frontier with isolated timing/profile passes.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

ROOT=/data/kkkqkx/linkrs
echo "=== build parity bench ==="
cargo build --release --features embedded --bin linkrs-parity-bench > "$ROOT/count-frontier-build.log" 2>&1
BUILD_EXIT=$?
echo "BUILD_EXIT=$BUILD_EXIT"
tail -n 8 "$ROOT/count-frontier-build.log"
if [ "$BUILD_EXIT" -ne 0 ]; then exit 1; fi

echo "=== correctness gates ==="
cargo test -p linkrs-query --lib -- partitioning > "$ROOT/count-frontier-partitioning.log" 2>&1
PARTITIONING_EXIT=$?
echo "PARTITIONING_EXIT=$PARTITIONING_EXIT"
tail -n 5 "$ROOT/count-frontier-partitioning.log"
if [ "$PARTITIONING_EXIT" -ne 0 ]; then exit 1; fi
cargo test -p linkrs-query --lib -- streaming > "$ROOT/count-frontier-streaming.log" 2>&1
STREAMING_EXIT=$?
echo "STREAMING_EXIT=$STREAMING_EXIT"
tail -n 5 "$ROOT/count-frontier-streaming.log"
if [ "$STREAMING_EXIT" -ne 0 ]; then exit 1; fi
cargo test -p linkrs-query --test dql > "$ROOT/count-frontier-dql.log" 2>&1
DQL_EXIT=$?
echo "DQL_EXIT=$DQL_EXIT"
tail -n 5 "$ROOT/count-frontier-dql.log"
if [ "$DQL_EXIT" -ne 0 ]; then exit 1; fi
cargo test -p linkrs-query --test parallel_partition_execution > "$ROOT/count-frontier-parallel.log" 2>&1
PARALLEL_EXIT=$?
echo "PARALLEL_EXIT=$PARALLEL_EXIT"
tail -n 5 "$ROOT/count-frontier-parallel.log"
if [ "$PARALLEL_EXIT" -ne 0 ]; then exit 1; fi

BIN=target/release/linkrs-parity-bench
SOURCE_DB="$ROOT/parity/lsqb-sf0.1-v4/db"
GOLD_DB="$ROOT/parity/lsqb-sf0.1-v4/golden-db"
if [ ! -d "$GOLD_DB" ]; then cp -a "$SOURCE_DB" "$GOLD_DB"; fi
OUT="$ROOT/count-frontier-results"
rm -rf "$OUT"
for round in 1 2 3 4; do
  DB="$ROOT/count-frontier-db-r$round"
  rm -rf "$DB"
  cp -a "$GOLD_DB" "$DB"
  echo "=== fixed gate round $round ==="
  taskset -c 16-19 "$BIN" run \
    --db "$DB" --space lsqb \
    --benchmark-dir benches/parity/queries/lsqb \
    --warmup 1 --run 5 --threads 1 --parallel-workers 4 \
    --min-rows-per-partition 4 \
    --count-frontier on --ordered-parallel-expand off \
    --profile-runs 3 --no-checkpoint \
    --explain-dir "$OUT/round$round/explain" \
    --skip-file benches/parity/skip.txt \
    --out "$OUT/round$round" > "$ROOT/count-frontier-round$round.log" 2>&1
  echo "ROUND${round}_EXIT=$?"
  awk '/join_two_hop|variable_path_count/' "$ROOT/count-frontier-round$round.log" | tail -n 2
  if awk '/result_stable=false/{found=1} END{exit !found}' "$ROOT/count-frontier-round$round.log"; then
    echo "RESULT_UNSTABLE_ROUND=$round"
    exit 1
  fi
done

echo "=== parallel metrics from EXPLAIN ANALYZE ==="
awk '/Parallel Workers:/' "$OUT"/round1/explain/*_r1.txt | tee "$ROOT/count-frontier-metrics.log" | head -n 20
echo "COUNT_FRONTIER_DONE"
