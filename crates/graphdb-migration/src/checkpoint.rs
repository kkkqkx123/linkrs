pub use crate::plan::{checkpoint_now_millis as now_millis, MigrationCheckpoint, StepResult};

#[cfg(test)]
mod tests {
    use crate::plan::{MigrationCheckpoint, MigrationTarget, SafetyLevel, StepResult, VersionRange};
    use super::now_millis;

    fn make_plan(hash: &str) -> crate::plan::MigrationPlan {
        let mut p = crate::plan::MigrationPlan::new(
            MigrationTarget {
                space: "s".into(),
                label: "l".into(),
                is_edge: false,
            },
            VersionRange { from: 1, to: 2 },
            vec![],
            0,
            SafetyLevel::Safe,
            None,
        );
        p.plan_hash = hash.to_string();
        p
    }

    #[test]
    fn test_checkpoint_save_load_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = make_plan("abc123");
        let cp = MigrationCheckpoint {
            completed_step_index: 1,
            rows_migrated_before: 0,
            rows_migrated_after: 10,
            timestamp: now_millis(),
            step_result: StepResult::Success,
        };
        cp.save(&plan, tmp.path()).unwrap();
        let loaded = MigrationCheckpoint::load(&plan, tmp.path()).unwrap().unwrap();
        assert_eq!(loaded.completed_step_index, 1);
        assert_eq!(loaded.rows_migrated_after, 10);
        MigrationCheckpoint::cleanup(&plan, tmp.path()).unwrap();
        assert!(MigrationCheckpoint::load(&plan, tmp.path()).unwrap().is_none());
    }

    #[test]
    fn test_checkpoint_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = make_plan("hash2");
        assert!(MigrationCheckpoint::load(&plan, tmp.path()).unwrap().is_none());
    }
}
