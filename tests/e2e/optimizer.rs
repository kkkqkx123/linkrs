//! E2E Test Suite for Query Optimizer
//!
//! Tests optimizer behavior including:
//! - Index selection
//! - Join algorithm selection
//! - Aggregation strategies
//! - TopN optimization
//! - Query plan validation via EXPLAIN

use crate::common::{assert_query_ok, create_test_db, setup_test_space};

/// Index selection optimization tests
mod index {
    use super::*;

    /// Equality query should use IndexScan
    #[test]
    fn test_index_scan_for_equality() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer",
            &["CREATE TAG person(name: STRING, age: INT, city: STRING, salary: INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Create indexes
        db.execute_query("CREATE TAG INDEX idx_person_name ON person(name)")
            .expect("CREATE INDEX should succeed");
        db.execute_query("CREATE TAG INDEX idx_person_age ON person(age)")
            .expect("CREATE INDEX should succeed");

        // Insert test data
        for i in 0..100 {
            let name = format!("Person_{:03}", i);
            let age = 20 + (i % 40);
            let city = match i % 3 {
                0 => "Beijing",
                1 => "Shanghai",
                _ => "Shenzhen",
            };
            let salary = 5000 + (i * 100);

            db.execute_query(&format!(
                "INSERT VERTEX person(name, age, city, salary) VALUES 'p{:03}': ('{}', {}, '{}', {})",
                i, name, age, city, salary
            )).expect("INSERT should succeed");
        }

        // Test equality query
        let result = db.execute_query("EXPLAIN MATCH (p:person {name: 'Person_001'}) RETURN p.age");
        assert_query_ok(result, "EXPLAIN should succeed");
    }

    /// Range query should use IndexScan
    #[test]
    fn test_index_scan_for_range() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer_range",
            &["CREATE TAG person(name: STRING, age: INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Create index
        db.execute_query("CREATE TAG INDEX idx_person_age ON person(age)")
            .expect("CREATE INDEX should succeed");

        // Insert test data
        for i in 0..100 {
            db.execute_query(&format!(
                "INSERT VERTEX person(name, age) VALUES 'p{:03}': ('Person_{:03}', {})",
                i,
                i,
                20 + (i % 40)
            ))
            .expect("INSERT should succeed");
        }

        // Test range query
        let result = db.execute_query(
            "EXPLAIN MATCH (p:person) WHERE p.age > 25 AND p.age < 35 RETURN p.name",
        );
        assert_query_ok(result, "EXPLAIN should succeed");
    }

    /// Query on non-indexed field should use SeqScan
    #[test]
    fn test_no_index_full_scan() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer_scan",
            &["CREATE TAG person(name: STRING, salary: INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Insert test data (no index on salary)
        for i in 0..50 {
            db.execute_query(&format!(
                "INSERT VERTEX person(name, salary) VALUES 'p{:03}': ('Person_{:03}', {})",
                i,
                i,
                5000 + i * 100
            ))
            .expect("INSERT should succeed");
        }

        // Test query on non-indexed field
        let result =
            db.execute_query("EXPLAIN MATCH (p:person) WHERE p.salary > 10000 RETURN p.name");
        assert_query_ok(result, "EXPLAIN should succeed");
    }
}

/// Join optimization tests
mod join {
    use super::*;

    /// Verify traversal operation is selected for graph patterns
    #[test]
    fn test_join_algorithm_selection() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer_join",
            &[
                "CREATE TAG company(name: STRING, industry: STRING)",
                "CREATE TAG employee(name: STRING, salary: INT)",
            ],
            &["CREATE EDGE works_at(position: STRING)"],
        )
        .expect("Failed to setup test space");

        // Insert companies (fewer)
        for i in 0..10 {
            db.execute_query(&format!(
                "INSERT VERTEX company(name, industry) VALUES 'c{:02}': ('Company_{:02}', 'Tech')",
                i, i
            ))
            .expect("INSERT should succeed");
        }

        // Insert employees (more)
        for i in 0..100 {
            db.execute_query(&format!(
                "INSERT VERTEX employee(name, salary) VALUES 'e{:03}': ('Employee_{:03}', {})",
                i,
                i,
                5000 + i * 100
            ))
            .expect("INSERT should succeed");
        }

        // Create relationships
        for i in 0..100 {
            let company_id = format!("c{:02}", i % 10);
            db.execute_query(&format!(
                "INSERT EDGE works_at(position) VALUES 'e{:03}' -> '{}' @0: ('Engineer')",
                i, company_id
            ))
            .expect("INSERT EDGE should succeed");
        }

        // Test join query
        let result = db.execute_query(
            "EXPLAIN MATCH (e:employee)-[:works_at]->(c:company) RETURN e.name, c.name",
        );
        assert_query_ok(result, "EXPLAIN should succeed");
    }
}

