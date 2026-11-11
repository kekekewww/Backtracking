#!/usr/bin/env python3
"""Create Backtracking's signed GitHub Release update sidecars using OpenSSL."""

import argparse
import hashlib
import json
import re
import subprocess
import tempfile
from pathlib import Path
from zipfile import is_zipfile


PLATFORMS = {
    "windows-x64": (".exe", 64 * 1024 * 1024),
    "macos-arm64": (".zip", 128 * 1024 * 1024),
    "macos-x64": (".zip", 128 * 1024 * 1024),
}
ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")


def public_key(private_key: Path) -> str:
    der = subprocess.check_output(
        ["openssl", "pkey", "-in", str(private_key), "-pubout", "-outform", "DER"]
    )
    if not der.startswith(ED25519_SPKI_PREFIX) or len(der) != len(ED25519_SPKI_PREFIX) + 32:
        raise ValueError("The private key must be Ed25519.")
    return der[len(ED25519_SPKI_PREFIX):].hex()


def check_artifact(path: Path, suffix: str) -> None:
    if suffix == ".zip":
        if not is_zipfile(path):
            raise ValueError("The macOS artifact must be a ZIP archive.")
        return
    with path.open("rb") as stream:
        if stream.read(2) != b"MZ":
            raise ValueError("The Windows artifact must be a PE executable.")
        stream.seek(0x3C)
        offset = int.from_bytes(stream.read(4), "little")
        stream.seek(offset)
        if stream.read(6) != b"PE\0\0\x64\x86":
            raise ValueError("The Windows artifact must target x64.")


def sign(private_key: Path, platform: str, version: str, artifact: Path) -> tuple[Path, Path]:
    public_key(private_key)
    if not VERSION.fullmatch(version) or any(int(part) > 2**63 - 1 for part in version.split(".")):
        raise ValueError("The version must use the stable X.Y.Z format.")
    suffix, limit = PLATFORMS[platform]
    base = f"Backtracking-update-{platform}"
    if artifact.name != base + suffix:
        raise ValueError(f"Name the update artifact {base}{suffix}.")
    size = artifact.stat().st_size
    if not 0 < size <= limit:
        raise ValueError(f"The artifact must be between 1 and {limit} bytes.")
    check_artifact(artifact, suffix)
    with artifact.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    manifest = {
        "version": version,
        "platform": platform,
        "artifact": artifact.name,
        "size": size,
        "sha256": digest,
    }
    target_json = artifact.with_name(base + ".json")
    target_sig = artifact.with_name(base + ".json.sig")
    with tempfile.TemporaryDirectory(prefix="backtracking-sign-", dir=artifact.parent) as temp:
        temp_json = Path(temp) / "manifest.json"
        temp_sig = Path(temp) / "manifest.sig"
        temp_json.write_bytes((json.dumps(manifest, indent=2) + "\n").encode("utf-8"))
        subprocess.run(
            ["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(private_key),
             "-in", str(temp_json), "-out", str(temp_sig)],
            check=True,
        )
        if temp_sig.stat().st_size != 64:
            raise ValueError("OpenSSL produced an unexpected Ed25519 signature size.")
        temp_json.replace(target_json)
        temp_sig.replace(target_sig)
    return target_json, target_sig


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    key = commands.add_parser("public-key", help="Print the public key to embed in builds.")
    key.add_argument("private_key", type=Path)
    make = commands.add_parser("sign", help="Sign the version and hash of a release artifact.")
    make.add_argument("platform", choices=PLATFORMS)
    make.add_argument("version")
    make.add_argument("artifact", type=Path)
    make.add_argument("private_key", type=Path)
    args = parser.parse_args()
    try:
        if args.action == "public-key":
            print(public_key(args.private_key))
        else:
            for path in sign(args.private_key, args.platform, args.version, args.artifact):
                print(path)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
