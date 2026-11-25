#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
case "$(uname -m)" in
arm64) arch=aarch64 ;;
x86_64) arch=x86_64 ;;
*) exit 2 ;;
esac
target="$arch-apple-darwin"
build="${CARGO_TARGET_DIR:-$PWD/target}"
mkdir -p "$build"
stage=$(mktemp -d "$build/agent-package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
cargo build --release --locked --target "$target" --target-dir "$build"
mkdir "$stage/bin"
cp "$build/$target/release/backtracking-agent" "$stage/bin/"
cp ../../LICENSE "$stage/LICENSE.txt"
python3 - "$arch" package/addon.json "$stage/addon.json" <<'PY'
import json, sys
with open(sys.argv[2]) as f:
    manifest = json.load(f)
manifest["run"] = {"macos-" + sys.argv[1]: {"program": "bin/backtracking-agent", "args": []}}
with open(sys.argv[3], "w") as f:
    json.dump(manifest, f, indent=2)
PY
python3 -I licenses.py "$stage/THIRD_PARTY_LICENSES.md" --target "$target"
cargo run --locked --quiet --manifest-path ../../crates/backtracking-addon/Cargo.toml --features install --bin btaddon-pack -- "$stage" "${1:-$build/backtracking-agent.btaddon}"
