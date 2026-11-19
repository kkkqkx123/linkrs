#!/usr/bin/env bash
# Same-binary A/B worker matrix for the count frontier.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

ROOT=/data/kkkqkx/linkrs
BIN=target/release/linkrs-parity-bench
SOURCE_DB="$ROOT/parity/lsqb-sf0.1-v4/db"
GOLD_DB="$ROOT/parity/lsqb-sf0.1-v4/golden-db"
if [ ! -d "$GOLD_DB" ]; then cp -a "$SOURCE_DB" "$GOLD_DB"; fi
OUT="$ROOT/count-frontier-ab"
rm -rf "$OUT"

run_case() {
  local tag="$1" workers="$2" mode="$3"
  local db="$ROOT/count-frontier-ab-db-$tag"
  rm -rf "$db"
  cp -a "$GOLD_DB" "$db"
  echo "=== $tag workers=$workers frontier=$mode ==="
  taskset -c 16-19 "$BIN" run \
    --db "$db" --space lsqb \
    --benchmark-dir benches/parity/queries/lsqb \
    --warmup 1 --run 8 --threads 1 --parallel-workers "$workers" \
    --min-rows-per-partition 4 \
    --count-frontier "$mode" --ordered-parallel-expand off \
    --profile-runs 3 --no-checkpoint \
    --explain-dir "$OUT/$tag/explain" \
    --skip-file benches/parity/skip.txt \
    --out "$OUT/$tag" > "$ROOT/count-frontier-ab-$tag.log" 2>&1
  echo "CASE_${tag}_EXIT=$?"
  awk '/join_two_hop|variable_path_count/' "$ROOT/count-frontier-ab-$tag.log" | tail -n 2
  if awk '/result_stable=false/{found=1} END{exit !found}' "$ROOT/count-frontier-ab-$tag.log"; then
    echo "RESULT_UNSTABLE_CASE=$tag"
  fi
}

run_case baseline_1 1 off
for workers in 2 4 8; do
  run_case "w${workers}_off" "$workers" off
  run_case "w${workers}_on" "$workers" on
done
echo "MATRIX_DONE"
