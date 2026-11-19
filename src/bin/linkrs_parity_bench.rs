//! linkrs-parity-bench: executor for the Ladybug parity benchmark suite.
//!
//! Two subcommands:
//! - `copy`: run a schema + COPY script against a CSV dataset, rewriting
//!   Neo4j admin-import style headers (`id:ID(X)`, `:START_ID(X)`,
//!   `:END_ID(X)`, `prop:type`) into Linkrs COPY headers (`vid`, `src`,
//!   `dst`, bare property names) via sidecar files.
//! - `run`: execute `.benchmark` files (warmup + N timed runs) against a
//!   serialized database and write JSON results comparable with Ladybug logs.
//!   Per-run compile/execute split comes from result metadata; `--skip-file`
//!   takes a skip list (`dataset/group/name` per line, `#` comments).
//!
//! See `docs/release/guides/parity-benchmark.md` for the full methodology.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::Instant;

use linkrs::api::embedded::{DatabaseConfig, GraphDatabase, QueryResult};
use linkrs::storage::GraphStorage;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("copy") => cmd_copy(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        _ => {
            eprintln!(
                "usage:\n  linkrs-parity-bench copy --dataset <name> --csv-dir <dir> \
                 --script <file.cypher> --db <path> [--force]\n  \
                 linkrs-parity-bench run --db <path> --space <name> --benchmark-dir <dir> \
                 [--warmup N] [--run N] [--out <dir>] [--threads N]
                 [--parallel-workers N] [--min-rows-per-partition N]
                 [--skip-file <path>]"
            );
            std::process::exit(2);
        }
    }
}

fn opt_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

// ---------------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------------

struct CopyStats {
    statement: String,
    kind: String,
    elapsed_ms: f64,
    rows: u64,
    error: Option<String>,
}

