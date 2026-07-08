use std::sync::Mutex;

static CLEANUP_DONE: Mutex<bool> = Mutex::new(false);

pub fn cleanup_old_e2e_collections() {
    let mut done = CLEANUP_DONE.lock().unwrap();
    if *done {
        return;
    }
    *done = true;

    let port = std::env::var("QDRANT_HTTP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6333);
    std::thread::spawn(move || {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build();
        let Ok(http) = http else {
            return;
        };
        let url = format!("http://localhost:{}/collections", port);
        let Ok(resp) = http.get(&url).send() else {
            return;
        };
        let Ok(json) = resp.json::<serde_json::Value>() else {
            return;
        };
        let Some(collections) = json["result"]["collections"].as_array() else {
            return;
        };
        let mut cleaned = 0;
        for col in collections {
            let Some(name) = col["name"].as_str() else {
                continue;
            };
            if name.starts_with("e2e_") || name.starts_with("quant_") {
                let del_url = format!("http://localhost:{}/collections/{}", port, name);
                if http.delete(&del_url).send().is_ok() {
                    cleaned += 1;
                }
            }
        }
        if cleaned > 0 {
            eprintln!("[E2E] Cleaned up {} old test collections", cleaned);
        }
    });
}
