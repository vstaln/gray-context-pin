//! gray-context-pin — pin files or notes into the prompt on every turn.
//!
//! Port of the context-pin idea: `/pin <path-or-note>` records a pin in the
//! session store, and the `prompt/context` hook re-injects every pin on every
//! turn, so context compaction can never drop them. `/unpin <n>` removes a
//! pin by its `/pins` index; `/pins` lists them.
//!
//! `/pin <arg>` is a FILE pin when `arg` resolves to an existing file
//! (relative to the session cwd), otherwise a NOTE pin holding the literal
//! text. File pins re-read the file at injection time, so a pin always shows
//! the current contents (capped at FILE_CAP bytes).
//!
//! Pins persist per session at `~/.gray/context-pin/<sha1(session)>.json`,
//! keyed by `session.id` (falling back to `session.cwd`).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Per-file injection cap — a pin protects small anchors, not whole files.
const FILE_CAP: usize = 24 * 1024;

fn manifest() -> Value {
    json!({
        "name": "context-pin",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "tools": [],
        "commands": ["/pin", "/unpin", "/pins"],
        "hooks": ["prompt/context"],
    })
}

fn state_dir() -> PathBuf {
    std::env::var_os("GRAY_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".gray")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("context-pin")
}

/// Session key: `session.id`, else `session.cwd`, else "default".
fn session_key(params: &Value) -> String {
    let s = params.get("session").cloned().unwrap_or(Value::Null);
    s.get("id")
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .or_else(|| s.get("cwd").and_then(Value::as_str))
        .unwrap_or("default")
        .to_string()
}

fn session_cwd(params: &Value) -> String {
    params
        .pointer("/session/cwd")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| ".".into())
}

fn state_path(key: &str) -> PathBuf {
    state_dir().join(format!("{}.json", key_file(key)))
}

#[derive(Clone, PartialEq)]
enum Pin {
    File(String),
    Note(String),
}

fn load(path: &Path) -> Vec<Pin> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    v.get("pins")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let value = p.get("value").and_then(Value::as_str)?.to_string();
                    match p.get("kind").and_then(Value::as_str) {
                        Some("file") => Some(Pin::File(value)),
                        Some("note") => Some(Pin::Note(value)),
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save(path: &Path, pins: &[Pin]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let pins: Vec<Value> = pins
        .iter()
        .map(|p| match p {
            Pin::File(v) => json!({"kind": "file", "value": v}),
            Pin::Note(v) => json!({"kind": "note", "value": v}),
        })
        .collect();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json!({"pins": pins}).to_string())?;
    std::fs::rename(&tmp, path)
}

/// Resolve `arg` as a path: absolute, `~/…`, or relative to `cwd`.
fn resolve_path(arg: &str, cwd: &str) -> PathBuf {
    if let Some(rest) = arg.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    let p = PathBuf::from(arg);
    if p.is_absolute() { p } else { Path::new(cwd).join(p) }
}

/// `/pin <path-or-note>`: existing file → file pin, anything else → note.
fn pin(arg: &str, params: &Value) -> String {
    if arg.is_empty() {
        return "usage: /pin <path-or-note>".into();
    }
    let path = state_path(&session_key(params));
    let mut pins = load(&path);
    let resolved = resolve_path(arg, &session_cwd(params));
    let new = if resolved.is_file() {
        Pin::File(resolved.to_string_lossy().into_owned())
    } else {
        Pin::Note(arg.to_string())
    };
    if pins.contains(&new) {
        return "already pinned".into();
    }
    let msg = match &new {
        Pin::File(p) => format!("pinned file {p}"),
        Pin::Note(t) => format!("pinned note \"{}\"", truncate(t, 60)),
    };
    pins.push(new);
    match save(&path, &pins) {
        Ok(()) => format!("{msg} (#{})", pins.len()),
        Err(e) => format!("couldn't save pins: {e}"),
    }
}