fn cmd_copy(args: &[String]) {
    let dataset = opt_value(args, "--dataset").unwrap_or_else(|| "dataset".into());
    let csv_dir = PathBuf::from(required(args, "--csv-dir"));
    let script = PathBuf::from(required(args, "--script"));
    let db = PathBuf::from(required(args, "--db"));
    let force = args.iter().any(|a| a == "--force");

    // Serialized-dir layout: everything lives next to the database file.
    let sidecar_dir = db
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&sidecar_dir)
        .unwrap_or_else(|e| die(format!("create {}: {e}", sidecar_dir.display())));
    let version_file = sidecar_dir.join("version.txt");
    let head = git_head();
    if !force {
        if let Ok(v) = fs::read_to_string(&version_file) {
            if v.trim() == head {
                println!(
                    "dataset {dataset} already serialized at version {head}; \
                     skipping COPY (use --force to redo)"
                );
                return;
            }
        }
    }

    let text = fs::read_to_string(&script).unwrap_or_else(|e| die(format!("read {script:?}: {e}")));
    let statements = split_statements(&text);
    let prepare_dir = sidecar_dir.join("prepare");
    fs::create_dir_all(&prepare_dir).unwrap_or_else(|e| die(format!("create prepare dir: {e}")));

    // checkpoint_on_close quiesces the background flush threads and persists
    // one final checkpoint; without an explicit close the edge WAL can be
    // lost on process exit (verified on the LSQB sf0.1 smoke run).
    let config = DatabaseConfig::file(&db).with_checkpoint_on_close(true);
    let database = GraphDatabase::<GraphStorage>::open_with_config(config)
        .unwrap_or_else(|e| die(format!("open db {}: {e}", db.display())));
    let session = database
        .session()
        .unwrap_or_else(|e| die(format!("open session: {e}")));

    let mut stats: Vec<CopyStats> = Vec::new();
    for stmt in &statements {
        let lowered = stmt.trim_start().to_ascii_lowercase();
        let kind = classify_statement(&lowered);
        let stmt = if lowered.starts_with("copy") {
            prepare_copy_statement(stmt, &csv_dir, &prepare_dir)
        } else {
            stmt.clone()
        };

        let start = Instant::now();
        let result = session.execute(&stmt);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(res) => {
                // COPY statements return a summary row [op, affected_count].
                let rows = affected_count(&res);
                println!("[copy] {kind}: {elapsed:.1} ms, {rows} row(s)");
                stats.push(CopyStats {
                    statement: stmt.clone(),
                    kind,
                    elapsed_ms: elapsed,
                    rows,
                    error: None,
                });
            }
            Err(e) => {
                eprintln!("[copy] {kind} FAILED: {e}\nstatement: {stmt}");
                stats.push(CopyStats {
                    statement: stmt,
                    kind,
                    elapsed_ms: elapsed,
                    rows: 0,
                    error: Some(e.to_string()),
                });
                write_copy_log(&sidecar_dir, &dataset, &head, &stats);
                // Flush what was committed before exiting; a half-written
                // checkpoint would block the next open.
                database.close();
                std::process::exit(1);
            }
        }
    }
    write_copy_log(&sidecar_dir, &dataset, &head, &stats);
    // Persist the final checkpoint before recording the version; the next
    // open replays from a complete image instead of a half-written WAL.
    database.close();
    // Import-side bottleneck report: total wall, slowest single statement,
    // and process RSS peak for the memory-model comparison.
    let total_ms: f64 = stats.iter().map(|s| s.elapsed_ms).sum();
    println!(
        "[copy] {dataset} total {total_ms:.1} ms over {} statement(s)",
        stats.len()
    );
    if let Some(slowest) = stats.iter().max_by(|a, b| {
        a.elapsed_ms
            .partial_cmp(&b.elapsed_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        println!(
            "[copy] slowest: {} ({:.1} ms)",
            slowest.kind, slowest.elapsed_ms
        );
    }
    if let Some(kb) = rss_peak_kb() {
        println!("[copy] VmHWM peak: {kb} kB");
    }
    fs::write(&version_file, format!("{head}\n"))
        .unwrap_or_else(|e| die(format!("write version.txt: {e}")));
    println!(
        "dataset {dataset} serialized at {}, version {head}",
        db.display()
    );
}

fn required(args: &[String], flag: &str) -> String {
    opt_value(args, flag).unwrap_or_else(|| die(format!("{flag} is required")))
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("linkrs-parity-bench: {msg}");
    std::process::exit(2);
}

/// The affected-entity count carried by a modify summary result row
/// (`[op_name, count]`), or the plain row count for other statements.
fn affected_count(result: &QueryResult) -> u64 {
    if result.is_empty() {
        return 0;
    }
    // Single-row modify summary: count in the second column.
    if let Some(row) = result.first() {
        if let Some(v) = row.get_by_index(1) {
            // `Value` has an inherent `to_string() -> Result<String, String>`
            // shadowing Display; format explicitly to get the rendered number.
            return format!("{v}").parse().unwrap_or(0);
        }
    }
    result.len() as u64
}

fn git_head() -> String {
    Process::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Split a script into statements on `;` outside single quotes, dropping
/// `--` comment lines and empty statements.
fn split_statements(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("--") {
            continue;
        }
        for ch in line.chars() {
            if ch == '\'' {
                in_quote = !in_quote;
            }
            if ch == ';' && !in_quote {
                out.push(std::mem::take(&mut current));
            } else {
                current.push(ch);
            }
        }
        current.push('\n');
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn classify_statement(lowered: &str) -> String {
    if lowered.starts_with("copy vertex") || lowered.starts_with("copy tag") {
        let t = second_ident(lowered);
        format!("COPY VERTEX {t}")
    } else if lowered.starts_with("copy edge") {
        let t = second_ident(lowered);
        format!("COPY EDGE {t}")
    } else if lowered.starts_with("copy") {
        "COPY".into()
    } else if lowered.starts_with("use") {
        "USE".into()
    } else if lowered.starts_with("create space") {
        "CREATE SPACE".into()
    } else {
        "DDL".into()
    }
}

fn second_ident(lowered: &str) -> String {
    lowered
        .split_whitespace()
        .nth(2)
        .unwrap_or("?")
        .trim_end_matches('(')
        .to_string()
}

/// Rewrite `FROM '<path>'` targets to sidecar CSVs with Linkrs-compatible
/// headers; the original files are never modified.
fn prepare_copy_statement(stmt: &str, csv_dir: &Path, prepare_dir: &Path) -> String {
    let delimiter = statement_delimiter(stmt);
    let mut out = stmt.to_string();
    for original in quoted_paths_after_from(stmt) {
        let src_path = if Path::new(&original).is_absolute() {
            PathBuf::from(&original)
        } else {
            csv_dir.join(&original)
        };
        let rewritten = rewrite_csv_header(&src_path, prepare_dir, delimiter)
            .unwrap_or_else(|e| die(format!("prepare {}: {e}", src_path.display())));
        out = out.replace(
            &format!("'{original}'"),
            &format!("'{}'", rewritten.display()),
        );
    }
    out
}

/// Parse the `DELIMITER '<char>'` option; defaults to comma.
fn statement_delimiter(stmt: &str) -> u8 {
    let lowered = stmt.to_ascii_lowercase();
    match lowered.find("delimiter") {
        Some(pos) => {
            let rest = &lowered[pos + "delimiter".len()..];
            rest.chars()
                .find(|c| !c.is_whitespace() && *c != '\'' && *c != '"' && *c != '=' && *c != '(')
                .map(|c| c as u8)
                .unwrap_or(b',')
        }
        None => b',',
    }
}

/// Quoted file paths following the `FROM` keyword (single or comma-
/// separated list). Single-character segments (the DELIMITER value) are
/// not paths.
fn quoted_paths_after_from(stmt: &str) -> Vec<String> {
    let lowered = stmt.to_ascii_lowercase();
    let Some(from_pos) = lowered.find(" from ") else {
        return Vec::new();
    };
    let rest = &stmt[from_pos + " from ".len()..];
    rest.split('\'')
        .skip(1)
        .step_by(2)
        .filter(|s| s.trim().len() > 1)
        .map(|s| s.trim().to_string())
        .collect()
}

/// Read the CSV's first line, rewrite Neo4j admin-import headers to Linkrs
/// COPY headers, and write a sidecar file under `prepare_dir`. Only the
/// header line changes; the data rows are streamed through unchanged.
fn rewrite_csv_header(src: &Path, prepare_dir: &Path, delimiter: u8) -> std::io::Result<PathBuf> {
    let dst = prepare_dir.join(
        src.file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default(),
    );
    if dst.exists() {
        let src_time = fs::metadata(src)?.modified()?;
        let dst_time = fs::metadata(&dst)?.modified()?;
        if dst_time >= src_time {
            return Ok(dst);
        }
    }
    let file = fs::File::open(src)?;
    let mut reader = BufReader::new(file);
    let mut header = String::new();
    reader.read_line(&mut header)?;
    let delim = delimiter as char;
    let rewritten: Vec<String> = header
        .trim_end_matches(['\r', '\n'])
        .split(delim)
        .map(rewrite_header_field)
        .collect();
    let mut out_file = fs::File::create(&dst)?;
    writeln!(out_file, "{}", rewritten.join(&delim.to_string()))?;
    std::io::copy(&mut reader, &mut out_file)?;
    Ok(dst)
}

fn rewrite_header_field(field: &str) -> String {
    let f = field.trim();
    if f.starts_with(":START_ID") {
        return "src".into();
    }
    if f.starts_with(":END_ID") {
        return "dst".into();
    }
    if f.contains(":ID(") {
        return "vid".into();
    }
    // Property headers like `firstName:string` keep only the property name;
    // bare names (`firstName`) stay as-is.
    match f.find(':') {
        Some(pos) if pos > 0 => f[..pos].trim().to_string(),
        _ => f.to_string(),
    }
}

fn write_copy_log(sidecar_dir: &Path, dataset: &str, version: &str, stats: &[CopyStats]) {
    let mut json = String::from("{\n");
    let _ = writeln!(json, "  \"dataset\": {},", json_escape(dataset));
    let _ = writeln!(json, "  \"version\": {},", json_escape(version));
    json.push_str("  \"statements\": [\n");
    for (i, s) in stats.iter().enumerate() {
        let _ = write!(
            json,
            "    {{\"statement\": {}, \"kind\": {}, \"elapsed_ms\": {:.3}, \"rows\": {}",
            json_escape(&s.statement),
            json_escape(&s.kind),
            s.elapsed_ms,
            s.rows
        );
        if let Some(e) = &s.error {
            let _ = write!(json, ", \"error\": {}", json_escape(e));
        }
        json.push('}');
        if i + 1 < stats.len() {
            json.push(',');
        }
        json.push('\n');
    }
    json.push_str("  ]\n}\n");
    let path = sidecar_dir.join(format!("copy_log_{dataset}.json"));
    fs::write(&path, json).ok();
    println!("copy log written to {}", path.display());
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompareMode {
    Exact,
    Float,
    Unordered,
}

impl CompareMode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "exact" => Some(Self::Exact),
            "float" => Some(Self::Float),
            "unordered" => Some(Self::Unordered),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Float => "float",
            Self::Unordered => "unordered",
        }
    }
}

struct Benchmark {
    name: String,
    query: String,
    prerun: String,
    expected: Option<String>,
    compare: CompareMode,
}

fn benchmark_marker(line: &str) -> Option<(&str, Option<&str>)> {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("----") {
        let key = rest.trim();
        if key.parse::<usize>().is_ok() {
            return Some(("EXPECTED", None));
        }
        return Some((key, None));
    }
    for key in ["NAME", "QUERY", "PRERUN", "EXPECTED", "COMPARE"] {
        let upper = format!("-{key}");
        let lower = upper.to_ascii_lowercase();
        if let Some(rest) = trimmed
            .strip_prefix(&upper)
            .or_else(|| trimmed.strip_prefix(&lower))
        {
            let value = rest.trim_start();
            return Some((key, if value.is_empty() { None } else { Some(value) }));
        }
    }
    None
}

/// Parse a Ladybug-style `.benchmark` file. Both the local `----key`
/// blocks and the upstream `-KEY value` markers are accepted.
fn parse_benchmark_file(path: &Path) -> Benchmark {
    let text = fs::read_to_string(path).unwrap_or_else(|e| die(format!("read {path:?}: {e}")));
    parse_benchmark_text(&text, path)
}

fn parse_benchmark_text(text: &str, path: &Path) -> Benchmark {
    let mut blocks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some((key, inline)) = benchmark_marker(line) {
            let key = key.to_ascii_lowercase();
            current = Some(key.clone());
            let block = blocks.entry(key).or_default();
            if let Some(value) = inline {
                block.push(value.to_string());
            }
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('-') && !trimmed.starts_with("--") {
            current = None;
            continue;
        }
        if let Some(key) = &current {
            blocks
                .entry(key.clone())
                .or_default()
                .push(line.to_string());
        }
    }
    let query = blocks
        .get("query")
        .map(|lines| lines.join("\n").trim().to_string())
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| die(format!("{} has no query block", path.display())));
    let expected = blocks.get("expected").map(|lines| {
        let kept: Vec<&str> = lines
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        kept.join("\n")
    });
    let expected = expected.filter(|e| !e.is_empty());
    let prerun = blocks
        .get("prerun")
        .map(|lines| lines.join("\n").trim().to_string())
        .unwrap_or_default();
    let compare = blocks
        .get("compare")
        .and_then(|lines| lines.first())
        .and_then(|value| CompareMode::parse(value))
        .unwrap_or(CompareMode::Exact);
    let name = blocks
        .get("name")
        .map(|lines| lines.join(" ").trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });
    Benchmark {
        name,
        query,
        prerun,
        expected,
        compare,
    }
}

