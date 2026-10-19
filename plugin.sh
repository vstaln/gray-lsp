#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/lsp.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/lsp" ;;
    *) rel="x86_64-apple-darwin/lsp" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/lsp" ;;
    *) rel="x86_64-unknown-linux-musl/lsp" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/lsp.exe" "$here/target/release/lsp" "$here/target/debug/lsp.exe" "$here/target/debug/lsp"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "lsp binary not found in $here" >&2
exit 1
