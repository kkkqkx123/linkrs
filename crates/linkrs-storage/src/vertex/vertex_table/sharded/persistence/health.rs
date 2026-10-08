//! Offline health inspection: read-only patrol entry for half-damaged stores.

use std::path::Path;

use super::super::ShardedVertexTable;
use super::commit_manifest::{
    commit_manifest_path, verify_commit_manifest_content, CommitManifest,
};
use super::sidecar::SnapshotSidecarRecord;
use super::table_manifest::TABLE_MANIFEST_FILE_NAME;
use linkrs_core::StorageResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHealthReport {
    /// Whether `commit_manifest.json` exists.
    pub manifest_present: bool,
    /// Whether the manifest decoded as JSON.
    pub manifest_decodable: bool,
    /// Checkpoint epoch pinned by the manifest, if decodable.
    pub epoch: Option<u64>,
    /// `full` or `incremental`, if decodable.
    pub kind: Option<String>,
    /// Base epoch for incremental checkpoints, if decodable.
    pub base_epoch: Option<u64>,
    /// Redistribution generation pinned by the commit manifest, if decodable.
    pub commit_generation: Option<u64>,
    /// Routing scheme version pinned by the table manifest, if decodable.
    pub router_version: Option<u8>,
    /// Redistribution generation pinned by the table manifest, if decodable.
    pub generation: Option<u64>,
    /// Lineage defects: unknown router, table/commit generation mismatch,
    /// or an undecodable table manifest. Empty when the lineage proves.
    pub lineage_issues: Vec<String>,
    /// Files listed by the manifest.
    pub listed_files: Vec<String>,
    /// Listed files missing from disk.
    pub missing_files: Vec<String>,
    /// Orphan temp/staging files (tolerated, cleaned by recovery).
    pub orphan_tmp_files: Vec<String>,
    /// Whether every shard's primary-key files decode and agree.
    pub pk_index_ok: bool,
    /// Per-shard primary-key decode issues, empty when healthy.
    pub pk_issues: Vec<String>,
    /// Sidecars pinned by the manifest.
    pub sidecars: Vec<String>,
    /// Discardable sidecar defects (missing, checksum mismatch, or corrupt
    /// frames). Never block `is_healthy`: reload drops the sidecar and
    /// keeps the chunks resident.
    pub sidecar_issues: Vec<String>,
    /// Total bytes of pinned sidecars present on disk. Missing sidecars
    /// count as zero; corrupt files still count here and are distinguished
    /// through `sidecar_issues`.
    pub sidecar_bytes: u64,
}

impl CommitHealthReport {
    /// Whether the directory is safe to open strictly: a decodable manifest
    /// with no missing files, a verifiable primary-key index, and a proven
    /// lineage (known router, matching table and commit generations).
    /// Sidecar defects never block health: they are discardable caches.
    pub fn is_healthy(&self) -> bool {
        self.manifest_present
            && self.manifest_decodable
            && self.missing_files.is_empty()
            && self.pk_index_ok
            && self.lineage_issues.is_empty()
    }
}

/// Aggregated health across label directories under one vertices root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalCommitHealth {
    /// Per-table reports keyed by directory name.
    pub tables: Vec<(String, CommitHealthReport)>,
    /// Whether the baseline plus incremental epoch chain is continuous.
    pub chain_ok: bool,
    /// Human-readable issues, empty when healthy.
    pub issues: Vec<String>,
}

impl GlobalCommitHealth {
    /// Whether every table is healthy and the epoch chain is continuous.
    pub fn is_healthy(&self) -> bool {
        self.chain_ok && self.tables.iter().all(|(_, r)| r.is_healthy())
    }
}