fn collect_benchmark_files(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| die(format!("read {dir:?}: {e}")))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    let mut files = Vec::new();
    for path in entries {
        if path.is_dir() {
            files.extend(collect_benchmark_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "benchmark") {
            files.push(path);
        }
    }
    files
}

fn results_match(expected: &str, got: &str, mode: CompareMode) -> bool {
    match mode {
        CompareMode::Exact => expected == got,
        CompareMode::Unordered => {
            let mut expected_lines: Vec<&str> = expected.lines().map(str::trim).collect();
            let mut got_lines: Vec<&str> = got.lines().map(str::trim).collect();
            expected_lines.sort_unstable();
            got_lines.sort_unstable();
            expected_lines == got_lines
        }
        CompareMode::Float => {
            let expected_lines: Vec<&str> = expected.lines().map(str::trim).collect();
            let got_lines: Vec<&str> = got.lines().map(str::trim).collect();
            expected_lines.len() == got_lines.len()
                && expected_lines
                    .iter()
                    .zip(got_lines.iter())
                    .all(|(e, g)| cell_matches(e, g))
        }
    }
}

fn cell_matches(expected: &str, got: &str) -> bool {
    let expected_cells: Vec<&str> = expected.split('|').collect();
    let got_cells: Vec<&str> = got.split('|').collect();
    if expected_cells.len() != got_cells.len() {
        return false;
    }
    expected_cells.iter().zip(got_cells.iter()).all(|(e, g)| {
        if e == g {
            return true;
        }
        let expected_cell = e.trim();
        let got_cell = g.trim();
        if expected_cell.parse::<i64>().is_ok() && got_cell.parse::<i64>().is_ok() {
            return false;
        }
        match (expected_cell.parse::<f64>(), got_cell.parse::<f64>()) {
            (Ok(expected), Ok(got)) => {
                let scale = expected.abs().max(got.abs()).max(1.0);
                (expected - got).abs() <= 1e-3 * scale
            }
            _ => false,
        }
    })
}

