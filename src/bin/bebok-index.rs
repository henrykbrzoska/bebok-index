//! `bebok-index` standalone plugin server.
//!
//! Spawned by the Bebok engine from the plugin slot directory and spoken to
//! over **JSON-lines on stdio**: one request object per stdin line, exactly
//! one response object per stdout line. Nothing else may ever be printed on
//! stdout (logs go to stderr, which the engine discards).
//!
//! Supported actions:
//! - `{"action":"status","directory":"..."}` → current index snapshot.
//! - `{"action":"search","directory":"...","query":"...","limit":N?}`
//!   → full-text hits (waits up to ~25 s for the first build).
//! - `{"action":"rebuild","directory":"..."}` → trigger a full rebuild.
//! - `{"action":"notify","directory":"...","path"?}` → nudge a rescan.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bebok_code_index::{
    CodeIndexBackend, StatusCallback, factory::spawn_default_backend, indexing_disabled,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Crate version, also reported via `--version`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default hit limit when the request carries no `limit`.
const DEFAULT_LIMIT: usize = 5;
/// Scan cap per backend: none — index every file the scan finds.
/// (Previously 10_000; removed so large projects are never truncated.)
const MAX_FILES_UNCAPPED: usize = usize::MAX;
/// How long `search` waits for the first build (engine times out at 30 s).
const QUERY_WAIT_TOTAL_MS: u64 = 25_000;
/// Poll interval while waiting for `ready`.
const QUERY_POLL_MS: u64 = 100;

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Data dir without extra dependencies: `%APPDATA%` (else `%LOCALAPPDATA%`,
/// else temp) on Windows; `$XDG_DATA_HOME` (else `~/.local/share`, else
/// temp) on Unix.
fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        for var in ["APPDATA", "LOCALAPPDATA"] {
            if let Ok(v) = std::env::var(var)
                && !v.trim().is_empty()
            {
                return PathBuf::from(v);
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(v) = std::env::var("XDG_DATA_HOME")
            && !v.trim().is_empty()
        {
            return PathBuf::from(v);
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home).join(".local/share");
        }
    }
    std::env::temp_dir()
}