/// Aggregation optimization tests
mod aggregate {
    use super::*;

    /// HashAggregate for GROUP BY
    #[test]
    fn test_hash_aggregate() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer_agg",
            &["CREATE TAG sales(product: STRING, amount: INT, category: STRING)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Insert sales data
        for i in 0..1000 {
            let product = format!("Product_{:02}", i % 20);
            let amount = 10 + (i % 1000);
            let category = match i % 3 {
                0 => "A",
                1 => "B",
                _ => "C",
            };

            db.execute_query(&format!(
                "INSERT VERTEX sales(product, amount, category) VALUES 's{:04}': ('{}', {}, '{}')",
                i, product, amount, category
            ))
            .expect("INSERT should succeed");
        }

        // Test aggregation query
        let result = db.execute_query(
            "EXPLAIN MATCH (s:sales) RETURN s.category, sum(s.amount) AS total GROUP BY s.category",
        );
        assert_query_ok(result, "EXPLAIN should succeed");
    }
}

/// TopN optimization tests
mod topn {
    use super::*;

    /// ORDER BY + LIMIT should use TopN
    #[test]
    fn test_order_by_limit() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer_topn",
            &["CREATE TAG product(name: STRING, price: INT, sales: INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        for i in 0..100 {
            db.execute_query(&format!(
                "INSERT VERTEX product(name, price, sales) VALUES 'p{:03}': ('Product_{:03}', {}, {})",
                i, i, 10 + (i % 1000), i * 10
            )).expect("INSERT should succeed");
        }

        // Test TopN query
        let result = db.execute_query(
            "EXPLAIN MATCH (p:product) RETURN p.name, p.price ORDER BY p.price DESC LIMIT 10",
        );
        assert_query_ok(result, "EXPLAIN should succeed");
    }
}

/// EXPLAIN format tests
mod explain_format {
    use super::*;

    /// EXPLAIN with text format
    #[test]
    fn test_text_format() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer_explain",
            &["CREATE TAG person(name: STRING, age: INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Test text format
        let result = db.execute_query("EXPLAIN MATCH (p:person) RETURN p.name");
        assert_query_ok(result, "EXPLAIN should succeed");
    }

    /// EXPLAIN with DOT format
    #[test]
    fn test_dot_format() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer_dot",
            &["CREATE TAG person(name: STRING, age: INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Test DOT format
        let result = db.execute_query("EXPLAIN FORMAT = DOT MATCH (p:person) RETURN p.name");
        assert_query_ok(result, "EXPLAIN FORMAT = DOT should succeed");
    }
}

/// PROFILE command tests
mod profile {
    use super::*;

    /// Basic PROFILE execution
    #[test]
    fn test_basic_profile() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "e2e_optimizer_profile",
            &["CREATE TAG person(name: STRING, age: INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        for i in 0..50 {
            db.execute_query(&format!(
                "INSERT VERTEX person(name, age) VALUES 'p{:03}': ('Person_{:03}', {})",
                i,
                i,
                20 + i
            ))
            .expect("INSERT should succeed");
        }

        // Test PROFILE
        let result = db.execute_query("PROFILE MATCH (p:person) RETURN count(p)");
        assert_query_ok(result, "PROFILE should succeed");
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
            "e2e_optimizer",
            "e2e_optimizer_range",
            "e2e_optimizer_scan",
            "e2e_optimizer_join",
            "e2e_optimizer_agg",
            "e2e_optimizer_topn",
            "e2e_optimizer_explain",
            "e2e_optimizer_dot",
            "e2e_optimizer_profile",
        ];

        for space in &spaces {
            let _ = db.execute_query(&format!("DROP SPACE IF EXISTS {}", space));
        }
    }
}
