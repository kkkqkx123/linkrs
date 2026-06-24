use graphdb_core::core::{DataType, Value};

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
        let step = MigrationStep::DropColumn { name: "email".into() };
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
        assert_eq!(add.reverse(), Some(MigrationStep::DropColumn { name: "x".into() }));

        assert_eq!(MigrationStep::DropColumn { name: "x".into() }.reverse(), None);

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
        let plan = MigrationPlan {
            space: "test".into(),
            label: "User".into(),
            is_edge: false,
            from_version: 1,
            to_version: 2,
            steps: vec![],
            estimated_rows: 0,
            overall_safety: SafetyLevel::Safe,
            rollback_plan: None,
        };
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
        };
        assert!(report.print_summary().contains("SUCCESS"));
        assert!(report.print_summary().contains("100"));

        let failed = MigrationReport {
            success: false,
            steps_completed: 1,
            rows_migrated: 0,
            errors: vec!["Error converting value".into()],
        };
        assert!(failed.print_summary().contains("FAILED"));
        assert!(failed.print_summary().contains("Error converting value"));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq)]
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
        }
    }

    pub fn is_data_modifying(&self) -> bool {
        matches!(
            self,
            MigrationStep::DropColumn { .. }
                | MigrationStep::RenameColumn { .. }
                | MigrationStep::ConvertType { .. }
                | MigrationStep::SetDefault { .. }
                | MigrationStep::ChangeNullability { .. }
        )
    }

    pub fn description(&self) -> String {
        match self {
            MigrationStep::AddColumn { name, data_type: dt, nullable, .. } => {
                format!("Add column '{}' of type {:?} (nullable: {})", name, dt, nullable)
            }
            MigrationStep::DropColumn { name } => {
                format!("Drop column '{}' - existing data will be lost", name)
            }
            MigrationStep::RenameColumn { old_name, new_name } => {
                format!("Rename column '{}' to '{}'", old_name, new_name)
            }
            MigrationStep::ConvertType { name, from_type, to_type } => {
                format!("Convert column '{}' from {:?} to {:?}", name, from_type, to_type)
            }
            MigrationStep::SetDefault { name, default_value } => {
                format!("Set default value for column '{}' to {:?}", name, default_value)
            }
            MigrationStep::ChangeNullability { name, was_nullable, now_nullable } => {
                format!(
                    "Change column '{}' nullability from {} to {}",
                    name, was_nullable, now_nullable
                )
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
            MigrationStep::ConvertType { name, from_type, to_type } => {
                Some(MigrationStep::ConvertType {
                    name: name.clone(),
                    from_type: to_type.clone(),
                    to_type: from_type.clone(),
                })
            }
            MigrationStep::SetDefault { name, default_value } => {
                Some(MigrationStep::SetDefault {
                    name: name.clone(),
                    default_value: default_value.clone(),
                })
            }
            MigrationStep::ChangeNullability { name, was_nullable, now_nullable } => {
                Some(MigrationStep::ChangeNullability {
                    name: name.clone(),
                    was_nullable: *now_nullable,
                    now_nullable: *was_nullable,
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct MigrationPlan {
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub from_version: u64,
    pub to_version: u64,
    pub steps: Vec<MigrationStep>,
    pub estimated_rows: u64,
    pub overall_safety: SafetyLevel,
    pub rollback_plan: Option<Box<MigrationPlan>>,
}

impl MigrationPlan {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn print_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Migration Plan: {} v{} → v{} ({} row(s))\n",
            self.label, self.from_version, self.to_version, self.estimated_rows
        ));
        out.push_str(&format!("Safety: {} ({:?})\n", self.overall_safety.label(), self.overall_safety));
        out.push_str(&format!("Steps: {}\n", self.steps.len()));
        for (i, step) in self.steps.iter().enumerate() {
            out.push_str(&format!(
                "  {}. [{:?}] {}\n",
                i + 1,
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

#[derive(Debug, Clone)]
pub struct MigrationReport {
    pub success: bool,
    pub steps_completed: usize,
    pub rows_migrated: u64,
    pub errors: Vec<String>,
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