fn unpin(arg: &str, params: &Value) -> String {
    let path = state_path(&session_key(params));
    let mut pins = load(&path);
    if arg == "all" {
        let n = pins.len();
        pins.clear();
        return match save(&path, &pins) {
            Ok(()) => format!("unpinned {n}"),
            Err(e) => format!("couldn't save pins: {e}"),
        };
    }
    let Ok(n) = arg.parse::<usize>() else {
        return "usage: /unpin <n|all> — see /pins".into();
    };
    if n == 0 || n > pins.len() {
        return format!("no pin #{n} — see /pins");
    }
    let gone = pins.remove(n - 1);
    match save(&path, &pins) {
        Ok(()) => format!(
            "unpinned #{} {}",
            n,
            match &gone {
                Pin::File(p) => format!("file {p}"),
                Pin::Note(t) => format!("note \"{}\"", truncate(t, 60)),
            }
        ),
        Err(e) => format!("couldn't save pins: {e}"),
    }
}

fn list(params: &Value) -> String {
    let pins = load(&state_path(&session_key(params)));
    if pins.is_empty() {
        return "no pins — /pin <path-or-note>".into();
    }
    let mut out = String::new();
    for (i, p) in pins.iter().enumerate() {
        out.push_str(&match p {
            Pin::File(f) => format!("#{} file {}\n", i + 1, f),
            Pin::Note(t) => format!("#{} note \"{}\"\n", i + 1, truncate(t, 60)),
        });
    }
    out.trim_end().to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// `prompt/context` — re-inject every pin so compaction can't drop it.
fn inject(params: &Value) -> Value {
    let pins = load(&state_path(&session_key(params)));
    if pins.is_empty() {
        return json!({});
    }
    let mut text = String::from("## Pinned context (re-injected every turn)\n");
    for p in &pins {
        match p {
            Pin::Note(t) => {
                text.push_str(&format!("\n- {t}\n"));
            }
            Pin::File(f) => {
                text.push_str(&format!("\n### `{f}` (pinned file)\n"));
                match std::fs::read(f) {
                    Ok(bytes) => {
                        let cut = bytes.len() > FILE_CAP;
                        let body = String::from_utf8_lossy(&bytes[..bytes.len().min(FILE_CAP)]);
                        text.push_str(&format!("````\n{body}\n````\n"));
                        if cut {
                            text.push_str(&format!("… truncated at {FILE_CAP} bytes\n"));
                        }
                    }
                    Err(e) => text.push_str(&format!("(pinned file unavailable: {e})\n")),
                }
            }
        }
    }
    json!({ "text": text })
}

fn run_command(name: &str, argv: &[&str], params: &Value) -> String {
    let arg = argv.join(" ");
    match name {
        "/pin" => pin(arg.trim(), params),
        "/unpin" => unpin(arg.trim(), params),
        "/pins" => {
            if arg.is_empty() { list(params) } else { "usage: /pins".into() }
        }
        _ => "usage: /pin <path-or-note> · /unpin <n|all> · /pins".into(),
    }
}

/// One request → `Some(reply)`, or `None` for notifications. The bool asks
/// the loop to exit after writing the reply.
fn handle(req: &Value) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        return (None, method == "plugin/shutdown");
    };
    let result = match method {
        "plugin/manifest" => manifest(),
        "prompt/context" => inject(&params),
        "command/run" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let argv: Vec<&str> = params
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            json!({ "text": run_command(name, &argv, &params) })
        }
        "plugin/shutdown" => return (Some(json!({ "id": id, "result": {} })), true),
        _ => {
            let error = json!({ "code": -32601, "message": "method not found" });
            return (Some(json!({ "id": id, "error": error })), false);
        }
    };
    (Some(json!({ "id": id, "result": result })), false)
}

/// Sidecar entry: `gray-context-pin manifest` prints the manifest, otherwise
/// serve JSON-lines requests on stdin until EOF or `plugin/shutdown`.
pub fn run() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest());
        return Ok(());
    }
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (reply, exit) = handle(&req);
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
        if exit {
            break;
        }
    }
    Ok(())
}