fn cmd_run(args: &[String]) {
    let db = PathBuf::from(required(args, "--db"));
    let space = required(args, "--space");
    let bench_dir = PathBuf::from(required(args, "--benchmark-dir"));
    let out_dir = PathBuf::from(
        opt_value(args, "--out").unwrap_or_else(|| "/tmp/linkrs_parity_bench".into()),
    );
    let warmup: usize = opt_value(args, "--warmup")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let runs: usize = opt_value(args, "--run")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let threads: u32 = opt_value(args, "--threads")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1)
        });
    let parallel_workers: usize = opt_value(args, "--parallel-workers")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let min_rows_per_partition = opt_value(args, "--min-rows-per-partition").map(|value| {
        value
            .parse::<u64>()
            .unwrap_or_else(|e| die(format!("invalid --min-rows-per-partition {value}: {e}")))
    });
    let skip_file = opt_value(args, "--skip-file");
    let skip_set = skip_file.as_deref().map(load_skip_file).unwrap_or_default();

    let mut db_config = DatabaseConfig::file(&db).with_checkpoint_on_close(true);
    if parallel_workers > 1 {
        let mut parallel_config = linkrs::config::ParallelConfig {
            workers: parallel_workers,
            max_partitions: parallel_workers,
            allow_ordered_parallel_expand: true,
            ..linkrs::config::ParallelConfig::default()
        };
        if let Some(min_rows) = min_rows_per_partition {
            parallel_config.min_rows_per_partition = min_rows;
        }
        db_config = db_config.with_parallel_config(parallel_config);
    }
    let database = GraphDatabase::<GraphStorage>::open_with_config(db_config)
        .unwrap_or_else(|e| die(format!("open db {}: {e}", db.display())));
    let session = database
        .session()
        .unwrap_or_else(|e| die(format!("open session: {e}")));
    session
        .execute(&format!("USE {space}"))
        .unwrap_or_else(|e| die(format!("USE {space}: {e}")));

    fs::create_dir_all(&out_dir).unwrap_or_else(|e| die(format!("create out dir: {e}")));

    let files = collect_benchmark_files(&bench_dir);

    println!(
        "[run] db={} space={space} warmup={warmup} run={runs} threads={threads} \
         parallel_workers={parallel_workers} skipped={}",
        db.display(),
        skip_set.len()
    );
    if let Some(kb) = rss_peak_kb() {
        println!("[run] VmHWM peak at start: {kb} kB");
    }

    for path in &files {
        let bench = parse_benchmark_file(path);
        if is_skipped(&skip_set, &bench.name) {
            println!("[run] {} skipped by skip file", bench.name);
            write_run_result(&out_dir, &bench, None, None, None, "skipped", threads);
            continue;
        }
        let mut warmup_failed = false;
        for statement in split_statements(&bench.prerun) {
            if let Err(e) = session.execute(&statement) {
                eprintln!("[run] {} prerun failed: {e}", bench.name);
                write_run_result(
                    &out_dir,
                    &bench,
                    None,
                    None,
                    None,
                    &format!("prerun error: {e}"),
                    threads,
                );
                warmup_failed = true;
                break;
            }
        }
        for _ in 0..warmup {
            if let Err(e) = session.execute(&bench.query) {
                eprintln!("[run] {} warmup failed: {e}", bench.name);
                write_run_result(
                    &out_dir,
                    &bench,
                    None,
                    None,
                    None,
                    &format!("warmup error: {e}"),
                    threads,
                );
                warmup_failed = true;
                break;
            }
        }
        if warmup_failed {
            continue;
        }
        let mut times_ms: Vec<f64> = Vec::with_capacity(runs);
        let mut compile_ms: Vec<f64> = Vec::with_capacity(runs);
        let mut execute_ms: Vec<f64> = Vec::with_capacity(runs);
        let mut rows_seen = 0usize;
        let mut got_lines: Vec<String> = Vec::new();
        let mut error: Option<String> = None;
        for _ in 0..runs {
            let start = Instant::now();
            match session.execute(&bench.query) {
                Ok(result) => {
                    rows_seen = result.len();
                    got_lines = format_rows(&result);
                    times_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                    // Compiling/executing split per the Ladybug reporting
                    // pair: compile covers parse through optimize, execute
                    // covers the execution stage only.
                    let meta = result.metadata();
                    compile_ms.push(meta.compile_time.as_secs_f64() * 1000.0);
                    execute_ms.push(meta.execute_time.as_secs_f64() * 1000.0);
                }
                Err(e) => {
                    error = Some(e.to_string());
                    break;
                }
            }
        }
        if let Some(err) = &error {
            eprintln!("[run] {} FAILED: {err}", bench.name);
            write_run_result(&out_dir, &bench, None, None, None, err, threads);
            continue;
        }
        let mut status = "ok";
        if let Some(expected) = &bench.expected {
            let got = got_lines.join("\n");
            if !results_match(expected, &got, bench.compare) {
                status = "mismatch";
                eprintln!(
                    "[run] {} result mismatch\n  expected: {expected:?}\n  got: {got:?}",
                    bench.name
                );
            }
        }
        let exec_ms = trimmed_mean(&times_ms);
        let median_ms = median(&times_ms);
        let compile_mean = trimmed_mean(&compile_ms);
        let compile_median = median(&compile_ms);
        let execute_mean = trimmed_mean(&execute_ms);
        let execute_median = median(&execute_ms);
        println!(
            "[run] {}: {} run(s), exec {exec_ms:.2} ms (drop-max mean, median {median_ms:.2}), \
             compile {compile_mean:.2} ms (median {compile_median:.2}), \
             execute-stage {execute_mean:.2} ms (median {execute_median:.2}), \
             {rows_seen} row(s), status {status}",
            bench.name,
            times_ms.len()
        );
        write_run_result(
            &out_dir,
            &bench,
            Some(&times_ms),
            Some(&compile_ms),
            Some(&execute_ms),
            status,
            threads,
        );
    }
    if let Some(kb) = rss_peak_kb() {
        println!("[run] VmHWM peak at end: {kb} kB");
    }
}

