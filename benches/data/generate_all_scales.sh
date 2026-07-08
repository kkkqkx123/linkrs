#!/bin/bash
# benches/data/generate_all_scales.sh
# Generate benchmark data at multiple scales for scalability testing

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_DIR="$SCRIPT_DIR"

echo "Generating benchmark data at multiple scales..."
echo "Output directory: $OUTPUT_DIR"
echo ""

# Storage benchmark data
echo "=== Generating Storage Benchmark Data ==="
for vertices in 100 1000 10000; do
    echo "Generating storage data: ${vertices} vertices, 5 edges per vertex..."
    python3 "$SCRIPT_DIR/generate_benchmark_data.py" \
        --type storage \
        --vertices $vertices \
        --edges-per-vertex 5 \
        --output-dir "$OUTPUT_DIR"
done

# Query benchmark data
echo ""
echo "=== Generating Query Benchmark Data ==="
for vertices in 100 1000 10000; do
    echo "Generating query data: ${vertices} vertices..."
    python3 "$SCRIPT_DIR/generate_benchmark_data.py" \
        --type query \
        --vertices $vertices \
        --output-dir "$OUTPUT_DIR"
done

# Transaction benchmark data
echo ""
echo "=== Generating Transaction Benchmark Data ==="
for vertices in 100 1000 5000; do
    echo "Generating transaction data: ${vertices} vertices..."
    python3 "$SCRIPT_DIR/generate_benchmark_data.py" \
        --type transaction \
        --vertices $vertices \
        --output-dir "$OUTPUT_DIR"
done

# Fulltext benchmark data
echo ""
echo "=== Generating Fulltext Benchmark Data ==="
for docs in 100 1000 10000; do
    echo "Generating fulltext data: ${docs} documents..."
    python3 "$SCRIPT_DIR/generate_benchmark_data.py" \
        --type fulltext \
        --documents $docs \
        --output-dir "$OUTPUT_DIR"
done

# Vector benchmark data
echo ""
echo "=== Generating Vector Benchmark Data ==="
for dimensions in 128 256 512; do
    for vectors in 1000 10000; do
        echo "Generating vector data: ${vectors} vectors, ${dimensions}d..."
        python3 "$SCRIPT_DIR/generate_benchmark_data.py" \
            --type vector \
            --vectors $vectors \
            --dimensions $dimensions \
            --output-dir "$OUTPUT_DIR"
    done
done

echo ""
echo "=== Benchmark Data Generation Complete ==="
echo ""
echo "Generated files:"
ls -lh "$OUTPUT_DIR"/bench_*.gql | awk '{print "  " $9 " (" $5 ")"}'

echo ""
echo "Total size: $(du -sh "$OUTPUT_DIR" | cut -f1)"
echo ""
echo "To run benchmarks:"
echo "  cargo bench                    # Run all benchmarks"
echo "  cargo bench --release          # Run in release mode (recommended)"
echo "  cargo bench --bench storage_bench"
echo ""