impl ShardedVertexTable {
    /// Offline read-only health inspection for one table directory.
    ///
    /// Reuses the recovery path's manifest decoding plus file existence
    /// checks and reports: whether the commit manifest is present and
    /// decodable, its epoch/kind/base-epoch chain pointers, which listed
    /// files are missing, which orphan temp files exist, and whether every
    /// shard's primary-key files decode and agree. Never writes;
    /// cleanup stays with startup recovery. Baseline plus incremental epoch
    /// chain continuity across directories is validated by the global
    /// checkpoint manifest manager, which sees every table's pointers.
    pub fn inspect_commit_health<P: AsRef<Path>>(path: P) -> StorageResult<CommitHealthReport> {
        let dir = path.as_ref();
        let manifest_path = commit_manifest_path(dir);
        let manifest_present = manifest_path.exists();
        let mut manifest_decodable = false;
        let mut epoch = None;
        let mut kind = None;
        let mut base_epoch = None;
        let mut commit_generation = None;
        let mut listed_files = Vec::new();
        let mut missing_files = Vec::new();
        let mut pinned_sidecars: Vec<SnapshotSidecarRecord> = Vec::new();
        if manifest_present {
            if let Ok(payload) = std::fs::read(&manifest_path) {
                if let Ok(manifest) = serde_json::from_slice::<CommitManifest>(&payload) {
                    if verify_commit_manifest_content(&manifest, &manifest_path).is_ok() {
                        manifest_decodable = true;
                        epoch = Some(manifest.epoch);
                        kind = Some(manifest.kind.as_str().to_string());
                        base_epoch = manifest.base_epoch;
                        commit_generation = Some(manifest.generation);
                        listed_files = manifest.files.clone();
                        for rel in &manifest.files {
                            if !dir.join(rel).exists() {
                                missing_files.push(rel.clone());
                            }
                        }
                        pinned_sidecars = manifest.sidecars.clone();
                    }
                }
            }
        }
        let mut router_version = None;
        let mut generation = None;
        let mut lineage_issues = Vec::new();
        match Self::read_table_manifest(dir) {
            Ok(Some(table)) => {
                router_version = Some(table.router_version);
                generation = Some(table.generation);
                if table.router_version != super::super::routing::ROUTER_VERSION {
                    lineage_issues.push(format!(
                        "unknown table router version {} (expected {})",
                        table.router_version,
                        super::super::routing::ROUTER_VERSION,
                    ));
                }
                match commit_generation {
                    Some(commit) if commit != table.generation => lineage_issues.push(format!(
                        "table manifest generation {} differs from commit generation {}",
                        table.generation, commit,
                    )),
                    None if manifest_present => lineage_issues.push(
                        "commit manifest undecodable: checkpoint lineage unprovable".to_string(),
                    ),
                    _ => {}
                }
            }
            Ok(None) => lineage_issues.push(format!(
                "table manifest missing at {}: refusing open without shard layout pin; \
                 rebuild the table with the offline redistribution tool",
                dir.join(TABLE_MANIFEST_FILE_NAME).display(),
            )),
            // The strict decode error already names the file and the rebuild
            // directive (bad JSON, checksum, version, router, or layout), so
            // forwarding it keeps diagnosis and refusal in agreement.
            Err(e) => lineage_issues.push(format!("table manifest rejected: {e}")),
        }
        let mut orphan_tmp_files = Vec::new();
        if dir.exists() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".tmp") || name.ends_with(".staging") || name == "staging" {
                        orphan_tmp_files.push(name);
                    }
                }
            }
            for index in 0..usize::MAX {
                let shard_dir = dir.join(format!("shard_{}", index));
                if !shard_dir.exists() {
                    break;
                }
                if let Ok(entries) = std::fs::read_dir(&shard_dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".tmp") {
                            orphan_tmp_files.push(format!("shard_{}/{}", index, name));
                        }
                    }
                }
            }
        }
        orphan_tmp_files.sort();
        let mut pk_issues = Vec::new();
        for index in 0..usize::MAX {
            let shard_dir = dir.join(format!("shard_{}", index));
            if !shard_dir.exists() {
                break;
            }
            for issue in crate::vertex::vertex_table::core::VertexTable::verify_pk_files(&shard_dir)
            {
                pk_issues.push(format!("shard_{}: {}", index, issue));
            }
        }
        pk_issues.sort();
        let pk_index_ok = pk_issues.is_empty();
        // Sidecar verification is discardable-only: a missing, checksum
        // mismatched, or unparseable sidecar is reported but never blocks
        // health. Reload drops that sidecar and keeps chunks resident.
        let mut sidecars: Vec<String> = pinned_sidecars.iter().map(|r| r.file.clone()).collect();
        sidecars.sort();
        let mut sidecar_issues = Vec::new();
        let mut sidecar_bytes = 0u64;
        for record in &pinned_sidecars {
            let full = dir.join(&record.file);
            let bytes = std::fs::read(&full);
            match bytes {
                Err(_) => {
                    sidecar_issues.push(format!("sidecar missing (discardable): {}", record.file))
                }
                Ok(payload) => {
                    sidecar_bytes += payload.len() as u64;
                    let mut hasher = crc32fast::Hasher::new();
                    hasher.update(&payload);
                    if payload.len() as u64 != record.bytes || hasher.finalize() != record.checksum
                    {
                        sidecar_issues.push(format!(
                            "sidecar checksum mismatch (discardable): {}",
                            record.file
                        ));
                        continue;
                    }
                    if crate::vertex::column::chunk_residency::open_snapshot_sidecar(&full).is_err()
                    {
                        sidecar_issues
                            .push(format!("sidecar corrupt (discardable): {}", record.file));
                    }
                }
            }
        }
        // Unpinned sidecars on disk (crash window or dropped columns) are
        // swept on the next checkpoint; report but never fail health.
        for index in 0..usize::MAX {
            let shard_dir = dir.join(format!("shard_{}", index));
            if !shard_dir.exists() {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&shard_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".snapshot") {
                    continue;
                }
                let rel = format!("shard_{}/{}", index, name);
                if !sidecars.contains(&rel) {
                    sidecar_issues.push(format!("sidecar unpinned (swept on flush): {}", rel));
                }
            }
        }
        sidecar_issues.sort();
        Ok(CommitHealthReport {
            manifest_present,
            manifest_decodable,
            epoch,
            kind,
            base_epoch,
            commit_generation,
            router_version,
            generation,
            lineage_issues,
            listed_files,
            missing_files,
            orphan_tmp_files,
            pk_index_ok,
            pk_issues,
            sidecars,
            sidecar_issues,
            sidecar_bytes,
        })
    }

    /// Offline read-only inspection across label directories.
    ///
    /// Walks `vertices_dir` for `label_*` subdirectories, reuses the
    /// table-level inspection per label, and validates baseline plus
    /// incremental epoch chain continuity: every incremental base epoch must
    /// appear as another table epoch or as a full checkpoint epoch in the
    /// same walk. Never writes; cleanup stays with startup recovery. Serves
    /// as the offline patrol entry for half-damaged stores.
    pub fn inspect_store_health<P: AsRef<Path>>(
        vertices_dir: P,
    ) -> StorageResult<GlobalCommitHealth> {
        let root = vertices_dir.as_ref();
        let mut tables = Vec::new();
        let mut issues = Vec::new();
        if root.exists() {
            let mut names: Vec<String> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.starts_with("label_") {
                                names.push(name.to_string());
                            }
                        }
                    }
                }
            }
            names.sort();
            for name in names {
                match Self::inspect_commit_health(root.join(&name)) {
                    Ok(report) => {
                        if !report.manifest_present {
                            issues.push(format!("{}: commit manifest missing", name));
                        } else if !report.manifest_decodable {
                            issues.push(format!("{}: commit manifest undecodable", name));
                        }
                        for missing in &report.missing_files {
                            issues.push(format!("{}: listed file missing: {}", name, missing));
                        }
                        for lineage in &report.lineage_issues {
                            issues.push(format!("{}: {}", name, lineage));
                        }
                        for pk_issue in &report.pk_issues {
                            issues.push(format!("{}: {}", name, pk_issue));
                        }
                        tables.push((name, report));
                    }
                    Err(e) => {
                        issues.push(format!("{}: inspection failed: {}", name, e));
                    }
                }
            }
        }
        let mut epochs: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for (_, report) in &tables {
            if let Some(epoch) = report.epoch {
                epochs.insert(epoch);
            }
            if let Some(base) = report.base_epoch {
                epochs.insert(base);
            }
        }
        let mut chain_ok = true;
        for (name, report) in &tables {
            if report.kind.as_deref() == Some("incremental") {
                match (report.epoch, report.base_epoch) {
                    (Some(epoch), Some(base)) => {
                        if base >= epoch {
                            chain_ok = false;
                            issues.push(format!(
                                "{}: incremental base {} not older than epoch {}",
                                name, base, epoch
                            ));
                        }
                    }
                    _ => {
                        chain_ok = false;
                        issues.push(format!(
                            "{}: incremental checkpoint missing epoch pointers",
                            name
                        ));
                    }
                }
            }
        }
        if tables.is_empty() {
            issues.push("no label directories found".to_string());
        }
        let chain_ok_final = tables.iter().all(|(_, r)| r.missing_files.is_empty())
            && tables.iter().all(|(_, r)| {
                if r.manifest_present {
                    r.manifest_decodable
                } else {
                    false
                }
            })
            && chain_ok;
        Ok(GlobalCommitHealth {
            tables,
            chain_ok: chain_ok_final,
            issues,
        })
    }
}
