#!/usr/bin/env python3
# benches/data/generate_benchmark_data.py
"""
Benchmark data generator script
Generates GQL files for performance testing with different scales
"""

import argparse
import sys
from pathlib import Path

def generate_storage_bench_data(output_file, vertex_count, edges_per_vertex):
    """Generate storage benchmark data GQL file"""
    with open(output_file, 'w') as f:
        f.write("-- Storage Benchmark Data\n")
        f.write("-- Auto-generated for benchmark purposes\n\n")

        f.write(f"CREATE SPACE IF NOT EXISTS bench_storage_{vertex_count}v (vid_type=STRING)\n")
        f.write(f"USE bench_storage_{vertex_count}v\n\n")

        # Create vertex type
        f.write("CREATE TAG IF NOT EXISTS Vertex(\n")
        f.write("    name: STRING,\n")
        f.write("    value: DOUBLE,\n")
        f.write("    label: STRING,\n")
        f.write("    timestamp: INT\n")
        f.write(")\n\n")

        # Create edge type
        f.write("CREATE EDGE IF NOT EXISTS Edge(\n")
        f.write("    weight: DOUBLE DEFAULT 1.0,\n")
        f.write("    label: STRING\n")
        f.write(")\n\n")

        # Create indexes
        f.write("CREATE TAG INDEX IF NOT EXISTS idx_vertex_name ON Vertex(name)\n\n")

        # Generate vertices
        for i in range(vertex_count):
            vid = f"v{i}"
            name = f"vertex_{i}"
            value = i * 0.1
            f.write(f'INSERT VERTEX Vertex(name, value, label, timestamp) VALUES "{vid}":("{name}", {value}, "test", {i})\n')

        f.write("\n")

        # Generate edges
        for i in range(vertex_count):
            for j in range(edges_per_vertex):
                target = (i + j + 1) % vertex_count
                from_vid = f"v{i}"
                to_vid = f"v{target}"
                weight = 0.5 + j * 0.1
                f.write(f'INSERT EDGE Edge(weight, label) VALUES "{from_vid}"->"{to_vid}"({weight}, "test")\n')

    print(f"Generated storage benchmark data: {output_file}")
    print(f"  - Vertices: {vertex_count}")
    print(f"  - Edges per vertex: {edges_per_vertex}")
    print(f"  - Total edges: {vertex_count * edges_per_vertex}")

def generate_transaction_bench_data(output_file, vertex_count):
    """Generate transaction benchmark data GQL file"""
    with open(output_file, 'w') as f:
        f.write("-- Transaction Benchmark Data\n")
        f.write(f"CREATE SPACE IF NOT EXISTS bench_transaction_{vertex_count}v (vid_type=STRING)\n")
        f.write(f"USE bench_transaction_{vertex_count}v\n\n")

        f.write("CREATE TAG IF NOT EXISTS Data(\n")
        f.write("    value: INT,\n")
        f.write("    counter: INT DEFAULT 0\n")
        f.write(")\n\n")

        for i in range(vertex_count):
            f.write(f'INSERT VERTEX Data(value, counter) VALUES "d{i}"({i}, 0)\n')

    print(f"Generated transaction benchmark data: {output_file}")
    print(f"  - Vertices: {vertex_count}")

def generate_query_bench_data(output_file, vertex_count):
    """Generate query benchmark data GQL file"""
    with open(output_file, 'w') as f:
        f.write("-- Query Benchmark Data\n")
        f.write(f"CREATE SPACE IF NOT EXISTS bench_query_{vertex_count}v (vid_type=STRING)\n")
        f.write(f"USE bench_query_{vertex_count}v\n\n")

        f.write("CREATE TAG IF NOT EXISTS Node(\n")
        f.write("    name: STRING,\n")
        f.write("    value: DOUBLE\n")
        f.write(")\n\n")

        f.write("CREATE EDGE IF NOT EXISTS Link(\n")
        f.write("    weight: DOUBLE DEFAULT 1.0\n")
        f.write(")\n\n")

        # Generate vertices
        for i in range(vertex_count):
            name = f"node_{i}"
            value = i * 0.1
            f.write(f'INSERT VERTEX Node(name, value) VALUES "n{i}":("{name}", {value})\n')

        f.write("\n")

        # Create small-world network edges
        for i in range(vertex_count):
            for k in range(1, min(4, vertex_count)):
                j = (i + k) % vertex_count
                weight = 1.0 / k
                f.write(f'INSERT EDGE Link(weight) VALUES "n{i}"->"n{j}"({weight})\n')

    print(f"Generated query benchmark data: {output_file}")
    print(f"  - Vertices: {vertex_count}")
    print(f"  - Network pattern: Small-world (each node connects to next 3)")