/// Materialize every row as `value|value` lines, mirroring the Ladybug
/// expected-block text format.
fn format_rows(result: &QueryResult) -> Vec<String> {
    let cols = result.columns().len();
    result
        .iter()
        .map(|row| {
            (0..cols)
                .filter_map(|i| row.get_by_index(i).map(|v| format!("{v}")))
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted[sorted.len() / 2]
}

/// Load a skip list (`dataset/group/name` per line, `#` comments, blank
/// lines ignored) as used by `benches/parity/skip.txt`.
fn skip_entry(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    trimmed.split_whitespace().next().map(str::to_string)
}

fn load_skip_file(path: &str) -> HashSet<String> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| die(format!("read {path}: {e}")));
    text.lines().filter_map(skip_entry).collect()
}

/// Whether a benchmark name is covered by the skip list: exact match, or
/// the entry ends with `/name` so group-qualified entries match plain names.
fn is_skipped(skip_set: &HashSet<String>, name: &str) -> bool {
    skip_set.contains(name) || skip_set.iter().any(|e| e.ends_with(&format!("/{name}")))
}

/// Process RSS peak (VmHWM) in kB from `/proc/self/status`.
/// Linux-only; anything else reports nothing instead of failing the run.
fn rss_peak_kb() -> Option<u64> {
    let text = fs::read_to_string("/proc/self/status").ok()?;
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("VmHWM:")?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })
}

