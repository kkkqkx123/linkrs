//! E2E Test Suite for Extended Types
//!
//! Tests extended type functionality including:
//! - Geography/Geospatial types
//! - Vector search
//! - Full-text search

use crate::common::{assert_query_ok, create_test_db, setup_test_space, TestDb};

/// Geography/Geospatial type tests
mod geography {
    use super::*;

    /// Create points using ST_Point
    #[test]
    fn test_point_creation() {
        let mut db = create_test_db();
        setup_test_space(
        &mut db,
            "e2e_geography",
            &["CREATE TAG location(name: STRING NOT NULL, coord: GEOGRAPHY, address: STRING, category: STRING)"],
            &[],
        ).expect("Failed to setup test space");

        // Insert point
        let result = db.execute_query(
            "INSERT VERTEX location(name, coord, category) VALUES 'loc_test': ('Test Location', ST_Point(116.4, 39.9), 'test')"
        );
        assert_query_ok(result, "INSERT with ST_Point should succeed");
    }

    /// Create points using WKT format
    #[test]
    fn test_wkt_creation() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_geography_wkt",
            &["CREATE TAG location(name: STRING NOT NULL, coord: GEOGRAPHY, category: STRING)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Insert point from WKT
        let result = db.execute_query(
            "INSERT VERTEX location(name, coord, category) VALUES 'loc_wkt': ('WKT Location', ST_GeogFromText('POINT(116.5 39.8)'), 'test')"
        );
        assert_query_ok(result, "INSERT with WKT should succeed");
    }

    /// Calculate distance between points
    #[test]
    fn test_distance_calculation() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_geography_dist",
            &["CREATE TAG location(name: STRING NOT NULL, coord: GEOGRAPHY)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Insert points
        db.execute_query(
            "INSERT VERTEX location(name, coord) VALUES 'loc1': ('Tiananmen', ST_Point(116.3974, 39.9093))"
        ).expect("INSERT should succeed");
        db.execute_query(
            "INSERT VERTEX location(name, coord) VALUES 'loc2': ('Forbidden City', ST_Point(116.3972, 39.9163))"
        ).expect("INSERT should succeed");

        // Calculate distance
        let result = db.execute_query(
            "MATCH (a:location {name: 'Tiananmen'}), (b:location {name: 'Forbidden City'}) RETURN ST_Distance(a.coord, b.coord) AS distance_km"
        );
        assert_query_ok(result, "ST_Distance should succeed");
    }

    /// Find locations within distance
    #[test]
    fn test_within_distance() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_geography_within",
            &["CREATE TAG location(name: STRING NOT NULL, coord: GEOGRAPHY)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Insert points
        db.execute_query(
            "INSERT VERTEX location(name, coord) VALUES 'center': ('Tiananmen', ST_Point(116.4, 39.9))"
        ).expect("INSERT should succeed");
        db.execute_query(
            "INSERT VERTEX location(name, coord) VALUES 'loc1': ('Forbidden City', ST_Point(116.3972, 39.9163))"
        ).expect("INSERT should succeed");

        // Find within distance
        let result = db.execute_query(
            "MATCH (center:location {name: 'Tiananmen'}) MATCH (loc:location) WHERE ST_DWithin(center.coord, loc.coord, 5.0) RETURN loc.name, ST_Distance(center.coord, loc.coord) AS distance ORDER BY distance"
        );
        assert_query_ok(result, "ST_DWithin should succeed");
    }

    /// EXPLAIN geography query
    #[test]
    fn test_explain_geography_query() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_geography_explain",
            &["CREATE TAG location(name: STRING NOT NULL, coord: GEOGRAPHY)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Insert data
        db.execute_query(
            "INSERT VERTEX location(name, coord) VALUES 'loc1': ('Beijing', ST_Point(116.4, 39.9))",
        )
        .expect("INSERT should succeed");

        // EXPLAIN
        let result = db.execute_query(
            "EXPLAIN MATCH (loc:location) WHERE ST_DWithin(ST_Point(116.4, 39.9), loc.coord, 10.0) RETURN loc.name"
        );
        assert_query_ok(result, "EXPLAIN geography query should succeed");
    }
}

/// Vector search tests
mod vector {
    use super::*;

    fn require_vector_coordinator(db: &TestDb) -> bool {
        if !db.has_vector_coordinator {
            eprintln!("SKIP: No vector coordinator available (Qdrant not running)");
            return false;
        }
        true
    }

    /// Insert vertex with vector (VECTOR type is core, works without qdrant)
    #[test]
    fn test_vector_insertion() {
        let mut db = create_test_db();
        setup_test_space(
        &mut db,
            "e2e_vector",
            &["CREATE TAG product_vector(product_id: STRING NOT NULL, name: STRING, category: STRING, embedding: VECTOR(128), price: DOUBLE)"],
            &[],
        ).expect("Failed to setup test space");

        let vector = vec![0.1; 128];
        let vector_str = vector
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        let result = db.execute_query(&format!(
            "INSERT VERTEX product_vector(product_id, name, category, embedding, price) VALUES 'pv_test': ('TEST001', 'Test Product', 'test', [{}]::VECTOR, 99.99)",
            vector_str
        ));
        assert_query_ok(result, "INSERT VECTOR should succeed");
    }

