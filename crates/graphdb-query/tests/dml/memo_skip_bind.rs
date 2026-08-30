//! memo-hit bind-skip regression tests.
//!
//! When the Level-2 DML plan memo hits at prepare time, the binder result
//! would be discarded — `prepare_request` must skip binding entirely. Every
//! miss cause (cold, DDL invalidation, space switch, parameter type change)
//! must fall back to the bind path and still produce correct results.

use super::common::TestStorage;
use graphdb_core::stats::StatsManager;
use graphdb_core::types::VertexId;
use graphdb_query::optimizer::OptimizerEngine;
use graphdb_query::pipeline::QueryPipelineManager;
use graphdb_query::storage::StorageReader;
use parking_lot::RwLock;
use std::sync::Arc;

struct Fixture {
    storage: Arc<RwLock<graphdb_query::storage::GraphStorage>>,
    pipeline: QueryPipelineManager<graphdb_query::storage::GraphStorage>,
}

impl Fixture {
    fn new() -> Self {
        let test_storage = TestStorage::new().expect("Failed to create test storage");
        let storage = test_storage.storage();
        let stats_manager = Arc::new(StatsManager::new());
        let schema_manager = test_storage.schema_manager();
        let pipeline = QueryPipelineManager::with_optimizer(
            storage.clone(),
            stats_manager,
            Arc::new(OptimizerEngine::default()),
        )
        .with_schema_manager(schema_manager);
        Self { storage, pipeline }
    }

    fn create_space_with_person_tag(
        &mut self,
        space_name: &str,
    ) -> graphdb_core::types::SpaceInfo {
        self.pipeline
            .execute_query_with_space(&format!("CREATE SPACE IF NOT EXISTS {space_name}"), None)
            .expect("create space");
        let space = self
            .storage
            .read()
            .get_space(space_name)
            .expect("space lookup")
            .expect("space exists");
        self.pipeline
            .execute_query_with_space(
                "CREATE TAG person(name STRING, age INT)",
                Some(space.clone()),
            )
            .expect("create tag");
        space
    }

    fn insert(&mut self, space: &graphdb_core::types::SpaceInfo, query: &str) {
        self.pipeline
            .execute_query_with_space(query, Some(space.clone()))
            .expect("insert should succeed");
    }

    fn name_of(&self, space: &str, vid: &str) -> Option<graphdb_core::Value> {
        self.storage
            .read()
            .get_vertex(space, &VertexId::from_string(vid))
            .expect("vertex read")
            .and_then(|v| v.properties.get("name").cloned())
    }
}

/// Warm same-shape statements skip the bind step entirely; cold statements
/// bind. Both must produce identical, correct results.
#[test]
fn test_memo_hit_skips_bind_on_warm_statement() {
    let mut fx = Fixture::new();
    let space = fx.create_space_with_person_tag("memo_space");

    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p1\": (\"A\", 1)",
    );
    assert_eq!(
        fx.pipeline.dml_bind_skipped_count(),
        0,
        "cold statement must bind"
    );

    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p2\": (\"B\", 2)",
    );
    assert_eq!(
        fx.pipeline.dml_bind_skipped_count(),
        1,
        "warm same-shape statement must skip the bind step"
    );

    assert_eq!(
        fx.name_of("memo_space", "p1"),
        Some(graphdb_core::Value::string("A"))
    );
    assert_eq!(
        fx.name_of("memo_space", "p2"),
        Some(graphdb_core::Value::string("B"))
    );
}

