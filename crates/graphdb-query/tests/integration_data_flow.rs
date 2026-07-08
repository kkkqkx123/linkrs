//! Data Flow Integration Tests
//!
//! These tests demonstrate complete data flows across DDL, DML, and DQL operations,
//! validating that data changes are correctly reflected in subsequent queries.

mod common;

use common::test_scenario::TestScenario;
use graphdb_query::core::Value;
use std::collections::HashMap;

// ==================== Basic CRUD Flow ====================

#[test]
fn test_basic_crud_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        // Create
        .exec_ddl("CREATE TAG User(username STRING, email STRING, active BOOL)")
        .assert_success()
        .assert_tag_exists("User")
        // Read (empty)
        .query("MATCH (u:User) RETURN u.username")
        .assert_result_empty()
        // Create data
        .exec_dml("INSERT VERTEX User(username, email, active) VALUES 1:('alice', 'alice@example.com', true)")
        .assert_success()
        .assert_vertex_exists(1, "User")
        // Read
        .query("MATCH (u:User) RETURN u.username, u.email")
        .assert_result_count(1)
        .assert_result_contains(vec![
            Value::String("alice".into()),
            Value::String("alice@example.com".into()),
        ])
        // Update
        .exec_dml("UPDATE 1 SET email = 'newalice@example.com'")
        .assert_success()
        .assert_vertex_props(1, "User", {
            let mut map = HashMap::new();
            map.insert("email", Value::String("newalice@example.com".into()));
            map
        })
        // Read updated
        .query("MATCH (u:User) RETURN u.email")
        .assert_result_contains(vec![Value::String("newalice@example.com".into())])
        // Delete
        .exec_dml("DELETE VERTEX 1")
        .assert_success()
        .assert_vertex_not_exists(1, "User")
        // Read (empty again)
        .query("MATCH (u:User) RETURN u.username")
        .assert_result_empty();
}

// ==================== Schema Evolution Flow ====================

#[test]
fn test_schema_evolution_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        // Initial schema
        .exec_ddl("CREATE TAG Product(name STRING, price DOUBLE)")
        .assert_success()
        // Insert data
        .exec_dml("INSERT VERTEX Product(name, price) VALUES 1:('Laptop', 999.99)")
        .assert_success()
        // Query with initial schema
        .query("MATCH (p:Product) RETURN p.name, p.price")
        .assert_result_count(1)
        // Evolve schema - add field
        .exec_ddl("ALTER TAG Product ADD (stock INT)")
        .assert_success()
        // Update existing data with new field
        .exec_dml("UPDATE 1 SET stock = 10")
        .assert_success()
        // Query with new field
        .query("MATCH (p:Product) RETURN p.name, p.price, p.stock")
        .assert_result_count(1)
        .assert_result_contains(vec![
            Value::String("Laptop".into()),
            Value::Double(999.99),
            Value::BigInt(10),
        ])
        // Evolve schema - add another field
        .exec_ddl("ALTER TAG Product ADD (category STRING)")
        .assert_success()
        // Update with new field
        .exec_dml("UPDATE 1 SET category = 'Electronics'")
        .assert_success()
        // Query all fields
        .query("MATCH (p:Product) RETURN p.name, p.category")
        .assert_result_contains(vec![
            Value::String("Laptop".into()),
            Value::String("Electronics".into()),
        ]);
}

// ==================== Relationship Flow ====================

#[test]
fn test_relationship_crud_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("social_network")
        // Setup
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE FOLLOWS(since DATE) FROM Person TO Person")
        .assert_success()
        // Create persons
        .exec_dml(
            r#"
            INSERT VERTEX Person(name) VALUES 
                1:('Alice'),
                2:('Bob'),
                3:('Charlie')
        "#,
        )
        .assert_success()
        .assert_vertex_count("Person", 3)
        // Create relationships
        .exec_dml(
            r#"
            INSERT EDGE FOLLOWS(since) VALUES 
                1 -> 2:('2020-01-01'),
                1 -> 3:('2021-01-01')
        "#,
        )
        .assert_success()
        .assert_edge_count("FOLLOWS", 2)
        // Query relationships
        .query("GO FROM 1 OVER FOLLOWS YIELD $$.Person.name AS following")
        .debug_print_result()
        .assert_result_count(2)
        // Update relationship
        .exec_ddl("ALTER EDGE FOLLOWS ADD (strength DOUBLE)")
        .assert_success()
        .exec_dml("UPDATE 1 -> 2 OF FOLLOWS SET strength = 0.9")
        .assert_success()
        // Query updated relationship
        .query("FETCH PROP ON FOLLOWS 1 -> 2")
        .assert_success()
        // Delete relationship
        .exec_dml("DELETE EDGE FOLLOWS 1 -> 2")
        .assert_success()
        .assert_edge_not_exists(1, 2, "FOLLOWS")
        .assert_edge_count("FOLLOWS", 1)
        // Query remaining relationships
        .query("GO FROM 1 OVER FOLLOWS YIELD $$.Person.name AS following")
        .assert_result_count(1);
}

