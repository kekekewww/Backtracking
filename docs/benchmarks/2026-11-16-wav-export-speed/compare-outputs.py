"""Compares the files the output check wrote with each build.

    python compare-outputs.py <base-folder> <base-lines.jsonl> <change-folder> <change-lines.jsonl>

The .jsonl arguments are the lines output-check printed for each build. Writes
output-comparison.jsonl next to this script, one line per file: the case, its size, and
its SHA-256, or both builds' SHA-256 where they differ. Prints a summary and exits with
status 1 if any file differs or is missing.
"""

import hashlib
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as data:
        for block in iter(lambda: data.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def lines(name):
    # PowerShell may add a byte-order mark to each block of lines it writes.
    with open(name, encoding="utf-8-sig") as rows:
        return [json.loads(row.lstrip("﻿")) for row in rows if row.strip()]


def main():
    base_dir, base_lines, change_dir, change_lines = map(Path, sys.argv[1:5])
    base = lines(base_lines)
    change = lines(change_lines)
    if base != change:
        sys.exit("the builds wrote different file lists or sizes")
    listed = {row["file"] for row in base}
    for folder in (base_dir, change_dir):
        present = {path.name for path in folder.iterdir()}
        if present != listed:
            sys.exit(f"{folder}: files on disk do not match the printed list")
    different = 0
    total = 0
    with open(HERE / "output-comparison.jsonl", "w", encoding="utf-8", newline="\n") as out:
        for row in base:
            base_hash = sha256(base_dir / row["file"])
            change_hash = sha256(change_dir / row["file"])
            total += row["bytes"]
            if base_hash == change_hash:
                row = {**row, "identical": True, "sha256": base_hash}
            else:
                different += 1
                row = {**row, "identical": False, "base_sha256": base_hash, "change_sha256": change_hash}
            out.write(json.dumps(row, separators=(",", ":")) + "\n")
    cases = len({row["case"] for row in base})
    print(
        f"{cases} exports, {len(base)} files and {total:,} bytes per build; "
        f"{len(base) - different} identical, {different} different"
    )
    sys.exit(1 if different else 0)


if __name__ == "__main__":
    main()