/// DDL between prepare and execute bumps the schema version: the memo must
/// miss, restoring the bind path, then re-arm for the next warm statement.
#[test]
fn test_ddl_invalidation_restores_bind_path() {
    let mut fx = Fixture::new();
    let space = fx.create_space_with_person_tag("ddl_space");

    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p1\": (\"A\", 1)",
    );
    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p2\": (\"B\", 2)",
    );
    assert_eq!(fx.pipeline.dml_bind_skipped_count(), 1);

    fx.pipeline
        .execute_query_with_space("ALTER TAG person ADD (note STRING)", Some(space.clone()))
        .expect("alter tag");

    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p3\": (\"C\", 3)",
    );
    assert_eq!(
        fx.pipeline.dml_bind_skipped_count(),
        1,
        "statement after DDL must miss the memo and bind"
    );

    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p4\": (\"D\", 4)",
    );
    assert_eq!(
        fx.pipeline.dml_bind_skipped_count(),
        2,
        "memo re-armed under the new schema version"
    );

    assert_eq!(
        fx.name_of("ddl_space", "p3"),
        Some(graphdb_core::Value::string("C"))
    );
    assert_eq!(
        fx.name_of("ddl_space", "p4"),
        Some(graphdb_core::Value::string("D"))
    );
}

/// The same normalized template text in two different spaces binds to
/// different label ids: the memo (now space-scoped) must miss on the switch
/// and bind per space.
#[test]
fn test_space_switch_restores_bind_path() {
    let mut fx = Fixture::new();
    let space_a = fx.create_space_with_person_tag("space_a");
    let space_b = fx.create_space_with_person_tag("space_b");

    fx.insert(
        &space_a,
        "INSERT VERTEX person(name, age) VALUES \"p1\": (\"A\", 1)",
    );
    fx.insert(
        &space_a,
        "INSERT VERTEX person(name, age) VALUES \"p2\": (\"B\", 2)",
    );
    assert_eq!(fx.pipeline.dml_bind_skipped_count(), 1);

    fx.insert(
        &space_b,
        "INSERT VERTEX person(name, age) VALUES \"q1\": (\"X\", 9)",
    );
    assert_eq!(
        fx.pipeline.dml_bind_skipped_count(),
        1,
        "first statement in the other space must miss the memo and bind"
    );

    fx.insert(
        &space_b,
        "INSERT VERTEX person(name, age) VALUES \"q2\": (\"Y\", 8)",
    );
    assert_eq!(
        fx.pipeline.dml_bind_skipped_count(),
        2,
        "warm statement inside the new space reuses the memo"
    );

    // Data must have landed in the correct spaces (label id resolution differs
    // per space; a wrongly reused plan would write into the wrong table).
    assert_eq!(
        fx.name_of("space_a", "p1"),
        Some(graphdb_core::Value::string("A"))
    );
    assert_eq!(
        fx.name_of("space_a", "p2"),
        Some(graphdb_core::Value::string("B"))
    );
    assert_eq!(
        fx.name_of("space_b", "q1"),
        Some(graphdb_core::Value::string("X"))
    );
    assert_eq!(
        fx.name_of("space_b", "q2"),
        Some(graphdb_core::Value::string("Y"))
    );
    assert!(fx
        .storage
        .read()
        .get_vertex("space_b", &VertexId::from_string("p1"))
        .expect("read")
        .is_none());
}

/// A parameter type change inside the same template changes the memo key:
/// the bind path must be restored and results stay correct.
#[test]
fn test_param_type_change_restores_bind_path() {
    let mut fx = Fixture::new();
    let space = fx.create_space_with_person_tag("param_type_space");

    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p1\": (\"A\", 1)",
    );
    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p2\": (\"B\", 2)",
    );
    assert_eq!(fx.pipeline.dml_bind_skipped_count(), 1);

    // Same template, but the age slot now carries a Double: the param type
    // signature changes, so the memo must miss and the bind path runs.
    fx.insert(
        &space,
        "INSERT VERTEX person(name, age) VALUES \"p3\": (\"C\", 3.0)",
    );
    assert_eq!(
        fx.pipeline.dml_bind_skipped_count(),
        1,
        "param type change must miss the memo and bind"
    );

    assert_eq!(
        fx.name_of("param_type_space", "p3"),
        Some(graphdb_core::Value::string("C"))
    );
    let p3 = fx
        .storage
        .read()
        .get_vertex("param_type_space", &VertexId::from_string("p3"))
        .expect("read")
        .expect("exists");
    assert_eq!(
        p3.properties.get("age"),
        Some(&graphdb_core::Value::Double(3.0))
    );
}
