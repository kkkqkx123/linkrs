use clap::{Parser, Subcommand};
use graphdb_migration::{
    generate_edge_plan, generate_edge_plan_with_expand, generate_vertex_plan,
    generate_vertex_plan_with_expand, MigrationEvent, MigrationEventListener, MigrationFileLock,
};
use graphdb_storage::edge::EdgeStore;
use graphdb_storage::{GraphStorage, StorageReader};
use std::path::PathBuf;

struct CliEventListener;

impl MigrationEventListener for CliEventListener {
    fn on_event(&self, event: MigrationEvent) {
        match event {
            MigrationEvent::Started { plan } => {
                println!("[migration] Starting plan: {}", plan.print_summary());
            }
            MigrationEvent::StepStarted { step_idx } => {
                println!("[migration] Step {} started", step_idx);
            }
            MigrationEvent::StepCompleted { step_idx, rows } => {
                println!("[migration] Step {} completed ({} rows)", step_idx, rows);
            }
            MigrationEvent::Completed { report } => {
                println!("[migration] Completed: {}", report.print_summary());
            }
            MigrationEvent::Failed { error } => {
                eprintln!("[migration] Failed: {}", error);
            }
            MigrationEvent::RolledBack { report } => {
                println!("[migration] Rolled back: {}", report.print_summary());
            }
        }
    }
}

#[derive(Parser)]
#[command(name = "graphdb-migration", about = "GraphDB Migration CLI")]
struct Cli {
    #[arg(long, default_value = "./data", global = true)]
    db_path: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate and show migration plan
    Plan {
        #[arg(long)]
        space: String,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "false")]
        is_edge: bool,
        #[arg(long)]
        from: u64,
        #[arg(long)]
        to: u64,
        #[arg(long, default_value = "false")]
        expand_contract: bool,
    },
    /// Execute migration up (from -> to)
    Up {
        #[arg(long)]
        space: String,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "false")]
        is_edge: bool,
        #[arg(long)]
        from: u64,
        #[arg(long)]
        to: u64,
        #[arg(long, default_value = "false")]
        expand_contract: bool,
    },
    /// Rollback migration down
    Down {
        #[arg(long)]
        space: String,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "false")]
        is_edge: bool,
        #[arg(long)]
        plan_json: String,
    },
    /// Show migration status
    Status {
        #[arg(long)]
        space: String,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "false")]
        is_edge: bool,
    },
    /// Dry-run migration without committing
    DryRun {
        #[arg(long)]
        space: String,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "false")]
        is_edge: bool,
        #[arg(long)]
        from: u64,
        #[arg(long)]
        to: u64,
        #[arg(long, default_value = "false")]
        expand_contract: bool,
    },
    /// Show migration history
    History {
        #[arg(long)]
        space: String,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "false")]
        is_edge: bool,
    },
    /// Diagnose one edge-table WAL directory without touching it.
    ///
    /// Read-only: reports the last valid entry boundary and the torn tail
    /// the load path would reject. Run this first after a torn-tail load
    /// failure; repair stays a separate explicit step.
    EdgeWalDiagnose {
        /// Edge-table checkpoint directory holding `edge_wal.bin`.
        #[arg(long)]
        path: PathBuf,
    },
    /// Truncate the torn tail of one edge-table WAL directory.
    ///
    /// Explicit offline repair: truncates the log at the last valid entry
    /// and prints the salvaged prefix plus the discarded tail. Only run
    /// while no writer holds the table. The load path never truncates on
    /// its own and stays fail-closed without this command.
    EdgeWalRepair {
        /// Edge-table checkpoint directory holding `edge_wal.bin`.
        #[arg(long)]
        path: PathBuf,
    },
}

