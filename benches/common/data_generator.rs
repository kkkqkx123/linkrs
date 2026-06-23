// benches/common/data_generator.rs
//! Data generation utilities for benchmarks

use std::fs;
use std::path::Path;

pub struct DataGenerator;

impl DataGenerator {
    /// Generate benchmark data GQL file
    pub fn generate_storage_bench_data(path: &str, vertex_count: usize, edges_per_vertex: usize) -> std::io::Result<()> {
        let mut content = String::new();

        content.push_str("-- Storage Benchmark Data\n");
        content.push_str("-- This file is auto-generated for benchmark purposes\n\n");

        content.push_str("CREATE SPACE IF NOT EXISTS bench_storage (vid_type=STRING)\n");
        content.push_str("USE bench_storage\n\n");

        // Create vertex type
        content.push_str("CREATE TAG IF NOT EXISTS Vertex(\n");
        content.push_str("    name: STRING,\n");
        content.push_str("    value: DOUBLE,\n");
        content.push_str("    label: STRING,\n");
        content.push_str("    timestamp: INT\n");
        content.push_str(")\n\n");

        // Create edge type
        content.push_str("CREATE EDGE IF NOT EXISTS Edge(\n");
        content.push_str("    weight: DOUBLE DEFAULT 1.0,\n");
        content.push_str("    label: STRING\n");
        content.push_str(")\n\n");

        // Create indexes
        content.push_str("CREATE TAG INDEX IF NOT EXISTS idx_vertex_name ON Vertex(name)\n\n");

        // Generate vertices
        for i in 0..vertex_count {
            let vid = format!("v{}", i);
            let name = format!("vertex_{}", i);
            let value = (i as f64) * 0.1;
            content.push_str(&format!(
                "INSERT VERTEX Vertex(name, value, label, timestamp) VALUES \"{}\":(\"{}\" , {}, \"test\", {})\n",
                vid, name, value, i
            ));
        }

        content.push('\n');

        // Generate edges
        for i in 0..vertex_count {
            for j in 0..edges_per_vertex {
                let target = (i + j + 1) % vertex_count;
                let from_vid = format!("v{}", i);
                let to_vid = format!("v{}", target);
                let weight = 0.5 + (j as f64) * 0.1;
                content.push_str(&format!(
                    "INSERT EDGE Edge(weight, label) VALUES \"{}\"->\"{}\"({}, \"test\")\n",
                    from_vid, to_vid, weight
                ));
            }
        }

        fs::write(path, content)
    }

    /// Generate transaction benchmark data
    pub fn generate_transaction_bench_data(path: &str, vertex_count: usize) -> std::io::Result<()> {
        let mut content = String::new();

        content.push_str("-- Transaction Benchmark Data\n");
        content.push_str("CREATE SPACE IF NOT EXISTS bench_transaction (vid_type=STRING)\n");
        content.push_str("USE bench_transaction\n\n");

        content.push_str("CREATE TAG IF NOT EXISTS Data(\n");
        content.push_str("    value: INT,\n");
        content.push_str("    counter: INT DEFAULT 0\n");
        content.push_str(")\n\n");

        for i in 0..vertex_count {
            content.push_str(&format!(
                "INSERT VERTEX Data(value, counter) VALUES \"d{}\"({}, 0)\n",
                i, i
            ));
        }

        fs::write(path, content)
    }

