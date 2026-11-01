#!/bin/sh
# Builds the TUI web demo (crates/tui-web) into OUT_DIR as wasm-bindgen's
# `--target web` output, and packs it as OUT_DIR/devlanes-tui-web.tar.gz with
# the files at the archive root, which the website's sync unpacks into
# public/tui/.
#
# Needs the wasm32-unknown-unknown target and wasm-bindgen-cli at the version
# Cargo.lock pins for wasm-bindgen: the CLI refuses a module built by another
# version. wasm-opt (binaryen) shrinks the module when it is on PATH.
set -eu

if [ $# -ne 1 ] || [ -z "$1" ]; then
	echo "usage: $0 OUT_DIR" >&2
	exit 2
fi
out=$1
root=$(cd "$(dirname "$0")/../.." && pwd)

# The first `version` after wasm-bindgen's own entry in the lock file; the
# `tui-web` jobs in .github/workflows read it the same way.
locked=$(awk '/^name = "wasm-bindgen"$/ { found = 1; next } found && /^version = / { gsub(/"/, "", $3); print $3; exit }' "$root/Cargo.lock")
if [ -z "$locked" ]; then
	echo "wasm-bindgen is not in Cargo.lock" >&2
	exit 1
fi
installed=$(wasm-bindgen --version 2>/dev/null | cut -d ' ' -f 2 || true)
if [ "$installed" != "$locked" ]; then
	echo "wasm-bindgen-cli $locked is needed, found '${installed:-none}':" >&2
	echo "  cargo install wasm-bindgen-cli --version $locked --locked" >&2
	exit 1
fi

cargo build --manifest-path "$root/Cargo.toml" -p devlanes-tui-web \
	--target wasm32-unknown-unknown --profile web --locked

module=${CARGO_TARGET_DIR:-$root/target}/wasm32-unknown-unknown/web/devlanes_tui_web.wasm
staging=$out/devlanes-tui-web
rm -rf "$staging"
mkdir -p "$staging"
wasm-bindgen --target web --out-dir "$staging" --out-name devlanes_tui_web "$module"

wasm=$staging/devlanes_tui_web_bg.wasm
if command -v wasm-opt >/dev/null 2>&1; then
	# wasm-bindgen strips the target features list, so name the ones rustc
	# enables by default for wasm32 or wasm-opt rejects their instructions.
	wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
		--enable-sign-ext --enable-mutable-globals --enable-reference-types \
		--enable-multivalue -o "$wasm.opt" "$wasm"
	mv "$wasm.opt" "$wasm"
else
	echo "wasm-opt not found; the module stays unoptimized" >&2
fi

# Plain names, not `./name`: the website takes only files at the archive root.
(cd "$staging" && tar -czf ../devlanes-tui-web.tar.gz -- *)
ls -l "$wasm" "$out/devlanes-tui-web.tar.gz"
