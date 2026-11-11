//! linkrs-parity-bench: executor for the Ladybug parity benchmark suite.
//!
//! Two subcommands:
//! - `copy`: run a schema + COPY script against a CSV dataset, rewriting
//!   Neo4j admin-import style headers (`id:ID(X)`, `:START_ID(X)`,
//!   `:END_ID(X)`, `prop:type`) into Linkrs COPY headers (`vid`, `src`,
//!   `dst`, bare property names) via sidecar files.
//! - `run`: execute `.benchmark` files (warmup + N timed runs) against a
//!   serialized database and write JSON results comparable with Ladybug logs.
//!
//! See `docs/release/guides/parity-benchmark.md` for the full methodology.

use std::collections::BTreeMap;
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
                 [--warmup N] [--run N] [--out <dir>] [--threads N]"
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
    fs::create_dir_all(&sidecar_dir).unwrap_or_else(|e| die(format!("create {}: {e}", sidecar_dir.display())));
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
    let session = database.session().unwrap_or_else(|e| die(format!("open session: {e}")));

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
    fs::write(&version_file, format!("{head}\n")).unwrap_or_else(|e| die(format!("write version.txt: {e}")));
    println!("dataset {dataset} serialized at {}, version {head}", db.display());
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
        out = out.replace(&format!("'{original}'"), &format!("'{}'", rewritten.display()));
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

struct Benchmark {
    name: String,
    query: String,
    expected: Option<String>,
}

/// Parse a Ladybug-style `.benchmark` file:
/// `----name` / `----query` / optional `----expected` blocks.
fn parse_benchmark_file(path: &Path) -> Benchmark {
    let text = fs::read_to_string(path).unwrap_or_else(|e| die(format!("read {path:?}: {e}")));
    let mut blocks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(key) = line.strip_prefix("----") {
            let key = key.trim().to_string();
            current = Some(key.clone());
            blocks.entry(key).or_default();
            continue;
        }
        if let Some(key) = &current {
            blocks.entry(key.clone()).or_default().push(line.to_string());
        }
    }
    let query = blocks
        .get("query")
        .map(|lines| lines.join("\n").trim().to_string())
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| die(format!("{} has no ----query block", path.display())));
    let expected = blocks.get("expected").map(|lines| {
        let kept: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        kept.join("\n")
    });
    let expected = expected.filter(|e| !e.is_empty());
    let name = blocks
        .get("name")
        .map(|lines| lines.join(" ").trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });
    Benchmark { name, query, expected }
}

fn cmd_run(args: &[String]) {
    let db = PathBuf::from(required(args, "--db"));
    let space = required(args, "--space");
    let bench_dir = PathBuf::from(required(args, "--benchmark-dir"));
    let out_dir = PathBuf::from(opt_value(args, "--out").unwrap_or_else(|| "/tmp/linkrs_parity_bench".into()));
    let warmup: usize = opt_value(args, "--warmup").and_then(|v| v.parse().ok()).unwrap_or(1);
    let runs: usize = opt_value(args, "--run").and_then(|v| v.parse().ok()).unwrap_or(3);
    let threads: u32 = opt_value(args, "--threads")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1)
        });

    let database = GraphDatabase::<GraphStorage>::open(&db)
        .unwrap_or_else(|e| die(format!("open db {}: {e}", db.display())));
    let session = database.session().unwrap_or_else(|e| die(format!("open session: {e}")));
    session
        .execute(&format!("USE {space}"))
        .unwrap_or_else(|e| die(format!("USE {space}: {e}")));

    fs::create_dir_all(&out_dir).unwrap_or_else(|e| die(format!("create out dir: {e}")));

    let mut files: Vec<PathBuf> = fs::read_dir(&bench_dir)
        .unwrap_or_else(|e| die(format!("read {bench_dir:?}: {e}")))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "benchmark").unwrap_or(false))
        .collect();
    files.sort();

    println!(
        "[run] db={} space={space} warmup={warmup} run={runs} threads={threads}",
        db.display()
    );

    for path in &files {
        let bench = parse_benchmark_file(path);
        let mut warmup_failed = false;
        for _ in 0..warmup {
            if let Err(e) = session.execute(&bench.query) {
                eprintln!("[run] {} warmup failed: {e}", bench.name);
                write_run_result(&out_dir, &bench, None, &format!("warmup error: {e}"), threads);
                warmup_failed = true;
                break;
            }
        }
        if warmup_failed {
            continue;
        }
        let mut times_ms: Vec<f64> = Vec::with_capacity(runs);
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
                }
                Err(e) => {
                    error = Some(e.to_string());
                    break;
                }
            }
        }
        if let Some(err) = &error {
            eprintln!("[run] {} FAILED: {err}", bench.name);
            write_run_result(&out_dir, &bench, None, err, threads);
            continue;
        }
        let mut status = "ok";
        if let Some(expected) = &bench.expected {
            let got = got_lines.join("\n");
            if got != *expected {
                status = "mismatch";
                eprintln!(
                    "[run] {} result mismatch\n  expected: {expected:?}\n  got: {got:?}",
                    bench.name
                );
            }
        }
        let exec_ms = trimmed_mean(&times_ms);
        let median = median(&times_ms);
        println!(
            "[run] {}: {} run(s), exec {exec_ms:.2} ms (drop-max mean, median {median:.2}), {rows_seen} row(s), status {status}",
            bench.name,
            times_ms.len()
        );
        write_run_result(&out_dir, &bench, Some(&times_ms), status, threads);
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

fn write_run_result(out_dir: &Path, bench: &Benchmark, times: Option<&[f64]>, status: &str, threads: u32) {
    let mut json = String::from("{\n");
    let _ = writeln!(json, "  \"name\": {},", json_escape(&bench.name));
    let _ = writeln!(json, "  \"status\": {},", json_escape(status));
    let _ = writeln!(json, "  \"threads\": {threads},");
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
    let _ = writeln!(json, "  \"query\": {}", json_escape(&bench.query));
    json.push_str("}\n");
    let path = out_dir.join(format!("{}.json", bench.name.replace('/', "_")));
    fs::write(&path, json).ok();
}