/// Per-directory index storage: `<data>/bebok/plugin-index/<blake3(dir)>`.
fn index_dir_for(directory: &str) -> PathBuf {
    let hash = blake3::hash(directory.as_bytes());
    data_dir()
        .join("bebok")
        .join("plugin-index")
        .join(hash.to_hex().as_str())
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

/// Long-lived server holding one lazily-created backend per directory.
struct Server {
    backends: Mutex<HashMap<String, Arc<dyn CodeIndexBackend>>>,
    enabled: bool,
}

impl Server {
    fn new() -> Self {
        Self {
            backends: Mutex::new(HashMap::new()),
            enabled: !indexing_disabled(),
        }
    }

    /// Return the backend for `directory`, creating it on first use.
    fn backend_for(&self, directory: &str) -> Arc<dyn CodeIndexBackend> {
        let mut map = self.backends.lock().unwrap();
        if let Some(b) = map.get(directory) {
            return b.clone();
        }
        let index_dir = index_dir_for(directory);
        let _ = std::fs::create_dir_all(&index_dir);
        let noop: StatusCallback = Arc::new(|_| {});
        let b = spawn_default_backend(
            PathBuf::from(directory),
            index_dir,
            vec![],
            MAX_FILES_UNCAPPED,
            self.enabled,
            noop,
        );
        map.insert(directory.to_string(), b.clone());
        b
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

fn err(msg: impl Into<String>) -> Value {
    json!({ "ok": false, "error": msg.into() })
}

/// Effective directory for a request: explicit `directory`, else the
/// process working directory (the engine spawns us with cwd = slot dir,
/// but callers normally pass the project root explicitly).
fn request_directory(obj: &serde_json::Map<String, Value>) -> String {
    obj.get("directory")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

async fn handle(server: &Server, req: &Value) -> Value {
    let Some(obj) = req.as_object() else {
        return err("request must be a JSON object");
    };
    let action = obj.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let directory = request_directory(obj);
    match action {
        "status" => {
            let b = server.backend_for(&directory);
            let s = b.status();
            json!({
                "ok": true,
                "status": s.status,
                "files": s.files,
                "symbols": s.symbols,
                "directory": directory,
            })
        }
        "search" => {
            let query = obj.get("query").and_then(|v| v.as_str()).unwrap_or("");
            if query.trim().is_empty() {
                return err("query must not be empty");
            }
            let limit = obj
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_LIMIT);
            let extension = obj
                .get("extension")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let b = server.backend_for(&directory);
            // Wait for the first build, but leave headroom before the
            // engine's 30 s per-invocation timeout.
            let mut waited = 0u64;
            loop {
                let state = b.status().status;
                if state == bebok_code_index::CODE_INDEX_READY {
                    break;
                }
                if state == bebok_code_index::CODE_INDEX_DISABLED {
                    return err("code index is disabled");
                }
                if waited >= QUERY_WAIT_TOTAL_MS {
                    return err("index not ready (still indexing)");
                }
                tokio::time::sleep(Duration::from_millis(QUERY_POLL_MS)).await;
                waited += QUERY_POLL_MS;
            }
            match b.query(query, extension.as_deref(), limit).await {
                Ok(hits) => json!({
                    "ok": true,
                    "results": hits.iter().map(|h| json!({ "path": h.path, "score": h.score })).collect::<Vec<_>>(),
                    "directory": directory,
                }),
                Err(e) => err(e.to_string()),
            }
        }
        "rebuild" => {
            let b = server.backend_for(&directory);
            b.rebuild();
            let s = b.status();
            json!({
                "ok": true,
                "status": s.status,
                "files": s.files,
                "symbols": s.symbols,
                "directory": directory,
            })
        }
        "notify" => {
            let b = server.backend_for(&directory);
            b.notify_changed(obj.get("path").and_then(|v| v.as_str()));
            json!({ "ok": true, "directory": directory })
        }
        other => err(format!("unknown action '{other}'")),
    }
}

/// Parse one stdin line and produce the response value. Never panics.
async fn respond_to_line(server: &Server, line: &str) -> Value {
    match serde_json::from_str::<Value>(line) {
        Ok(req) => handle(server, &req).await,
        Err(e) => err(format!("invalid request: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("bebok-index {VERSION}");
        return;
    }
    if !args.iter().any(|a| a == "--plugin-server") {
        eprintln!("usage: bebok-index --plugin-server");
        eprintln!("  Runs the Bebok plugin server (JSON-lines over stdio).");
        std::process::exit(2);
    }

    let server = Server::new();
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.unwrap_or(0);
        if n == 0 {
            break; // EOF — the engine went away.
        }
        if line.trim().is_empty() {
            continue;
        }
        let resp = respond_to_line(&server, line.trim()).await;
        let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
            r#"{"ok":false,"error":"response serialisation failed"}"#.to_string()
        });
        out.push('\n');
        if stdout.write_all(out.as_bytes()).await.is_err() {
            break;
        }
        if stdout.flush().await.is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_server_disabled() -> Server {
        Server {
            backends: Mutex::new(HashMap::new()),
            enabled: false,
        }
    }

    #[tokio::test]
    async fn unknown_action_reports_name() {
        let server = Server::new();
        let resp = respond_to_line(&server, r#"{"action":"frobnicate","directory":"/tmp"}"#).await;
        assert_eq!(resp["ok"], false);
        assert!(
            resp["error"].as_str().unwrap().contains("frobnicate"),
            "got {resp}"
        );
    }

    #[tokio::test]
    async fn invalid_json_never_panics() {
        let server = Server::new();
        for bad in ["{not json", "", "   ", "[1,2,3]", "42", "\"str\""] {
            let resp = respond_to_line(&server, bad).await;
            assert_eq!(resp["ok"], false, "input {bad:?} gave {resp}");
        }
    }

    #[tokio::test]
    async fn empty_query_is_rejected() {
        let server = Server::new();
        let resp = respond_to_line(
            &server,
            r#"{"action":"search","directory":"/tmp","query":"   "}"#,
        )
        .await;
        assert_eq!(resp["ok"], false);
    }

    #[test]
    fn responses_serialise_to_single_line() {
        for resp in [
            json!({"ok": true, "status": "ready", "files": 3, "symbols": 0}),
            json!({"ok": true, "results": [{"path": "a/b.rs", "score": 1.5}]}),
            err("multi\nline\nerror"),
        ] {
            let s = serde_json::to_string(&resp).unwrap();
            assert!(!s.contains('\n'), "response leaks newline: {s}");
            // Round-trips as one JSON-lines frame.
            assert!(serde_json::from_str::<Value>(&s).is_ok());
        }
    }

    #[test]
    fn index_dir_is_stable_and_hex_named() {
        let a = index_dir_for("/some/project");
        let b = index_dir_for("/some/project");
        let c = index_dir_for("/other/project");
        assert_eq!(a, b);
        assert_ne!(a, c);
        let leaf = a.file_name().unwrap().to_string_lossy();
        assert_eq!(leaf.len(), 64);
        assert!(leaf.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn disabled_server_answers_status_and_refuses_search() {
        let server = test_server_disabled();
        let dir = std::env::temp_dir().join(format!("bebok-srv-{}", uuid_bin()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();

        let status = respond_to_line(
            &server,
            &format!(r#"{{"action":"status","directory":{d:?}}}"#),
        )
        .await;
        assert_eq!(status["ok"], true);
        assert_eq!(status["status"], "disabled");
        assert_eq!(status["directory"], d.as_str());

        let search = respond_to_line(
            &server,
            &format!(r#"{{"action":"search","directory":{d:?},"query":"x"}}"#),
        )
        .await;
        assert_eq!(search["ok"], false, "got {search}");

        let notify = respond_to_line(
            &server,
            &format!(r#"{{"action":"notify","directory":{d:?},"path":"a.rs"}}"#),
        )
        .await;
        assert_eq!(notify["ok"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn live_server_indexes_and_searches_tmp_project() {
        let server = Server::new();
        assert!(server.enabled, "BEBOK_NO_INDEX must be unset for this test");
        let dir = std::env::temp_dir().join(format!("bebok-srvlive-{}", uuid_bin()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.rs"), "fn liveserverprobefn() {}").unwrap();
        let d = dir.to_string_lossy().to_string();

        // Status: poll until ready (first build runs in the background).
        let mut ready = false;
        for _ in 0..100 {
            let s = respond_to_line(
                &server,
                &format!(r#"{{"action":"status","directory":{d:?}}}"#),
            )
            .await;
            assert_eq!(s["ok"], true);
            if s["status"] == "ready" {
                assert!(s["files"].as_u64().unwrap() >= 1);
                ready = true;
                break;
            }
            assert_ne!(s["status"], "disabled", "build failed: {s}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(ready, "index never became ready");

        // Search finds the file (the search path itself waits for ready).
        let hits = respond_to_line(
            &server,
            &format!(
                r#"{{"action":"search","directory":{d:?},"query":"liveserverprobefn","limit":5}}"#
            ),
        )
        .await;
        assert_eq!(hits["ok"], true, "got {hits}");
        let results = hits["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["path"], "src/main.rs");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unique suffix without extra dev-dependencies.
    fn uuid_bin() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{}-{}", std::process::id(), nanos)
    }
}