    /// Generate query benchmark data
    pub fn generate_query_bench_data(path: &str, vertex_count: usize) -> std::io::Result<()> {
        let mut content = String::new();

        content.push_str("-- Query Benchmark Data\n");
        content.push_str("CREATE SPACE IF NOT EXISTS bench_query (vid_type=STRING)\n");
        content.push_str("USE bench_query\n\n");

        content.push_str("CREATE TAG IF NOT EXISTS Node(\n");
        content.push_str("    name: STRING,\n");
        content.push_str("    value: DOUBLE\n");
        content.push_str(")\n\n");

        content.push_str("CREATE EDGE IF NOT EXISTS Link(\n");
        content.push_str("    weight: DOUBLE DEFAULT 1.0\n");
        content.push_str(")\n\n");

        // Generate a small-world network
        for i in 0..vertex_count {
            let name = format!("node_{}", i);
            content.push_str(&format!(
                "INSERT VERTEX Node(name, value) VALUES \"n{}\"(\"{}\", {})\n",
                i, name, i as f64 * 0.1
            ));
        }

        content.push('\n');

        // Create edges to form a small-world network
        for i in 0..vertex_count {
            // Connect to next K neighbors
            for k in 1..=3 {
                let j = (i + k) % vertex_count;
                content.push_str(&format!(
                    "INSERT EDGE Link(weight) VALUES \"n{}\"->\"n{}\"({})\n",
                    i, j, 1.0 / k as f64
                ));
            }
        }

        fs::write(path, content)
    }

    /// Generate fulltext search benchmark data
    pub fn generate_fulltext_bench_data(path: &str, document_count: usize) -> std::io::Result<()> {
        let mut content = String::new();

        content.push_str("-- Fulltext Search Benchmark Data\n");
        content.push_str("CREATE SPACE IF NOT EXISTS bench_fulltext (vid_type=STRING)\n");
        content.push_str("USE bench_fulltext\n\n");

        content.push_str("CREATE TAG IF NOT EXISTS Document(\n");
        content.push_str("    title: STRING,\n");
        content.push_str("    content: STRING,\n");
        content.push_str("    timestamp: INT\n");
        content.push_str(")\n\n");

        let keywords = ["performance", "database", "query", "optimization", "benchmark"];

        for i in 0..document_count {
            let keyword = keywords[i % keywords.len()];
            let title = format!("Document {} - {}", i, keyword);
            let content_text = format!(
                "This is document {} about {}. It contains important information for {} optimization. \
                 Performance is critical for {} systems. The {} should be fast and efficient.",
                i, keyword, keyword, keyword, keyword
            );

            content.push_str(&format!(
                "INSERT VERTEX Document(title, content, timestamp) VALUES \"doc{}\"(\"{}\", \"{}\", {})\n",
                i, title, content_text, i
            ));
        }

        fs::write(path, content)
    }

    /// Generate vector search benchmark data
    pub fn generate_vector_bench_data(path: &str, vector_count: usize, dimensions: usize) -> std::io::Result<()> {
        let mut content = String::new();

        content.push_str("-- Vector Search Benchmark Data\n");
        content.push_str("CREATE SPACE IF NOT EXISTS bench_vector (vid_type=STRING)\n");
        content.push_str("USE bench_vector\n\n");

        content.push_str("CREATE TAG IF NOT EXISTS Vector(\n");
        content.push_str("    embedding: STRING,\n");
        content.push_str("    label: STRING\n");
        content.push_str(")\n\n");

        for i in 0..vector_count {
            let embedding = (0..dimensions)
                .map(|j| format!("{:.6}", ((i * j) as f64 % 1.0)))
                .collect::<Vec<_>>()
                .join(",");

            content.push_str(&format!(
                "INSERT VERTEX Vector(embedding, label) VALUES \"vec{}\"(\"{}\", \"vector_{}\")\n",
                i, embedding, i
            ));
        }

        fs::write(path, content)
    }
}

/// Benchmark data statistics
#[derive(Debug, Clone)]
pub struct BenchmarkDataStats {
    pub vertex_count: usize,
    pub edge_count: usize,
    pub file_size_bytes: usize,
}

impl BenchmarkDataStats {
    pub fn from_file(path: &str) -> std::io::Result<Self> {
        let metadata = fs::metadata(path)?;
        let content = fs::read_to_string(path)?;
        let vertex_count = content.matches("INSERT VERTEX").count();
        let edge_count = content.matches("INSERT EDGE").count();

        Ok(Self {
            vertex_count,
            edge_count,
            file_size_bytes: metadata.len() as usize,
        })
    }
}
