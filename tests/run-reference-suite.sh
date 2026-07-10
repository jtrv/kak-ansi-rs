#!/bin/sh
# Run the original kak-ansi bash test suite against the Rust binary.
# Usage: tests/run-reference-suite.sh [path-to-kak-ansi-checkout]
set -eu
cd "$(dirname "$0")/.."
ref=${1:-../kak-ansi}
if ! [ -f "$ref/tests/tests.bash" ]; then
    echo "reference checkout not found at '$ref'" >&2
    exit 1
fi
cargo build --release
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp -r "$ref/tests" "$tmp/tests"
# functions.bash runs `make` before testing; stub it out
printf 'all:\n\t@true\n' > "$tmp/Makefile"
ln -s "$(pwd)/target/release/kak-ansi-filter" "$tmp/kak-ansi-filter"
cd "$tmp" && exec bash tests/tests.bash
