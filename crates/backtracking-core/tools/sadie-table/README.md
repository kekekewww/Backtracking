# SADIE II D1 table generator

This offline, dependency-free Cargo project reads a pinned OpenJOC `.ojhrtf`
file and writes the reduced table used by `backtracking-core::spatial`.
It has its own workspace and is never run by an application build or test.
See [THIRD_PARTY_NOTICES.md](../../../../THIRD_PARTY_NOTICES.md) for attribution,
the complete transformation record, hashes, sample format, and error figures.

From the repository root:

```powershell
cargo +1.95.0 run --release --manifest-path crates/backtracking-core/tools/sadie-table/Cargo.toml -- D:\Developing\OpenJOC\crates\openjoc-sofa\assets\sadie-ii-d1-ku100.ojhrtf crates/backtracking-core/src/spatial/sadie-d1.bin
cargo +1.95.0 test --manifest-path crates/backtracking-core/tools/sadie-table/Cargo.toml
cargo +1.100.0 fmt --manifest-path crates/backtracking-core/tools/sadie-table/Cargo.toml -- --check
cargo +1.100.0 clippy --manifest-path crates/backtracking-core/tools/sadie-table/Cargo.toml --all-targets -- -D warnings
```

The input path may point to any copy of the pinned source. The tool refuses
a different SHA-256, validates the envelope and records, ignores the 15
appended aliases, compares two regular grids against the full measured set,
and prints the output hash. It does not fetch data or play audio.