// ==================== Transaction-like Flow ====================

#[test]
fn test_ecommerce_order_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ecommerce")
        // Schema
        .exec_ddl("CREATE TAG Customer(name STRING, email STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG Product(name STRING, price DOUBLE, stock INT)")
        .assert_success()
        .exec_ddl("CREATE TAG Order(order_date DATE, total DOUBLE)")
        .assert_success()
        .exec_ddl("CREATE EDGE PURCHASED(quantity INT) FROM Customer TO Order")
        .assert_success()
        .exec_ddl("CREATE EDGE CONTAINS(quantity INT, price DOUBLE) FROM Order TO Product")
        .assert_success()
        // Insert customers
        .exec_dml("INSERT VERTEX Customer(name, email) VALUES 1:('John Doe', 'john@example.com')")
        .assert_success()
        // Insert products
        .exec_dml(
            r#"
            INSERT VERTEX Product(name, price, stock) VALUES 
                101:('Laptop', 999.99, 10),
                102:('Mouse', 29.99, 50)
        "#,
        )
        .assert_success()
        // Create order
        .exec_dml("INSERT VERTEX Order(order_date, total) VALUES 1001:('2024-01-15', 1029.98)")
        .assert_success()
        // Link customer to order
        .exec_dml("INSERT EDGE PURCHASED(quantity) VALUES 1 -> 1001:(1)")
        .assert_success()
        // Link order to products
        .exec_dml(
            r#"
            INSERT EDGE CONTAINS(quantity, price) VALUES 
                1001 -> 101:(1, 999.99),
                1001 -> 102:(1, 29.99)
        "#,
        )
        .assert_success()
        // Update product stock
        .exec_dml("UPDATE 101 SET stock = stock - 1")
        .assert_success()
        .exec_dml("UPDATE 102 SET stock = stock - 1")
        .assert_success()
        // Verify stock update
        .assert_vertex_props(101, "Product", {
            let mut map = HashMap::new();
            map.insert("stock", Value::BigInt(9));
            map
        })
        .assert_vertex_props(102, "Product", {
            let mut map = HashMap::new();
            map.insert("stock", Value::BigInt(49));
            map
        })
        // Test single-hop first
        .query(
            r#"
            MATCH (c:Customer)-[:PURCHASED]->(o:Order)
            WHERE c.name == 'John Doe'
            RETURN o.order_date
        "#,
        )
        .assert_result_count(1)
        // Query order details - multi-hop MATCH
        .query(
            r#"
            MATCH (c:Customer)-[:PURCHASED]->(o:Order)-[:CONTAINS]->(p:Product)
            WHERE c.name == 'John Doe'
            RETURN o.order_date, p.name, p.price
        "#,
        )
        .assert_result_count(2);
}

// ==================== Social Network Flow ====================

#[test]
fn test_social_network_complete_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("social_network")
        // Schema
        .exec_ddl(
            r#"
            CREATE TAG Person(
                name STRING,
                age INT,
                city STRING,
                join_date DATE
            )
        "#,
        )
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE) FROM Person TO Person")
        .assert_success()
        // Create users
        .exec_dml(
            r#"
            INSERT VERTEX Person(name, age, city, join_date) VALUES 
                1:('Alice', 30, 'NYC', '2020-01-01'),
                2:('Bob', 25, 'LA', '2020-06-01'),
                3:('Charlie', 35, 'NYC', '2021-01-01'),
                4:('David', 28, 'LA', '2021-06-01')
        "#,
        )
        .assert_success()
        .assert_vertex_count("Person", 4)
        // Create friendships
        .exec_dml(
            r#"
            INSERT EDGE KNOWS(since, strength) VALUES 
                1 -> 2:('2020-06-01', 0.9),
                1 -> 3:('2021-01-01', 0.8),
                2 -> 4:('2021-06-01', 0.7),
                3 -> 4:('2022-01-01', 0.9)
        "#,
        )
        .assert_success()
        .assert_edge_count("KNOWS", 4)
        // Query: Find all people in NYC
        .query("MATCH (p:Person) WHERE p.city == 'NYC' RETURN p.name, p.age")
        .assert_result_count(2)
        // Query: Find Alice's friends
        .query("GO FROM 1 OVER KNOWS YIELD $$.Person.name AS friend_name")
        .assert_result_count(2)
        // Query: Find friends of friends of Alice
        .query("GO 2 FROM 1 OVER KNOWS YIELD $$.Person.name AS fof_name")
        .assert_result_count(1)
        .assert_result_contains(vec![Value::String("David".into())])
        // Query: Find shortest path from Alice to David
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS")
        .assert_success()
        // Update: Alice moves to LA
        .exec_dml("UPDATE 1 SET city = 'LA'")
        .assert_success()
        // Query: Verify update
        .query("MATCH (p:Person) WHERE p.name == 'Alice' RETURN p.city")
        .assert_result_contains(vec![Value::String("LA".into())])
        // Delete: Remove a friendship
        .exec_dml("DELETE EDGE KNOWS 1 -> 2")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_count("KNOWS", 3);
}

