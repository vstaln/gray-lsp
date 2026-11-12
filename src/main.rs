//! lsp — batch diagnostics: run the right checker for the project
//! (cargo check, tsc --noEmit, pyright, ruff, go vet, golangci-lint)
//! and return normalized `file:line:col severity message` lines.
//! Detection order: marker files (Cargo.toml/tsconfig/pyproject/go.mod)
//! → first available checker binary.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use serde_json::{json, Value};

const NAME: &str = "lsp";
const TIMEOUT: Duration = Duration::from_secs(180);

fn manifest() -> Value {
    json!({
        "name": NAME, "display_name": "LSP",
        "version": env!("CARGO_PKG_VERSION"), "protocol": "1.1",
        "tools": [
            {"name": "lsp_check", "label": "Checked", "preview": "{path} · {tool}",
             "description": "Run the project's type/diagnostic checker (cargo check | tsc --noEmit | pyright | ruff check | go vet) and return file:line:col findings. Auto-detects from marker files; `tool` pins a checker.",
             "parameters": {"type": "object", "properties": {
                "path": {"type": "string", "description": "project dir (default cwd)"},
                "tool": {"type": "string",
                    "enum": ["auto","cargo","tsc","pyright","ruff","govet","clippy"],
                    "description": "auto (default) picks from marker files"}},
             }},
        ],
        "commands": ["/lsp"], "subcommands": ["lsp"],
    })
}

fn have(b: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {b}")])
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

/// Runs `bin` and returns (merged output, exit success). stdout/stderr are
/// drained on reader threads so a noisy child can't fill a pipe buffer and
/// deadlock against our try_wait poll.
fn run_at(bin: &str, args: &[&str], dir: &Path) -> Result<(String, bool), String> {
    use std::io::Read;
    let mut child = Command::new(bin).args(args).current_dir(dir)
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("spawn {bin}: {e}"))?;
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut f) = out_pipe.take() { let _ = f.read_to_string(&mut s); }
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut f) = err_pipe.take() { let _ = f.read_to_string(&mut s); }
        s
    });
    let t0 = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let mut s = t_out.join().unwrap_or_default();
            let e = t_err.join().unwrap_or_default();
            if !e.trim().is_empty() { s.push('\n'); s.push_str(&e); }
            return Ok((s, status.success()));
        }
        if t0.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = t_out.join();
            let _ = t_err.join();
            return Err(format!("{bin} timed out at {}s", TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

/// label → (binary, args). Single source of truth for detect() and `tool:` pins.
fn spec(label: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match label {
        "cargo" => ("cargo", &["check", "--quiet", "--message-format", "short"][..]),
        "clippy" => ("cargo", &["clippy", "--quiet", "--message-format", "short"][..]),
        "tsc" => ("tsc", &["--noEmit", "--pretty", "false"][..]),
        "pyright" => ("pyright", &["--outputjson"][..]),
        "ruff" => ("ruff", &["check", "--output-format", "concise"][..]),
        "govet" => ("go", &["vet", "./..."][..]),
        _ => return None,
    })
}

/// (marker file, [(binary, args, label) tried in order])
fn detect(dir: &Path) -> Vec<&'static str> {
    let mut v = Vec::new();
    if dir.join("Cargo.toml").exists() {
        v.push("cargo");
    }
    if dir.join("tsconfig.json").exists() || dir.join("package.json").exists() {
        v.push("tsc");
    }
    if dir.join("pyproject.toml").exists() || dir.join("setup.py").exists()
        || dir.join("requirements.txt").exists() {
        v.push("pyright");
        v.push("ruff");
    }
    if dir.join("go.mod").exists() {
        v.push("govet");
    }
    v
}