    /// Cosine similarity search (requires qdrant feature and running qdrant service)
    #[test]
    #[cfg_attr(not(feature = "qdrant"), ignore)]
    fn test_cosine_similarity() {
        let mut db = create_test_db();
        if !require_vector_coordinator(&db) {
            return;
        }
        setup_test_space(
        &mut db,
            "e2e_vector_search",
            &["CREATE TAG product_vector(product_id: STRING NOT NULL, name: STRING, embedding: VECTOR(128))"],
            &[],
        ).expect("Failed to setup test space");

        // Insert products with vectors
        for i in 0..100 {
            let vector: Vec<f64> = (0..128).map(|_| (i as f64) * 0.01).collect();
            let vector_str = vector
                .iter()
                .map(|v| format!("{:.4}", v))
                .collect::<Vec<_>>()
                .join(", ");

            db.execute_query(&format!(
                "INSERT VERTEX product_vector(product_id, name, embedding) VALUES 'pv{:03}': ('PROD{:03}', 'Product {}', [{}]::VECTOR)",
                i, i, i, vector_str
            )).expect("INSERT should succeed");
        }

        // Create vector index
        db.execute_query(
            "CREATE VECTOR INDEX idx_product_embedding ON product_vector(embedding) WITH (vector_size=128, distance='cosine')"
        ).expect("CREATE VECTOR INDEX should succeed");

        // Search vector
        let query_vector = vec![0.1; 128];
        let vector_str = query_vector
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        let result = db.execute_query(&format!(
            "SEARCH VECTOR idx_product_embedding WITH vector=[{}] YIELD product_id, name LIMIT 10",
            vector_str
        ));
        assert_query_ok(result, "SEARCH VECTOR should succeed");
    }

    /// Vector search with filter (requires qdrant feature and running qdrant service)
    #[test]
    #[cfg_attr(not(feature = "qdrant"), ignore)]
    fn test_filtered_vector_search() {
        let mut db = create_test_db();
        if !require_vector_coordinator(&db) {
            return;
        }
        setup_test_space(
        &mut db,
            "e2e_vector_filtered",
            &["CREATE TAG product_vector(product_id: STRING NOT NULL, name: STRING, embedding: VECTOR(128), price: DOUBLE)"],
            &[],
        ).expect("Failed to setup test space");

        // Insert data
        for i in 0..50 {
            let vector = vec![0.1; 128];
            let vector_str = vector
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ");

            db.execute_query(&format!(
                "INSERT VERTEX product_vector(product_id, name, embedding, price) VALUES 'pv{:03}': ('PROD{:03}', 'Product {}', [{}]::VECTOR, {}.0)",
                i, i, i, vector_str, i * 10
            )).expect("INSERT should succeed");
        }

        // Create vector index
        db.execute_query(
            "CREATE VECTOR INDEX idx_product_embedding ON product_vector(embedding) WITH (vector_size=128, distance='cosine')"
        ).expect("CREATE VECTOR INDEX should succeed");

        // Search with filter
        let query_vector = vec![0.1; 128];
        let vector_str = query_vector
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        let result = db.execute_query(&format!(
            "SEARCH VECTOR idx_product_embedding WITH vector=[{}] WHERE price < 500 YIELD product_id, name, price LIMIT 5",
            vector_str
        ));
        assert_query_ok(result, "SEARCH VECTOR with filter should succeed");
    }

    /// EXPLAIN vector query (requires qdrant feature and running qdrant service)
    #[test]
    #[cfg_attr(not(feature = "qdrant"), ignore)]
    fn test_explain_vector_query() {
        let mut db = create_test_db();
        if !require_vector_coordinator(&db) {
            return;
        }
        setup_test_space(
        &mut db,
            "e2e_vector_explain",
            &["CREATE TAG product_vector(product_id: STRING NOT NULL, name: STRING, embedding: VECTOR(128))"],
            &[],
        ).expect("Failed to setup test space");

        // Create vector index
        db.execute_query(
            "CREATE VECTOR INDEX idx_product_embedding ON product_vector(embedding) WITH (vector_size=128, distance='cosine')"
        ).expect("CREATE VECTOR INDEX should succeed");

        // Insert data
        let vector = vec![0.1; 128];
        let vector_str = vector
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        db.execute_query(&format!(
            "INSERT VERTEX product_vector(product_id, name, embedding) VALUES 'pv001': ('PROD001', 'Product 1', [{}]::VECTOR)",
            vector_str
        )).expect("INSERT should succeed");

        // EXPLAIN
        let result = db.execute_query(&format!(
            "EXPLAIN SEARCH VECTOR idx_product_embedding WITH vector=[{}] YIELD product_id, name LIMIT 10",
            vector_str
        ));
        assert_query_ok(result, "EXPLAIN SEARCH VECTOR should succeed");
    }
}

/// Full-text search tests
mod fulltext {
    use super::*;

