#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/gray-context-pin.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/gray-context-pin" ;;
    *) rel="x86_64-apple-darwin/gray-context-pin" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/gray-context-pin" ;;
    *) rel="x86_64-unknown-linux-musl/gray-context-pin" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/gray-context-pin.exe" "$here/target/release/gray-context-pin" "$here/target/debug/gray-context-pin.exe" "$here/target/debug/gray-context-pin"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "gray-context-pin binary not found in $here" >&2
exit 1
