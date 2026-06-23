//! DQL Aggregation Tests
//!
//! Test coverage:
//! - GROUP BY - Grouping results
//! - ORDER BY - Sorting results
//! - LIMIT - Limiting results
//! - SKIP - Skipping results
//! - Aggregate functions: COUNT, SUM, AVG, MIN, MAX
//! - HAVING - Filtering grouped results

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::core::Value;
use graphdb_query::query::parser::Parser;

// ==================== GROUP BY Parser Tests ====================

#[test]
fn test_group_by_parser_basic() {
    let query = "MATCH (v:Person) RETURN v.age AS age, COUNT(*) AS count GROUP BY v.age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BY parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_group_by_parser_multiple() {
    let query = "MATCH (v:Person) RETURN v.age, v.name, COUNT(*) GROUP BY v.age, v.name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BY multiple fields parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== ORDER BY Parser Tests ====================

#[test]
fn test_order_by_parser_asc() {
    let query = "MATCH (v:Person) RETURN v.name ORDER BY v.age ASC";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ORDER BY ASC parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_order_by_parser_desc() {
    let query = "MATCH (v:Person) RETURN v.name ORDER BY v.age DESC";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ORDER BY DESC parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_order_by_parser_multiple() {
    let query = "MATCH (v:Person) RETURN v.name ORDER BY v.age ASC, v.name DESC";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ORDER BY multiple fields parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== LIMIT/SKIP Parser Tests ====================

#[test]
fn test_limit_parser_basic() {
    let query = "MATCH (v:Person) RETURN v.name LIMIT 10";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "LIMIT parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_skip_parser_basic() {
    let query = "MATCH (v:Person) RETURN v.name SKIP 5 LIMIT 10";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SKIP parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== Aggregate Function Parser Tests ====================

#[test]
fn test_count_parser() {
    let query = "MATCH (v:Person) RETURN COUNT(v) AS total";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "COUNT parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_sum_parser() {
    let query = "MATCH (v:Person) RETURN SUM(v.age) AS total_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SUM parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_avg_parser() {
    let query = "MATCH (v:Person) RETURN AVG(v.age) AS avg_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "AVG parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_min_max_parser() {
    let query = "MATCH (v:Person) RETURN MIN(v.age) AS min_age, MAX(v.age) AS max_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MIN/MAX parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== Aggregation Execution Tests ====================

#[test]
fn test_count_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C')")
        .assert_success()
        .query("MATCH (v:Person) RETURN COUNT(v) AS total")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(3)]);
}

#[test]
fn test_order_by_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 20), 3:('Charlie', 25)")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.name, v.age ORDER BY v.age ASC")
        .assert_success();
}

#[test]
fn test_limit_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C'), 4:('D'), 5:('E')")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.name LIMIT 3")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_skip_limit_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C'), 4:('D'), 5:('E')")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.name SKIP 2 LIMIT 2")
        .assert_success()
        .assert_result_count(2);
}

// ==================== HAVING Parser Tests ====================

#[test]
fn test_having_parser_basic() {
    let query = "MATCH (v:Person) RETURN v.age, COUNT(*) AS cnt GROUP BY v.age HAVING cnt > 1";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "HAVING parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_having_parser_with_count_star() {
    let query =
        "MATCH (v:Person) RETURN v.city, COUNT(*) AS cnt GROUP BY v.city HAVING COUNT(*) >= 2";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "HAVING with COUNT(*) parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_having_parser_with_sum() {
    let query = "MATCH (v:Product) RETURN v.category, SUM(v.price) AS total GROUP BY v.category HAVING total > 100";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "HAVING with SUM parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== HAVING Execution Tests ====================

#[test]
fn test_having_execution_filter_some() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, city STRING)")
        .exec_dml("INSERT VERTEX Person(name, city) VALUES 1:('Alice', 'NYC'), 2:('Bob', 'LA'), 3:('Charlie', 'NYC'), 4:('David', 'LA'), 5:('Eve', 'SF')")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.city, COUNT(*) AS cnt GROUP BY v.city HAVING cnt >= 2")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_having_execution_filter_all() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, city STRING)")
        .exec_dml("INSERT VERTEX Person(name, city) VALUES 1:('Alice', 'NYC'), 2:('Bob', 'LA'), 3:('Charlie', 'SF')")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.city, COUNT(*) AS cnt GROUP BY v.city HAVING cnt >= 2")
        .assert_success()
        .assert_result_empty();
}

#[test]
fn test_having_execution_sum_condition() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Sale(category STRING, amount INT)")
        .exec_dml("INSERT VERTEX Sale(category, amount) VALUES 1:('A', 100), 2:('A', 50), 3:('B', 30)")
        .assert_success()
        .query("MATCH (s:Sale) RETURN s.category, SUM(s.amount) AS total GROUP BY s.category HAVING total > 100")
        .assert_success()
        .assert_result_count(1);
}

// ==================== Aggregate Function Execution Tests ====================

#[test]
fn test_sum_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Product(name STRING, price DOUBLE)")
        .exec_dml("INSERT VERTEX Product(name, price) VALUES 1:('Laptop', 999.99), 2:('Mouse', 29.99), 3:('Keyboard', 79.99)")
        .assert_success()
        .query("MATCH (p:Product) RETURN SUM(p.price) AS total_price")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::Double(999.99 + 29.99 + 79.99)]);
}

#[test]
fn test_avg_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
        .assert_success()
        .query("MATCH (p:Person) RETURN AVG(p.age) AS avg_age")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_min_max_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Product(name STRING, price DOUBLE)")
        .exec_dml("INSERT VERTEX Product(name, price) VALUES 1:('Laptop', 999.99), 2:('Mouse', 29.99), 3:('Keyboard', 79.99)")
        .assert_success()
        .query("MATCH (p:Product) RETURN MIN(p.price) AS min_price, MAX(p.price) AS max_price")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::Double(29.99)])
        .assert_result_contains(vec![Value::Double(999.99)]);
}

#[test]
fn test_sum_int_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Item(name STRING, quantity INT)")
        .exec_dml("INSERT VERTEX Item(name, quantity) VALUES 1:('A', 10), 2:('B', 20), 3:('C', 30)")
        .assert_success()
        .query("MATCH (i:Item) RETURN SUM(i.quantity) AS total_qty")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(60)]);
}

#[test]
fn test_group_by_with_sum_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Sale(category STRING, amount INT)")
        .exec_dml(
            "INSERT VERTEX Sale(category, amount) VALUES 1:('A', 100), 2:('A', 200), 3:('B', 150)",
        )
        .assert_success()
        .query("MATCH (s:Sale) RETURN s.category, SUM(s.amount) AS total GROUP BY s.category")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_group_by_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Product(name STRING, category STRING, price DOUBLE)")
        .exec_dml("INSERT VERTEX Product(name, category, price) VALUES 1:('Laptop', 'Electronics', 999.99), 2:('Mouse', 'Electronics', 29.99), 3:('Keyboard', 'Electronics', 79.99), 4:('Desk', 'Furniture', 299.99)")
        .assert_success()
        .query("MATCH (p:Product) RETURN p.category, COUNT(*) AS count ORDER BY count DESC")
        .assert_success()
        .assert_result_count(2);
}
