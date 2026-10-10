<div align="center">
  <img alt="gray-lsp" src="assets/icon.svg" width="120" height="120" />
  <h1>gray-lsp</h1>
  <p><strong>Batch diagnostics: run the project's real checker and get normalized file:line findings.</strong></p>
  <p>
    <a href="https://gray.alignment.id">Website</a> ·
    <a href="https://github.com/vstaln/gray-lsp">Source</a> ·
    <a href="https://github.com/vstaln/gray">gray</a>
  </p>
  <p>
    <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-1c1c20?style=flat-square&labelColor=0a0a0b" /></a>
    <a href="https://www.rust-lang.org"><img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-rust-1c1c20?style=flat-square&labelColor=0a0a0b&logo=rust&logoColor=d4a373" /></a>
    <img alt="gray plugin" src="https://img.shields.io/badge/gray-plugin-1c1c20?style=flat-square&labelColor=0a0a0b&color=7aa2f7" />
  </p>
</div>

<br/>
```bash
gray plugin install lsp
```

`gray-lsp` gives the agent one tool — `lsp_check` — that picks the right
checker for a project and returns normalized `file:line:col severity
message` lines. Batch, not a persistent language-server session.

## Detection

Marker file first, then first available checker binary:

| Marker | Checkers tried |
|---|---|
| `Cargo.toml` | `cargo check` |
| `tsconfig.json` / `package.json` | `tsc --noEmit` |
| `pyproject.toml` / `setup.py` | `pyright`, `ruff` |
| `go.mod` | `go vet`, `golangci-lint` |

No marker or no checker → an honest error saying what it looked for.

## Tool

| Tool | What it does |
|---|---|
| `lsp_check` | `path` → findings list (`file:line:col severity message`). |

## Build from source

```sh
cargo build --release
gray plugin check ./target/release/lsp
```

## License

MIT