    /// Create fulltext index
    #[test]
    #[cfg_attr(not(feature = "fulltext-search"), ignore)]
    fn test_fulltext_index_creation() {
        let mut db = create_test_db();
        setup_test_space(
        &mut db,
            "e2e_fulltext",
            &["CREATE TAG article(doc_id: STRING NOT NULL, title: STRING, content: STRING, author: STRING)"],
            &[],
        ).expect("Failed to setup test space");

        let result = db.execute_query(
            "CREATE FULLTEXT INDEX IF NOT EXISTS idx_article_content ON article(content) ENGINE BM25 OPTIONS (analyzer='standard')"
        );
        assert_query_ok(result, "CREATE FULLTEXT INDEX should succeed");
    }

    /// Basic fulltext search
    #[test]
    #[cfg_attr(not(feature = "fulltext-search"), ignore)]
    fn test_basic_search() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_fulltext_search",
            &["CREATE TAG article(doc_id: STRING NOT NULL, title: STRING, content: STRING)"],
            &[],
        )
        .expect("Failed to setup test space");

        db.execute_query(
            "CREATE FULLTEXT INDEX IF NOT EXISTS idx_article_content ON article(content) ENGINE BM25 OPTIONS (analyzer='standard')"
        ).expect("CREATE FULLTEXT INDEX should succeed");

        // Insert articles
        db.execute_query(
            "INSERT VERTEX article(doc_id, title, content) VALUES 'art001': ('art001', 'Graph Database Introduction', 'Graph databases are designed for connected data')"
        ).expect("INSERT should succeed");
        db.execute_query(
            "INSERT VERTEX article(doc_id, title, content) VALUES 'art002': ('art002', 'Query Optimization', 'Optimizing queries improves performance significantly')"
        ).expect("INSERT should succeed");

        // Search
        let result = db.execute_query(
            "SEARCH INDEX idx_article_content MATCH 'database' YIELD doc_id, title, score",
        );
        assert_query_ok(result, "SEARCH INDEX should succeed");
    }

    /// Boolean query search
    #[test]
    #[cfg_attr(not(feature = "fulltext-search"), ignore)]
    fn test_boolean_search() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_fulltext_bool",
            &["CREATE TAG article(doc_id: STRING NOT NULL, title: STRING, content: STRING)"],
            &[],
        )
        .expect("Failed to setup test space");

        db.execute_query(
            "CREATE FULLTEXT INDEX IF NOT EXISTS idx_article_content ON article(content) ENGINE BM25 OPTIONS (analyzer='standard')"
        ).expect("CREATE FULLTEXT INDEX should succeed");

        // Insert articles
        db.execute_query(
            "INSERT VERTEX article(doc_id, title, content) VALUES 'art001': ('art001', 'Graph Database', 'Graph databases are designed for connected data')"
        ).expect("INSERT should succeed");
        db.execute_query(
            "INSERT VERTEX article(doc_id, title, content) VALUES 'art002': ('art002', 'Query Optimization', 'Optimizing queries improves performance')"
        ).expect("INSERT should succeed");

        // Boolean search
        let result = db.execute_query(
            "SEARCH INDEX idx_article_content MATCH 'graph AND database' YIELD doc_id, title",
        );
        assert_query_ok(result, "SEARCH INDEX with boolean should succeed");
    }

    /// EXPLAIN fulltext search
    #[test]
    #[cfg_attr(not(feature = "fulltext-search"), ignore)]
    fn test_explain_fulltext() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_fulltext_explain",
            &["CREATE TAG article(doc_id: STRING NOT NULL, title: STRING, content: STRING)"],
            &[],
        )
        .expect("Failed to setup test space");

        db.execute_query(
            "CREATE FULLTEXT INDEX IF NOT EXISTS idx_article_content ON article(content) ENGINE BM25 OPTIONS (analyzer='standard')"
        ).expect("CREATE FULLTEXT INDEX should succeed");

        // Insert articles
        db.execute_query(
            "INSERT VERTEX article(doc_id, title, content) VALUES 'art001': ('art001', 'Performance Tuning', 'Performance tuning is crucial for database performance')"
        ).expect("INSERT should succeed");

        // EXPLAIN
        let result = db.execute_query(
            "EXPLAIN SEARCH INDEX idx_article_content MATCH 'performance' YIELD doc_id, score",
        );
        assert_query_ok(result, "EXPLAIN SEARCH INDEX should succeed");
    }
}

/// Cleanup tests
mod cleanup {
    use super::*;

    /// Drop all test spaces
    #[test]
    fn test_cleanup() {
        let mut db = create_test_db();

        let spaces = [
            "e2e_geography",
            "e2e_geography_wkt",
            "e2e_geography_dist",
            "e2e_geography_within",
            "e2e_geography_explain",
            "e2e_vector",
            "e2e_vector_search",
            "e2e_vector_filtered",
            "e2e_vector_explain",
            "e2e_fulltext",
            "e2e_fulltext_search",
            "e2e_fulltext_bool",
            "e2e_fulltext_explain",
        ];

        for space in &spaces {
            let _ = db.execute_query(&format!("DROP SPACE IF EXISTS {}", space));
        }
    }
}
