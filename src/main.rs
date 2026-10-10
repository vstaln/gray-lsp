//! lsp — batch diagnostics: run the right checker for the project
//! (cargo check, tsc --noEmit, pyright, ruff, go vet, golangci-lint)
//! and return normalized `file:line:col severity message` lines.
//! Detection order: marker files (Cargo.toml/tsconfig/pyproject/go.mod)
//! → first available checker binary.

use std::io::{BufRead, Write};
use std::path::Path;
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
            {"name": "lsp_check",
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

fn run_at(bin: &str, args: &[&str], dir: &Path) -> Result<String, String> {
    let mut child = Command::new(bin).args(args).current_dir(dir)
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("spawn {bin}: {e}"))?;
    let t0 = Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            let o = child.wait_with_output().map_err(|e| e.to_string())?;
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            let e = String::from_utf8_lossy(&o.stderr);
            if !e.trim().is_empty() { s.push('\n'); s.push_str(&e); }
            return Ok(s);
        }
        if t0.elapsed() > TIMEOUT {
            let _ = child.kill();
            return Err(format!("{bin} timed out at {}s", TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

/// (marker file, [(binary, args, label) tried in order])
fn detect(dir: &Path) -> Vec<(&'static str, &'static str, &'static [&'static str])> {
    let mut v = Vec::new();
    if dir.join("Cargo.toml").exists() {
        v.push(("cargo", "cargo", &["check", "--quiet", "--message-format", "short"][..]));
    }
    if dir.join("tsconfig.json").exists() || dir.join("package.json").exists() {
        v.push(("tsc", "tsc", &["--noEmit", "--pretty", "false"][..]));
    }
    if dir.join("pyproject.toml").exists() || dir.join("setup.py").exists()
        || dir.join("requirements.txt").exists() {
        v.push(("pyright", "pyright", &["--outputjson"][..]));
        v.push(("ruff", "ruff", &["check", "--output-format", "concise"][..]));
    }
    if dir.join("go.mod").exists() {
        v.push(("govet", "go", &["vet", "./..."][..]));
    }
    v
}

fn lsp_check(a: &Value) -> Result<String, String> {
    let dir = a["path"].as_str().map(Path::new).map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let pin = a["tool"].as_str().unwrap_or("auto");
    let candidates = detect(&dir);
    if candidates.is_empty() && pin == "auto" {
        return Err(format!("no project markers in {}", dir.display()));
    }
    let mut tried = Vec::new();
    for (label, bin, args) in candidates {
        if pin != "auto" && pin != label { continue; }
        tried.push(label);
        if !have(bin) { continue; }
        let out = run_at(bin, args, &dir)?;
        let lines: Vec<&str> = out.lines()
            .filter(|l| l.contains(": error") || l.contains(": warning")
                || l.contains("error:") || l.contains("warning:"))
            .collect();
        let summary = if lines.is_empty() {
            format!("{label}: clean ({})", out.lines().next().unwrap_or("no output"))
        } else {
            format!("{label}: {} findings\n{}", lines.len(),
                lines.iter().take(80).cloned().collect::<Vec<_>>().join("\n"))
        };
        return Ok(summary);
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
                    let r = match v["params"]["name"].as_str().unwrap_or("") {
                        "lsp_check" => match lsp_check(&v["params"]["args"]) {
                            Ok(c) => json!({"result": {"content": c}}),
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
        Some("sidecar") => { let _ = sidecar(); }
        Some("check") => {
            match lsp_check(&json!({})) { Ok(t) => println!("{t}"),
                Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
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