/// (text, findings?): the CLI `check` verb exits non-zero on findings.
fn lsp_check(a: &Value, session_cwd: &Path) -> Result<(String, bool), String> {
    let dir = match a["path"].as_str() {
        Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
        Some(p) => session_cwd.join(p),
        None => session_cwd.to_path_buf(),
    };
    let pin = a["tool"].as_str().unwrap_or("auto");
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    let candidates: Vec<String> = if pin == "auto" {
        let d = detect(&dir);
        if d.is_empty() {
            return Err(format!("no project markers in {}", dir.display()));
        }
        d.iter().map(|s| s.to_string()).collect()
    } else {
        if spec(pin).is_none() {
            return Err(format!(
                "unknown tool '{pin}' — pick auto|cargo|clippy|tsc|pyright|ruff|govet"));
        }
        vec![pin.to_string()]
    };
    let mut tried = Vec::new();
    for label in &candidates {
        let Some((bin, args)) = spec(label) else { continue };
        tried.push(label.as_str());
        if !have(bin) { continue; }
        let (out, ok) = run_at(bin, args, &dir)?;
        let lines: Vec<&str> = out.lines()
            .filter(|l| l.contains(": error") || l.contains(": warning")
                || l.contains("error:") || l.contains("warning:"))
            .collect();
        if !lines.is_empty() {
            return Ok((format!("{label}: {} findings\n{}", lines.len(),
                lines.iter().take(80).cloned().collect::<Vec<_>>().join("\n")), true));
        }
        if !ok {
            let tail: Vec<&str> = out.lines().collect();
            let start = tail.len().saturating_sub(10);
            return Err(format!(
                "{label} exited non-zero with no parseable diagnostics:\n{}",
                tail[start..].join("\n")));
        }
        return Ok((format!("{label}: clean ({})", out.lines().next().unwrap_or("no output")), false));
    }
    Err(format!("no usable checker for {} (tried: {})",
        dir.display(), tried.join(", ")))
}

fn status() -> String {
    format!("lsp\n  checkers available: {}",
        [("cargo",have("cargo")), ("tsc",have("tsc")), ("pyright",have("pyright")),
         ("ruff",have("ruff")), ("go",have("go")), ("clippy",have("cargo-clippy"))]
        .iter().filter(|(_,a)| *a).map(|(n,_)| *n).collect::<Vec<_>>().join(", "))
}

fn sidecar() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let method = v["method"].as_str().unwrap_or("");
        match v.get("id").cloned() {
            None if method == "plugin/shutdown" => return Ok(()),
            None => {}
            Some(id) => {
                let reply = if method == "plugin/manifest" {
                    json!({"id": id, "result": manifest()})
                } else if method == "tool/call" {
                    let scwd = v["params"]["session"]["cwd"].as_str()
                        .map(std::path::PathBuf::from)
                        .or_else(|| std::env::current_dir().ok())
                        .unwrap_or_else(|| PathBuf::from("."));
                    let r = match v["params"]["name"].as_str().unwrap_or("") {
                        "lsp_check" => match lsp_check(&v["params"]["args"], &scwd) {
                            Ok((c, _)) => json!({"result": {"content": c}}),
                            Err(e) => json!({"result": {"content": e, "is_error": true}}),
                        },
                        t => json!({"result": {"content": format!("unknown tool {t}"),
                                    "is_error": true}}),
                    };
                    let mut r = r; r["id"] = id; r
                } else if method == "command/run" {
                    json!({"id": id, "result": {"text": status()}})
                } else {
                    json!({"id": id, "error": {"code": -32601,
                        "message": format!("method not found: {method}")}})
                };
                writeln!(out, "{reply}")?; out.flush()?;
            }
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<String> = match args.first().map(String::as_str) {
        Some("lsp") => args[1..].to_vec(), _ => args };
    match args.first().map(String::as_str) {
        Some("manifest") => println!("{}", manifest()),
        Some("-h" | "--help" | "help") => println!("{}", status()),
        Some("sidecar") => { let _ = sidecar(); }
        Some("check") => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match lsp_check(&json!({}), &cwd) {
                Ok((t, found)) => { println!("{t}"); if found { std::process::exit(1); } }
                Err(e) => { eprintln!("{e}"); std::process::exit(1); }
            }
        }
        _ => {
            if args.is_empty() {
                if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    println!("{}", status()); return;
                }
                let _ = sidecar(); return;
            }
            eprintln!("unknown verb"); std::process::exit(1);
        }
    }
}