fn open_storage(path: &std::path::Path) -> anyhow::Result<GraphStorage> {
    if path.exists() {
        Ok(GraphStorage::open(path.to_path_buf())?)
    } else {
        Ok(GraphStorage::new()?)
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Plan {
            space,
            label,
            is_edge,
            from,
            to,
            expand_contract,
        } => {
            let storage = open_storage(&cli.db_path)?;
            let plan = if is_edge {
                if expand_contract {
                    generate_edge_plan_with_expand(&storage, &space, &label, from, to, true)?
                } else {
                    generate_edge_plan(&storage, &space, &label, from, to)?
                }
            } else {
                if expand_contract {
                    generate_vertex_plan_with_expand(&storage, &space, &label, from, to, true)?
                } else {
                    generate_vertex_plan(&storage, &space, &label, from, to)?
                }
            };
            println!("{}", plan.print_summary());
            println!("Plan JSON: {}", serde_json::to_string_pretty(&plan)?);
            println!("Plan Hash: {}", plan.plan_hash);
        }
        Commands::Up {
            space,
            label,
            is_edge,
            from,
            to,
            expand_contract,
        } => {
            let mut storage = open_storage(&cli.db_path)?;
            let plan = if is_edge {
                if expand_contract {
                    generate_edge_plan_with_expand(&storage, &space, &label, from, to, true)?
                } else {
                    generate_edge_plan(&storage, &space, &label, from, to)?
                }
            } else {
                if expand_contract {
                    generate_vertex_plan_with_expand(&storage, &space, &label, from, to, true)?
                } else {
                    generate_vertex_plan(&storage, &space, &label, from, to)?
                }
            };
            println!("Executing plan: {}", plan.print_summary());
            let lock_path = cli.db_path.join("migration.lock");
            let _lock = MigrationFileLock::try_acquire(&lock_path)?;
            let listener = CliEventListener;
            let report = graphdb_migration::execute_migration_plan_with_progress(
                &mut storage,
                &plan,
                &graphdb_migration::NoopProgress,
                Some(&listener),
            )?;
            if !report.success {
                std::process::exit(1);
            }
        }
        Commands::Down {
            space: _,
            label: _,
            is_edge: _,
            plan_json,
        } => {
            let mut storage = open_storage(&cli.db_path)?;
            let plan: graphdb_migration::MigrationPlan = serde_json::from_str(&plan_json)?;
            let lock_path = cli.db_path.join("migration.lock");
            let _lock = MigrationFileLock::try_acquire(&lock_path)?;
            let report = graphdb_migration::rollback_migration(&mut storage, &plan)?;
            println!("{}", report.print_summary());
        }
        Commands::Status {
            space,
            label,
            is_edge,
        } => {
            let storage = open_storage(&cli.db_path)?;
            let history = storage
                .list_migration_history(&space, &label, is_edge)
                .unwrap_or_default();
            let versions = storage
                .get_applied_versions(&space, &label, is_edge)
                .unwrap_or_default();
            println!("Space: {} Label: {} is_edge: {}", space, label, is_edge);
            println!("Applied versions: {:?}", versions);
            println!("History records: {}", history.len());
            for rec in &history {
                println!(
                    "  to_version={} hash={} status={} rows={} at={}",
                    rec.to_version, rec.plan_hash, rec.status, rec.rows_migrated, rec.applied_at
                );
            }
            // Also show version history if available
            let vh = if is_edge {
                storage.get_edge_version_history(&space, &label)
            } else {
                storage.get_vertex_version_history(&space, &label)
            };
            if let Ok(Some(h)) = vh {
                println!("Version history: {:?}", h.get_versions());
            }
        }
        Commands::DryRun {
            space,
            label,
            is_edge,
            from,
            to,
            expand_contract,
        } => {
            let mut storage = open_storage(&cli.db_path)?;
            let mut plan = if is_edge {
                if expand_contract {
                    generate_edge_plan_with_expand(&storage, &space, &label, from, to, true)?
                } else {
                    generate_edge_plan(&storage, &space, &label, from, to)?
                }
            } else {
                if expand_contract {
                    generate_vertex_plan_with_expand(&storage, &space, &label, from, to, true)?
                } else {
                    generate_vertex_plan(&storage, &space, &label, from, to)?
                }
            };
            plan.dry_run = true;
            println!("Dry-run plan: {}", plan.print_summary());
            let report = graphdb_migration::execute_migration_plan(&mut storage, &plan)?;
            println!("Dry-run report (preview only): {}", report.print_summary());
            println!("Dry-run does not modify storage data.");
        }
        Commands::History {
            space,
            label,
            is_edge,
        } => {
            let storage = open_storage(&cli.db_path)?;
            let history = storage
                .list_migration_history(&space, &label, is_edge)
                .unwrap_or_default();
            if history.is_empty() {
                println!(
                    "No migration history for {}/{} (is_edge={})",
                    space, label, is_edge
                );
            } else {
                for rec in history {
                    println!(
                        "id={} space={} label={} is_edge={} {}->{} hash={} safety={} rows={} status={} applied_at={}",
                        rec.id, rec.space, rec.label, rec.is_edge, rec.from_version, rec.to_version, rec.plan_hash, rec.safety_level, rec.rows_migrated, rec.status, rec.applied_at
                    );
                }
            }
        }
        Commands::EdgeWalDiagnose { path } => {
            let diagnosis = EdgeStore::diagnose_edge_wal_at(&path)?;
            println!(
                "edge WAL diagnosis for {}: file_bytes={} valid_bytes={} valid_ops={} torn_bytes={} clean={}",
                path.display(),
                diagnosis.file_bytes,
                diagnosis.valid_bytes,
                diagnosis.valid_ops,
                diagnosis.torn_bytes,
                diagnosis.is_clean(),
            );
            if let Some(cause) = diagnosis.error {
                println!("torn cause: {}", cause);
                println!("load stays fail-closed; repair explicitly with edge-wal-repair");
            }
        }
        Commands::EdgeWalRepair { path } => {
            let report = EdgeStore::repair_edge_wal_at_reported(&path)?;
            println!(
                "edge WAL repair for {}: file_bytes={} valid_bytes={} valid_ops={} torn_bytes={} repaired={}",
                path.display(),
                report.file_bytes,
                report.valid_bytes,
                report.valid_ops,
                report.torn_bytes,
                report.repaired,
            );
        }
    }
    Ok(())
}
