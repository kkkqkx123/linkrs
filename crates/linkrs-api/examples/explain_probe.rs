//! Diagnostic probe: run EXPLAIN + timed COUNT queries against a prepared LSQB db.
//! Usage: explain_probe <db-path> <space>
use std::time::Instant;

use linkrs_api::embedded::{DatabaseConfig, GraphDatabase};
use linkrs_storage::GraphStorage;

fn main() {
    let mut args = std::env::args().skip(1);
    let db = args.next().expect("db path required");
    let space = args.next().expect("space required");

    let config = DatabaseConfig::file(std::path::Path::new(&db)).with_checkpoint_on_close(false);
    let database = GraphDatabase::<GraphStorage>::open_with_config(config)
        .unwrap_or_else(|e| die(format!("open db: {e}")));
    let session = database
        .session()
        .unwrap_or_else(|e| die(format!("session: {e}")));
    session
        .execute(&format!("USE {space}"))
        .unwrap_or_else(|e| die(format!("USE: {e}")));

    let probes: &[&str] = &[
        "EXPLAIN MATCH (n:Person) RETURN count(n)",
        "EXPLAIN MATCH (n:Comment_) RETURN count(n)",
        "EXPLAIN MATCH (:Person)-[k:KNOWS]->(:Person) RETURN count(k)",
        "EXPLAIN MATCH (:Comment_)-[h:HASCREATOR_COMMENT]->(:Person) RETURN count(h)",
        "EXPLAIN MATCH (:Comment_)-[r:REPLYOF_POST]->(:Post) RETURN count(r)",
        "MATCH (n:Person) RETURN count(n)",
        "MATCH (n:Comment_) RETURN count(n)",
        "MATCH (:Person)-[k:KNOWS]->(:Person) RETURN count(k)",
        "MATCH (:Comment_)-[h:HASCREATOR_COMMENT]->(:Person) RETURN count(h)",
        "MATCH (:Comment_)-[r:REPLYOF_POST]->(:Post) RETURN count(r)",
    ];

    for probe in probes {
        let start = Instant::now();
        let out = match session.execute(probe) {
            Ok(r) => r,
            Err(e) => {
                println!("--- {probe}\nERROR: {e}");
                continue;
            }
        };
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let mut body = String::new();
        for row in out.iter() {
            body.push_str(&format!("{row:?}\n"));
        }
        println!("--- {probe}  [{elapsed:.2} ms wall]\n{body}");
    }
}

fn die(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}