/// Filename-safe session key: sanitized prefix plus a `DefaultHasher`
/// suffix so different keys can't collide. The hash is per-build-stable,
/// which is all a state filename needs.
fn key_file(key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    let clean: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .take(48)
        .collect();
    format!("{}-{:016x}", clean.trim_matches('.'), h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());
    static CTR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Run `body` with GRAY_HOME pointed at a fresh temp dir. Serialized:
    /// the env var is process-global and tests run in one process.
    fn with_home(body: impl FnOnce(&Path)) {
        let _g = LOCK.lock().unwrap();
        let n = CTR.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("gray-context-pin-test-{n}"));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("GRAY_HOME", &home) };
        body(&home);
        let _ = std::fs::remove_dir_all(&home);
    }

    fn call(method: &str, params: Value) -> Value {
        handle(&json!({ "id": 1, "method": method, "params": params }))
            .0
            .unwrap()
    }

    fn cmd(name: &str, argv: &[&str], session: Value) -> Value {
        call(
            "command/run",
            json!({"name": name, "argv": argv, "session": session}),
        )
    }

    fn sess() -> Value {
        json!({"id": "s1", "cwd": "/tmp/pin-proj"})
    }

    #[test]
    fn manifest_claims_commands_and_context_hook() {
        let m = call("plugin/manifest", Value::Null)["result"].clone();
        assert_eq!(m["name"], "context-pin");
        assert_eq!(m["protocol"], "1.1");
        assert_eq!(m["commands"], json!(["/pin", "/unpin", "/pins"]));
        assert_eq!(m["hooks"], json!(["prompt/context"]));
    }

    #[test]
    fn note_pin_roundtrip_and_inject() {
        with_home(|_| {
            let r = cmd("/pin", &["remember the deploy key rotates"], sess());
            assert!(r["result"]["text"].as_str().unwrap().contains("pinned note"));

            let r = call("prompt/context", json!({"session": sess()}));
            let text = r["result"]["text"].as_str().unwrap();
            assert!(text.contains("Pinned context"));
            assert!(text.contains("remember the deploy key rotates"));

            let r = cmd("/pins", &[], sess());
            assert!(r["result"]["text"].as_str().unwrap().contains("#1 note"));

            let r = cmd("/unpin", &["1"], sess());
            assert!(r["result"]["text"].as_str().unwrap().contains("unpinned #1"));
            let r = call("prompt/context", json!({"session": sess()}));
            assert_eq!(r["result"], json!({}));
        });
    }

    #[test]
    fn path_arg_becomes_file_pin_and_reads_live() {
        with_home(|home| {
            let proj = home.join("proj");
            std::fs::create_dir_all(&proj).unwrap();
            std::fs::write(proj.join("spec.md"), "v1 content").unwrap();
            let s = json!({"id": "s9", "cwd": proj.to_string_lossy()});

            let r = cmd("/pin", &["spec.md"], s.clone());
            assert!(r["result"]["text"].as_str().unwrap().contains("pinned file"));

            std::fs::write(proj.join("spec.md"), "v2 changed").unwrap();
            let r = call("prompt/context", json!({"session": s}));
            let text = r["result"]["text"].as_str().unwrap();
            assert!(text.contains("v2 changed") && !text.contains("v1 content"));
        });
    }

    #[test]
    fn pins_are_scoped_per_session() {
        with_home(|_| {
            cmd("/pin", &["for-a"], json!({"id": "a", "cwd": "/x"}));
            cmd("/pin", &["for-b"], json!({"id": "b", "cwd": "/x"}));
            let a = call("prompt/context", json!({"session": {"id": "a", "cwd": "/x"}}));
            let text = a["result"]["text"].as_str().unwrap();
            assert!(text.contains("for-a") && !text.contains("for-b"));
        });
    }

    #[test]
    fn unpin_all_and_bad_index() {
        with_home(|_| {
            cmd("/pin", &["one"], sess());
            cmd("/pin", &["two"], sess());
            let r = cmd("/unpin", &["9"], sess());
            assert!(r["result"]["text"].as_str().unwrap().contains("no pin #9"));
            let r = cmd("/unpin", &["all"], sess());
            assert!(r["result"]["text"].as_str().unwrap().contains("unpinned 2"));
            assert!(cmd("/pins", &[], sess())["result"]["text"]
                .as_str()
                .unwrap()
                .contains("no pins"));
        });
    }

    #[test]
    fn shutdown_replies_then_exits() {
        let (reply, exit) = handle(&json!({ "id": 2, "method": "plugin/shutdown" }));
        assert!(reply.is_some() && exit);
        let (reply, exit) = handle(&json!({ "method": "plugin/shutdown" }));
        assert!(reply.is_none() && exit);
    }
}