// ==================== Index and Query Flow ====================

#[test]
fn test_index_query_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        // Schema without index
        .exec_ddl("CREATE TAG User(username STRING, age INT)")
        .assert_success()
        // Insert data
        .exec_dml(
            r#"
            INSERT VERTEX User(username, age) VALUES 
                1:('user1', 25),
                2:('user2', 30),
                3:('user3', 25),
                4:('user4', 35)
        "#,
        )
        .assert_success()
        // Query without index (full scan)
        .query("MATCH (u:User) WHERE u.age == 25 RETURN u.username")
        .assert_result_count(2)
        // Create index
        .exec_ddl("CREATE TAG INDEX idx_user_age ON User(age)")
        .assert_success()
        // Query with index
        .query("LOOKUP ON User WHERE User.age == 25")
        .assert_result_count(2)
        // Insert more data
        .exec_dml("INSERT VERTEX User(username, age) VALUES 5:('user5', 25)")
        .assert_success()
        // Query again
        .query("LOOKUP ON User WHERE User.age == 25")
        .assert_result_count(3)
        // Drop index
        .exec_ddl("DROP TAG INDEX idx_user_age")
        .assert_success()
        // Query still works (full scan)
        .query("MATCH (u:User) WHERE u.age == 25 RETURN u.username")
        .assert_result_count(3);
}

// ==================== Batch Operations Flow ====================

#[test]
fn test_batch_operations_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        // Schema
        .exec_ddl("CREATE TAG Item(name STRING, category STRING, price DOUBLE)")
        .assert_success()
        // Batch insert
        .exec_dml(
            r#"
            INSERT VERTEX Item(name, category, price) VALUES 
                1:('Item1', 'A', 10.0),
                2:('Item2', 'A', 20.0),
                3:('Item3', 'B', 30.0),
                4:('Item4', 'B', 40.0),
                5:('Item5', 'A', 50.0)
        "#,
        )
        .assert_success()
        .assert_vertex_count("Item", 5)
        // Query by category
        .query("MATCH (i:Item) WHERE i.category == 'A' RETURN i.name, i.price")
        .assert_result_count(3)
        // Batch update
        .exec_dml("UPDATE 1 SET price = price * 1.1")
        .assert_success()
        .exec_dml("UPDATE 2 SET price = price * 1.1")
        .assert_success()
        .exec_dml("UPDATE 5 SET price = price * 1.1")
        .assert_success()
        // Verify updates
        .assert_vertex_props(1, "Item", {
            let mut map = HashMap::new();
            map.insert("price", Value::Double(11.0));
            map
        })
        // Batch delete
        .exec_dml("DELETE VERTEX 3, 4")
        .assert_success()
        .assert_vertex_count("Item", 3)
        .assert_vertex_not_exists(3, "Item")
        .assert_vertex_not_exists(4, "Item");
}

// ==================== Complex Aggregation Flow ====================

#[test]
fn test_aggregation_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        // Schema
        .exec_ddl("CREATE TAG Order(order_id STRING, amount DOUBLE, status STRING)")
        .assert_success()
        // Insert orders
        .exec_dml(
            r#"
            INSERT VERTEX Order(order_id, amount, status) VALUES 
                1:('ORD001', 100.0, 'completed'),
                2:('ORD002', 200.0, 'completed'),
                3:('ORD003', 150.0, 'pending'),
                4:('ORD004', 300.0, 'completed'),
                5:('ORD005', 50.0, 'cancelled')
        "#,
        )
        .assert_success()
        // Count by status
        .query(
            r#"
            MATCH (o:Order)
            RETURN o.status, count(*) AS count
            ORDER BY count DESC
        "#,
        )
        .assert_result_count(3)
        // Sum by status
        .query(
            r#"
            MATCH (o:Order)
            WHERE o.status == 'completed'
            RETURN sum(o.amount) AS total_completed
        "#,
        )
        .assert_success()
        // Average amount
        .query(
            r#"
            MATCH (o:Order)
            WHERE o.status == 'completed'
            RETURN avg(o.amount) AS avg_amount
        "#,
        )
        .assert_success();
}