def generate_fulltext_bench_data(output_file, document_count):
    """Generate fulltext search benchmark data GQL file"""
    with open(output_file, 'w') as f:
        f.write("-- Fulltext Search Benchmark Data\n")
        f.write(f"CREATE SPACE IF NOT EXISTS bench_fulltext_{document_count}d (vid_type=STRING)\n")
        f.write(f"USE bench_fulltext_{document_count}d\n\n")

        f.write("CREATE TAG IF NOT EXISTS Document(\n")
        f.write("    title: STRING,\n")
        f.write("    content: STRING,\n")
        f.write("    timestamp: INT\n")
        f.write(")\n\n")

        keywords = ["performance", "database", "query", "optimization", "benchmark"]

        for i in range(document_count):
            keyword = keywords[i % len(keywords)]
            title = f"Document {i} - {keyword}"
            content = (
                f"This is document {i} about {keyword}. It contains important information for {keyword} optimization. "
                f"Performance is critical for {keyword} systems. The {keyword} should be fast and efficient."
            )
            # Escape quotes
            content = content.replace('"', '\\"')
            title = title.replace('"', '\\"')
            f.write(f'INSERT VERTEX Document(title, content, timestamp) VALUES "doc{i}":("{title}", "{content}", {i})\n')

    print(f"Generated fulltext benchmark data: {output_file}")
    print(f"  - Documents: {document_count}")
    print(f"  - Keywords: {len(keywords)}")

def generate_vector_bench_data(output_file, vector_count, dimensions):
    """Generate vector search benchmark data GQL file"""
    with open(output_file, 'w') as f:
        f.write("-- Vector Search Benchmark Data\n")
        f.write(f"CREATE SPACE IF NOT EXISTS bench_vector_{vector_count}v_{dimensions}d (vid_type=STRING)\n")
        f.write(f"USE bench_vector_{vector_count}v_{dimensions}d\n\n")

        f.write("CREATE TAG IF NOT EXISTS Vector(\n")
        f.write("    embedding: STRING,\n")
        f.write("    label: STRING\n")
        f.write(")\n\n")

        for i in range(vector_count):
            embedding = ",".join([f"{((i * j) % 100) / 100:.6f}" for j in range(dimensions)])
            f.write(f'INSERT VERTEX Vector(embedding, label) VALUES "vec{i}":("{embedding}", "vector_{i}")\n')

    print(f"Generated vector benchmark data: {output_file}")
    print(f"  - Vectors: {vector_count}")
    print(f"  - Dimensions: {dimensions}")

def main():
    parser = argparse.ArgumentParser(description='Generate benchmark data for GraphDB')
    parser.add_argument('--type', choices=['storage', 'transaction', 'query', 'fulltext', 'vector', 'all'],
                       required=True, help='Type of benchmark data to generate')
    parser.add_argument('--output-dir', default='benches/data', help='Output directory for GQL files')
    parser.add_argument('--vertices', type=int, default=1000, help='Number of vertices (storage, transaction, query)')
    parser.add_argument('--edges-per-vertex', type=int, default=5, help='Edges per vertex (storage)')
    parser.add_argument('--documents', type=int, default=1000, help='Number of documents (fulltext)')
    parser.add_argument('--vectors', type=int, default=1000, help='Number of vectors (vector)')
    parser.add_argument('--dimensions', type=int, default=128, help='Vector dimensions (vector)')

    args = parser.parse_args()

    # Create output directory
    Path(args.output_dir).mkdir(parents=True, exist_ok=True)

    if args.type in ['storage', 'all']:
        output = Path(args.output_dir) / f'bench_storage_{args.vertices}v_{args.edges_per_vertex}e.gql'
        generate_storage_bench_data(str(output), args.vertices, args.edges_per_vertex)

    if args.type in ['transaction', 'all']:
        output = Path(args.output_dir) / f'bench_transaction_{args.vertices}v.gql'
        generate_transaction_bench_data(str(output), args.vertices)

    if args.type in ['query', 'all']:
        output = Path(args.output_dir) / f'bench_query_{args.vertices}v.gql'
        generate_query_bench_data(str(output), args.vertices)

    if args.type in ['fulltext', 'all']:
        output = Path(args.output_dir) / f'bench_fulltext_{args.documents}d.gql'
        generate_fulltext_bench_data(str(output), args.documents)

    if args.type in ['vector', 'all']:
        output = Path(args.output_dir) / f'bench_vector_{args.vectors}v_{args.dimensions}d.gql'
        generate_vector_bench_data(str(output), args.vectors, args.dimensions)

    print(f"\nAll benchmark data files generated in: {args.output_dir}")

if __name__ == '__main__':
    main()