/// LSQB sample processing (docs/release/benchmark/parity-benchmark.md §6):
/// drop the slowest run and average the rest; a single run degenerates to it.
fn trimmed_mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    if values.len() == 1 {
        return values[0];
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let kept = &sorted[..sorted.len() - 1];
    kept.iter().sum::<f64>() / kept.len() as f64
}

fn write_run_result(
    out_dir: &Path,
    bench: &Benchmark,
    times: Option<&[f64]>,
    compile: Option<&[f64]>,
    execute: Option<&[f64]>,
    status: &str,
    threads: u32,
) {
    let mut json = String::from("{\n");
    let _ = writeln!(json, "  \"name\": {},", json_escape(&bench.name));
    let _ = writeln!(json, "  \"status\": {},", json_escape(status));
    let _ = writeln!(json, "  \"threads\": {threads},");
    let _ = writeln!(
        json,
        "  \"compare_mode\": {},",
        json_escape(bench.compare.as_str())
    );
    match times {
        Some(times) => {
            let list = times
                .iter()
                .map(|t| format!("{t:.3}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(json, "  \"execution_times_ms\": [{list}],");
            let _ = writeln!(json, "  \"execution_ms\": {:.3},", trimmed_mean(times));
            let _ = writeln!(json, "  \"median_ms\": {:.3},", median(times));
        }
        None => {
            json.push_str("  \"execution_times_ms\": [],\n");
            json.push_str("  \"execution_ms\": 0,\n");
            json.push_str("  \"median_ms\": 0,\n");
        }
    }
    match compile {
        Some(values) => {
            let list = values
                .iter()
                .map(|t| format!("{t:.3}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(json, "  \"compile_times_ms\": [{list}],");
            let _ = writeln!(json, "  \"compile_ms\": {:.3},", trimmed_mean(values));
            let _ = writeln!(json, "  \"compile_median_ms\": {:.3},", median(values));
        }
        None => {
            json.push_str("  \"compile_times_ms\": [],\n");
            json.push_str("  \"compile_ms\": 0,\n");
            json.push_str("  \"compile_median_ms\": 0,\n");
        }
    }
    match execute {
        Some(values) => {
            let list = values
                .iter()
                .map(|t| format!("{t:.3}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(json, "  \"execute_times_ms\": [{list}],");
            let _ = writeln!(json, "  \"execute_ms\": {:.3},", trimmed_mean(values));
            let _ = writeln!(json, "  \"execute_median_ms\": {:.3},", median(values));
        }
        None => {
            json.push_str("  \"execute_times_ms\": [],\n");
            json.push_str("  \"execute_ms\": 0,\n");
            json.push_str("  \"execute_median_ms\": 0,\n");
        }
    }
    let _ = writeln!(json, "  \"query\": {}", json_escape(&bench.query));
    json.push_str("}\n");
    let path = out_dir.join(format!("{}.json", bench.name.replace('/', "_")));
    fs::write(&path, json).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn benchmark_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("benches/parity/queries/lsqb")
            .join(name)
    }

    #[test]
    fn parses_nested_benchmark_files() {
        let path = benchmark_path("operators/filter_id_range.benchmark");
        let bench = parse_benchmark_file(&path);
        assert_eq!(bench.name, "lsqb/operators/filter_id_range");
        assert!(bench.query.contains("MATCH (n:Person)"));
        assert!(bench.prerun.is_empty());
        assert_eq!(bench.compare, CompareMode::Exact);
    }

    #[test]
    fn parses_upstream_markers_and_expected_row_count() {
        let text = [
            "-NAME upstream/q1",
            "-PRERUN USE lsqb;",
            "-QUERY MATCH (n:Person) RETURN count(n)",
            "---- 1",
            "1700",
        ]
        .join("\n");
        let bench = parse_benchmark_text(&text, Path::new("upstream/q1.benchmark"));
        assert_eq!(bench.name, "upstream/q1");
        assert_eq!(bench.prerun, "USE lsqb;");
        assert_eq!(bench.expected.as_deref(), Some("1700"));
    }

    #[test]
    fn discovers_benchmark_files_in_nested_groups() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/parity/queries/lsqb");
        let files = collect_benchmark_files(&root);
        assert!(
            files
                .iter()
                .any(|p| p.ends_with("operators/filter_id_range.benchmark")),
            "nested operator benchmark must be discovered, got {} file(s)",
            files.len()
        );
    }

    #[test]
    fn skip_entries_use_only_the_first_token() {
        assert_eq!(skip_entry("# reason"), None);
        assert_eq!(skip_entry("  "), None);
        assert_eq!(
            skip_entry("lsqb/operators/shortest_path unsupported syntax | unlock with fix"),
            Some("lsqb/operators/shortest_path".to_string())
        );
    }

    #[test]
    fn compare_modes_cover_exact_float_and_unordered_results() {
        assert!(results_match("1|2", "1|2", CompareMode::Exact));
        assert!(!results_match("1|2", "2|1", CompareMode::Exact));
        assert!(results_match("1\n2", "2\n1", CompareMode::Unordered));
        assert!(results_match(
            "8737|0.000138",
            "8737|0.0001381",
            CompareMode::Float
        ));
        assert!(!results_match(
            "8737|0.000138",
            "8738|0.000138",
            CompareMode::Float
        ));
    }
}
