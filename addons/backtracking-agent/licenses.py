"""Writes THIRD_PARTY_LICENSES.md for the Backtracking Agent package.

usage: python licenses.py <output.md> [--list]

Takes the crates `cargo tree -e normal` lists for this crate on x86_64-pc-windows-msvc (what
the program is built from, without dev- or build-only crates and with the features a release
build uses), finds them with `cargo metadata`, and copies every licence file each
crate ships (LICENSE*, LICENCE*, COPYING*, NOTICE*, UNLICENSE*), with identical texts written
once. Crates that ship no licence file are listed at the end, so that they can be checked by
hand. --list prints one line per crate (name, version, licence) instead.
"""
import argparse
import hashlib
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PREFIXES = ("license", "licence", "copying", "notice", "unlicense")


def crates(target):
    tree = subprocess.run(
        ["cargo", "tree", "--locked", "-e", "normal", "--target", target,
         "--prefix", "none", "-f", "{p}"],
        cwd=HERE, check=True, capture_output=True, text=True).stdout
    built = set()
    for line in tree.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            built.add((parts[0], parts[1][1:]))
    metadata = json.loads(subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1",
         "--filter-platform", target],
        cwd=HERE, check=True, capture_output=True).stdout)
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root = next(pid for pid, package in packages.items() if package["name"] == "backtracking-agent")
    seen, stack = set(), [root]
    while stack:
        current = stack.pop()
        for dep in nodes[current]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                stack.append(dep["pkg"])
    chosen = [packages[pid] for pid in seen if (packages[pid]["name"], packages[pid]["version"]) in built]
    return sorted(chosen, key=lambda p: (p["name"], p["version"]))


def licence_files(package):
    folder = os.path.dirname(package["manifest_path"])
    files = []
    for name in sorted(os.listdir(folder)):
        path = os.path.join(folder, name)
        if os.path.isfile(path) and name.lower().startswith(PREFIXES):
            files.append(path)
    if package.get("license_file"):
        path = os.path.join(folder, package["license_file"])
        if os.path.isfile(path) and path not in files:
            files.append(path)
    return files


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output")
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--target", default="x86_64-pc-windows-msvc")
    args = parser.parse_args()
    output, listing = args.output, args.list
    found = crates(args.target)
    if listing:
        for package in found:
            print(f"{package['name']} {package['version']}  {package.get('license') or package.get('license_file')}")
        return
    texts = {}  # hash -> (text, [crate names])
    missing = []
    for package in found:
        label = f"{package['name']} {package['version']}"
        if package["name"] == "backtracking-addon":
            continue  # Backtracking's own SDK, under Backtracking's licence.
        files = licence_files(package)
        if not files:
            missing.append(f"{label} ({package.get('license')})")
        for path in files:
            with open(path, encoding="utf-8", errors="replace") as source:
                text = source.read().replace("\r\n", "\n").strip()
            digest = hashlib.sha256(text.encode()).hexdigest()
            texts.setdefault(digest, (text, []))[1].append(f"{label} ({os.path.basename(path)})")
    with open(output, "w", encoding="utf-8", newline="\n") as out:
        out.write("# Third-party licences of Backtracking Agent\n\n")
        out.write("Backtracking Agent is built from the Rust crates below. Each licence text is "
                  "given once, after the crates it comes from.\n\n")
        out.write("| Crate | Version | Licence |\n| --- | --- | --- |\n")
        for package in found:
            if package["name"] == "backtracking-addon":
                continue
            out.write(f"| {package['name']} | {package['version']} | "
                      f"{(package.get('license') or 'see file').replace('|', '/')} |\n")
        for text, names in texts.values():
            out.write("\n---\n\n")
            out.write("From: " + ", ".join(names) + "\n\n")
            out.write("```text\n" + text.replace("```", "'''") + "\n```\n")
        if missing:
            out.write("\n---\n\nThese crates ship no licence file; their licence is the one "
                      "named in their Cargo.toml, whose standard text appears above:\n\n")
            for label in missing:
                out.write(f"- {label}\n")
    print(f"{output}: {len(found) - 1} crates, {len(texts)} distinct texts, "
          f"{len(missing)} without a licence file")
    for label in missing:
        print("  no licence file:", label)


if __name__ == "__main__":
    main()
