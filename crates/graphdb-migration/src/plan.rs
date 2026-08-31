use graphdb_core::{DataType, Value};
use serde::{Deserialize, Serialize};

use crate::generator::MigrationError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safety_level_ordering() {
        assert!(SafetyLevel::Safe.is_safe());
        assert!(!SafetyLevel::Warning.is_safe());
        assert!(!SafetyLevel::Dangerous.is_safe());
        assert_eq!(SafetyLevel::Safe.label(), "SAFE");
        assert_eq!(SafetyLevel::Warning.label(), "WARNING");
        assert_eq!(SafetyLevel::Dangerous.label(), "DANGEROUS");
    }

    #[test]
    fn test_add_column_safety() {
        let step = MigrationStep::AddColumn {
            name: "email".into(),
            data_type: DataType::String,
            nullable: true,
            default_value: None,
        };
        assert_eq!(step.safety_level(), SafetyLevel::Safe);
        assert!(!step.is_data_modifying());
    }

    #[test]
    fn test_drop_column_safety() {
        let step = MigrationStep::DropColumn {
            name: "email".into(),
        };
        assert_eq!(step.safety_level(), SafetyLevel::Dangerous);
        assert!(step.is_data_modifying());
    }

    #[test]
    fn test_rename_column_safety() {
        let step = MigrationStep::RenameColumn {
            old_name: "old".into(),
            new_name: "new".into(),
        };
        assert_eq!(step.safety_level(), SafetyLevel::Warning);
        assert!(step.is_data_modifying());
    }

    #[test]
    fn test_convert_type_safety() {
        let step = MigrationStep::ConvertType {
            name: "age".into(),
            from_type: DataType::Int,
            to_type: DataType::BigInt,
        };
        assert_eq!(step.safety_level(), SafetyLevel::Warning);
        assert!(step.is_data_modifying());
    }

    #[test]
    fn test_step_reverse() {
        let add = MigrationStep::AddColumn {
            name: "x".into(),
            data_type: DataType::String,
            nullable: true,
            default_value: None,
        };
        assert_eq!(
            add.reverse(),
            Some(MigrationStep::DropColumn { name: "x".into() })
        );

        assert_eq!(
            MigrationStep::DropColumn { name: "x".into() }.reverse(),
            None
        );

        let rename = MigrationStep::RenameColumn {
            old_name: "a".into(),
            new_name: "b".into(),
        };
        assert_eq!(
            rename.reverse(),
            Some(MigrationStep::RenameColumn {
                old_name: "b".into(),
                new_name: "a".into(),
            })
        );

        let convert = MigrationStep::ConvertType {
            name: "c".into(),
            from_type: DataType::Int,
            to_type: DataType::BigInt,
        };
        assert_eq!(
            convert.reverse(),
            Some(MigrationStep::ConvertType {
                name: "c".into(),
                from_type: DataType::BigInt,
                to_type: DataType::Int,
            })
        );
    }

    #[test]
    fn test_step_description() {
        let step = MigrationStep::AddColumn {
            name: "email".into(),
            data_type: DataType::String,
            nullable: false,
            default_value: None,
        };
        assert!(step.description().contains("email"));
        assert!(step.description().contains("String"));

        let drop = MigrationStep::DropColumn { name: "x".into() };
        assert!(drop.description().contains("x"));
        assert!(drop.description().contains("lost"));
    }

    #[test]
    fn test_empty_migration_plan() {
        let plan = MigrationPlan::new(
            MigrationTarget {
                space: "test".into(),
                label: "User".into(),
                is_edge: false,
            },
            VersionRange { from: 1, to: 2 },
            vec![],
            0,
            SafetyLevel::Safe,
            None,
        );
        assert!(plan.is_empty());
        assert!(plan.print_summary().contains("SAFE"));
    }

    #[test]
    fn test_migration_report() {
        let report = MigrationReport {
            success: true,
            steps_completed: 3,
            rows_migrated: 100,
            errors: vec![],
            completed_step_indices: vec![],
        };
        assert!(report.print_summary().contains("SUCCESS"));
        assert!(report.print_summary().contains("100"));

        let failed = MigrationReport {
            success: false,
            steps_completed: 1,
            rows_migrated: 0,
            errors: vec!["Error converting value".into()],
            completed_step_indices: vec![],
        };
        assert!(failed.print_summary().contains("FAILED"));
        assert!(failed.print_summary().contains("Error converting value"));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafetyLevel {
    Safe,
    Warning,
    Dangerous,
}

impl SafetyLevel {
    pub fn is_safe(&self) -> bool {
        matches!(self, SafetyLevel::Safe)
    }

    pub fn label(&self) -> &'static str {
        match self {
            SafetyLevel::Safe => "SAFE",
            SafetyLevel::Warning => "WARNING",
            SafetyLevel::Dangerous => "DANGEROUS",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MigrationStep {
    AddColumn {
        name: String,
        data_type: DataType,
        nullable: bool,
        default_value: Option<Value>,
    },
    DropColumn {
        name: String,
    },
    RenameColumn {
        old_name: String,
        new_name: String,
    },
    ConvertType {
        name: String,
        from_type: DataType,
        to_type: DataType,
    },
    SetDefault {
        name: String,
        default_value: Option<Value>,
    },
    ChangeNullability {
        name: String,
        was_nullable: bool,
        now_nullable: bool,
    },
    CreateLabel {
        label_name: String,
    },
    DropLabel {
        label_name: String,
    },
    CreateEdgeType {
        edge_type_name: String,
    },
    DropEdgeType {
        edge_type_name: String,
    },
}

impl MigrationStep {
    pub fn safety_level(&self) -> SafetyLevel {
        match self {
            MigrationStep::AddColumn { .. } => SafetyLevel::Safe,
            MigrationStep::DropColumn { .. } => SafetyLevel::Dangerous,
            MigrationStep::RenameColumn { .. } => SafetyLevel::Warning,
            MigrationStep::ConvertType { .. } => SafetyLevel::Warning,
            MigrationStep::SetDefault { .. } => SafetyLevel::Safe,
            MigrationStep::ChangeNullability { .. } => SafetyLevel::Warning,
            MigrationStep::CreateLabel { .. } => SafetyLevel::Safe,
            MigrationStep::DropLabel { .. } => SafetyLevel::Dangerous,
            MigrationStep::CreateEdgeType { .. } => SafetyLevel::Safe,
            MigrationStep::DropEdgeType { .. } => SafetyLevel::Dangerous,
        }
    }

    pub fn is_data_modifying(&self) -> bool {
        matches!(
            self,
            MigrationStep::DropColumn { .. }
                | MigrationStep::RenameColumn { .. }
                | MigrationStep::ConvertType { .. }
                | MigrationStep::SetDefault { .. }
        )
    }

    pub fn is_schema_modifying(&self) -> bool {
        matches!(
            self,
            MigrationStep::CreateLabel { .. }
                | MigrationStep::DropLabel { .. }
                | MigrationStep::CreateEdgeType { .. }
                | MigrationStep::DropEdgeType { .. }
        )
    }

    pub fn description(&self) -> String {
        match self {
            MigrationStep::AddColumn {
                name,
                data_type: dt,
                nullable,
                ..
            } => {
                format!(
                    "Add column '{}' of type {:?} (nullable: {})",
                    name, dt, nullable
                )
            }
            MigrationStep::DropColumn { name } => {
                format!("Drop column '{}' - existing data will be lost", name)
            }
            MigrationStep::RenameColumn { old_name, new_name } => {
                format!("Rename column '{}' to '{}'", old_name, new_name)
            }
            MigrationStep::ConvertType {
                name,
                from_type,
                to_type,
            } => {
                format!(
                    "Convert column '{}' from {:?} to {:?}",
                    name, from_type, to_type
                )
            }
            MigrationStep::SetDefault {
                name,
                default_value,
            } => {
                format!(
                    "Set default value for column '{}' to {:?}",
                    name, default_value
                )
            }
            MigrationStep::ChangeNullability {
                name,
                was_nullable,
                now_nullable,
            } => {
                format!(
                    "Change column '{}' nullability from {} to {}",
                    name, was_nullable, now_nullable
                )
            }
            MigrationStep::CreateLabel { label_name } => {
                format!("Create label '{}'", label_name)
            }
            MigrationStep::DropLabel { label_name } => {
                format!("Drop label '{}' - all data will be lost", label_name)
            }
            MigrationStep::CreateEdgeType { edge_type_name } => {
                format!("Create edge type '{}'", edge_type_name)
            }
            MigrationStep::DropEdgeType { edge_type_name } => {
                format!("Drop edge type '{}' - all data will be lost", edge_type_name)
            }
        }
    }

    pub fn reverse(&self) -> Option<MigrationStep> {
        match self {
            MigrationStep::AddColumn { name, .. } => {
                Some(MigrationStep::DropColumn { name: name.clone() })
            }
            MigrationStep::DropColumn { name: _ } => None,
            MigrationStep::RenameColumn { old_name, new_name } => {
                Some(MigrationStep::RenameColumn {
                    old_name: new_name.clone(),
                    new_name: old_name.clone(),
                })
            }
            MigrationStep::ConvertType {
                name,
                from_type,
                to_type,
            } => Some(MigrationStep::ConvertType {
                name: name.clone(),
                from_type: to_type.clone(),
                to_type: from_type.clone(),
            }),
            MigrationStep::SetDefault {
                name,
                default_value,
            } => Some(MigrationStep::SetDefault {
                name: name.clone(),
                default_value: default_value.clone(),
            }),
            MigrationStep::ChangeNullability {
                name,
                was_nullable,
                now_nullable,
            } => Some(MigrationStep::ChangeNullability {
                name: name.clone(),
                was_nullable: *now_nullable,
                now_nullable: *was_nullable,
            }),
            MigrationStep::CreateLabel { label_name } => {
                Some(MigrationStep::DropLabel { label_name: label_name.clone() })
            }
            MigrationStep::DropLabel { .. } => None,
            MigrationStep::CreateEdgeType { edge_type_name } => Some(MigrationStep::DropEdgeType {
                edge_type_name: edge_type_name.clone(),
            }),
            MigrationStep::DropEdgeType { .. } => None,
        }
    }
}

const DEFAULT_BATCH_SIZE: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationTarget {
    pub space: String,
    pub label: String,
    pub is_edge: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionRange {
    pub from: u64,
    pub to: u64,
}

fn compute_steps_hash(steps: &[MigrationStep]) -> String {
    use sha2::{Digest, Sha256};
    let json = serde_json::to_string(steps).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPlan {
    pub target: MigrationTarget,
    pub version_range: VersionRange,
    pub steps: Vec<MigrationStep>,
    pub estimated_rows: u64,
    pub overall_safety: SafetyLevel,
    pub rollback_plan: Option<Box<MigrationPlan>>,
    pub batch_size: usize,
    pub completed_steps: Vec<usize>,
    #[serde(default)]
    pub plan_hash: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub created_at: Option<u64>,
    #[serde(default)]
    pub expand_contract: Option<bool>,
}

impl MigrationPlan {
    pub fn new(
        target: MigrationTarget,
        version_range: VersionRange,
        steps: Vec<MigrationStep>,
        estimated_rows: u64,
        overall_safety: SafetyLevel,
        rollback_plan: Option<Box<MigrationPlan>>,
    ) -> Self {
        let hash = compute_steps_hash(&steps);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            target,
            version_range,
            steps,
            estimated_rows,
            overall_safety,
            rollback_plan,
            batch_size: DEFAULT_BATCH_SIZE,
            completed_steps: Vec::new(),
            plan_hash: hash,
            dry_run: false,
            created_at: Some(now),
            expand_contract: None,
        }
    }

    pub fn compute_hash(&self) -> String {
        compute_steps_hash(&self.steps)
    }

    pub fn refresh_hash(&mut self) {
        self.plan_hash = compute_steps_hash(&self.steps);
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn remaining_steps(&self) -> Vec<usize> {
        (0..self.steps.len())
            .filter(|i| !self.completed_steps.contains(i))
            .collect()
    }

    pub fn print_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Migration Plan: {} v{} → v{} ({} row(s))\n",
            self.target.label, self.version_range.from, self.version_range.to, self.estimated_rows
        ));
        out.push_str(&format!(
            "Safety: {} ({:?})\n",
            self.overall_safety.label(),
            self.overall_safety
        ));
        out.push_str(&format!("Steps: {}\n", self.steps.len()));
        for (i, step) in self.steps.iter().enumerate() {
            let prefix = if self.completed_steps.contains(&i) {
                "[DONE] "
            } else {
                ""
            };
            out.push_str(&format!(
                "  {}. {}[{:?}] {}\n",
                i + 1,
                prefix,
                step.safety_level(),
                step.description()
            ));
        }
        if self.rollback_plan.is_some() {
            out.push_str("Rollback plan: Available\n");
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    pub success: bool,
    pub steps_completed: usize,
    pub rows_migrated: u64,
    pub errors: Vec<String>,
    pub completed_step_indices: Vec<usize>,
}

impl MigrationReport {
    pub fn print_summary(&self) -> String {
        let status = if self.success { "SUCCESS" } else { "FAILED" };
        let mut out = format!(
            "Migration {}: {} step(s) completed, {} row(s) migrated, {} error(s)",
            status,
            self.steps_completed,
            self.rows_migrated,
            self.errors.len()
        );
        if !self.errors.is_empty() {
            out.push_str("\nErrors:");
            for (i, err) in self.errors.iter().enumerate() {
                out.push_str(&format!("\n  {}. {}", i + 1, err));
            }
        }
        out
    }
}

pub fn checkpoint_now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepResult {
    Success,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationCheckpoint {
    pub completed_step_index: usize,
    pub rows_migrated_before: u64,
    pub rows_migrated_after: u64,
    pub timestamp: u64,
    pub step_result: StepResult,
}

impl MigrationCheckpoint {
    pub fn save(
        &self,
        plan: &MigrationPlan,
        storage_path: &std::path::Path,
    ) -> Result<(), MigrationError> {
        let checkpoint_file = storage_path.join(format!(
            "checkpoint_{}_{}_{}.json",
            plan.target.space, plan.target.label, plan.plan_hash
        ));
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| MigrationError::Plan(format!("serialize checkpoint: {e}")))?;
        std::fs::write(&checkpoint_file, content)
            .map_err(|e| MigrationError::Plan(format!("write checkpoint: {e}")))?;
        Ok(())
    }

    pub fn load(
        plan: &MigrationPlan,
        storage_path: &std::path::Path,
    ) -> Result<Option<Self>, MigrationError> {
        let checkpoint_file = storage_path.join(format!(
            "checkpoint_{}_{}_{}.json",
            plan.target.space, plan.target.label, plan.plan_hash
        ));
        if !checkpoint_file.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&checkpoint_file)
            .map_err(|e| MigrationError::Plan(format!("read checkpoint: {e}")))?;
        let checkpoint: Self = serde_json::from_str(&content)
            .map_err(|e| MigrationError::Plan(format!("parse checkpoint: {e}")))?;
        Ok(Some(checkpoint))
    }

    pub fn cleanup(
        plan: &MigrationPlan,
        storage_path: &std::path::Path,
    ) -> Result<(), MigrationError> {
        let checkpoint_file = storage_path.join(format!(
            "checkpoint_{}_{}_{}.json",
            plan.target.space, plan.target.label, plan.plan_hash
        ));
        if checkpoint_file.exists() {
            std::fs::remove_file(&checkpoint_file)
                .map_err(|e| MigrationError::Plan(format!("remove checkpoint: {e}")))?;
        }
        Ok(())
    }
}
