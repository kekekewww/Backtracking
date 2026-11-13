#!/usr/bin/env python3
"""Read-only, streaming RIFF/WAVE structure and sample-value inspection.

Supports integer PCM (8/16/24/32 bits) and IEEE float (32/64 bits), including
standard WAVE_FORMAT_EXTENSIBLE subformats. Reports never include source paths.
This validates the file as stored, not its capture timing or audible quality.
"""

from __future__ import annotations

import argparse
from array import array
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import struct
import sys


BLOCK_BYTES = 1024 * 1024


def amplitude_dbfs(value: float) -> float | None:
    return 20 * math.log10(value) if value > 0 else None


def decode_samples(raw: bytes, format_tag: int, bits: int):
    if format_tag == 1 and bits == 8:
        return ((sample - 128) / 128 for sample in raw)
    if format_tag == 1 and bits == 24:
        return (
            int.from_bytes(raw[offset:offset + 3], "little", signed=True) / 8388608
            for offset in range(0, len(raw), 3)
        )
    code = {(1, 16): "h", (1, 32): "i", (3, 32): "f", (3, 64): "d"}.get((format_tag, bits))
    if code is None:
        raise ValueError(f"Unsupported WAV encoding tag={format_tag}, bits={bits}")
    samples = array(code)
    if samples.itemsize != bits // 8:
        raise ValueError("Platform array size does not match WAV sample width")
    samples.frombytes(raw)
    if sys.byteorder != "little":
        samples.byteswap()
    if format_tag == 1:
        divisor = 2 ** (bits - 1)
        return (sample / divisor for sample in samples)
    return iter(samples)


def inspect(path: Path, label: str, context: str | None) -> dict:
    before = path.stat()
    chunks = []
    data_chunks = []
    fmt = None
    fact_frames = None
    with path.open("rb") as source:
        header = source.read(12)
        if len(header) != 12:
            raise ValueError("File is shorter than a RIFF/WAVE header")
        container, riff_size, kind = struct.unpack("<4sI4s", header)
        if container != b"RIFF" or kind != b"WAVE":
            raise ValueError("Only little-endian RIFF/WAVE is supported")
        position = 12
        while position + 8 <= before.st_size:
            source.seek(position)
            chunk_id, size = struct.unpack("<4sI", source.read(8))
            offset = position + 8
            end = offset + size + (size & 1)
            if end > before.st_size:
                raise ValueError("A chunk extends beyond the physical file")
            chunks.append({"id": chunk_id.decode("ascii", errors="replace"),
                           "header_offset": position, "data_offset": offset,
                           "size_bytes": size, "padded_end_offset": end})
            if chunk_id == b"fmt ":
                if fmt is not None:
                    raise ValueError("Multiple fmt chunks are ambiguous")
                fmt = source.read(min(size, 64))
            elif chunk_id == b"fact" and size >= 4:
                fact_frames = struct.unpack("<I", source.read(4))[0]
            elif chunk_id == b"data":
                data_chunks.append((offset, size))
            position = end
        if fmt is None or len(fmt) < 16 or not data_chunks:
            raise ValueError("Required fmt/data chunk is missing or incomplete")
        tag, channels, rate, byte_rate, alignment, bits = struct.unpack("<HHIIHH", fmt[:16])
        encoding_tag = tag
        valid_bits = bits
        channel_mask = None
        if tag == 65534:
            if len(fmt) < 40 or fmt[28:40] != bytes.fromhex("00001000800000aa00389b71"):
                raise ValueError("Unsupported extensible WAV subformat")
            valid_bits, channel_mask = struct.unpack("<HI", fmt[18:24])
            encoding_tag = struct.unpack("<I", fmt[24:28])[0]
        if channels <= 0 or rate <= 0 or alignment <= 0:
            raise ValueError("Invalid channel count, sample rate, or block alignment")
        if (encoding_tag, bits) not in {(1, 8), (1, 16), (1, 24), (1, 32), (3, 32), (3, 64)}:
            raise ValueError(f"Unsupported WAV encoding tag={encoding_tag}, bits={bits}")
        data_bytes = sum(size for _, size in data_chunks)
        if alignment != channels * (bits // 8) or any(size % alignment for _, size in data_chunks):
            raise ValueError("Data or format is not aligned to complete sample frames")
        frames = data_bytes // alignment
        if frames == 0:
            raise ValueError("WAV contains no sample frames")

        stats = [{"finite_samples": 0, "non_finite_samples": 0,
                  "zero_samples": 0, "full_scale_or_over_samples": 0,
                  "peak_linear": 0.0, "sum_squares": 0.0,
                  "minimum": None, "maximum": None} for _ in range(channels)]
        block_size = max(alignment, BLOCK_BYTES // alignment * alignment)
        for offset, size in data_chunks:
            source.seek(offset)
            remaining = size
            while remaining:
                raw = source.read(min(block_size, remaining))
                if not raw:
                    raise ValueError("Data was truncated while reading")
                remaining -= len(raw)
                square_sums = [[] for _ in range(channels)]
                for index, sample in enumerate(decode_samples(raw, encoding_tag, bits)):
                    channel = index % channels
                    stat = stats[channel]
                    if not math.isfinite(sample):
                        stat["non_finite_samples"] += 1
                        continue
                    stat["finite_samples"] += 1
                    stat["zero_samples"] += sample == 0
                    stat["full_scale_or_over_samples"] += abs(sample) >= 1.0
                    stat["peak_linear"] = max(stat["peak_linear"], abs(sample))
                    stat["minimum"] = sample if stat["minimum"] is None else min(stat["minimum"], sample)
                    stat["maximum"] = sample if stat["maximum"] is None else max(stat["maximum"], sample)
                    square_sums[channel].append(sample * sample)
                for channel, values in enumerate(square_sums):
                    stats[channel]["sum_squares"] += math.fsum(values)

        source.seek(0)
        digest = hashlib.sha256()
        while block := source.read(BLOCK_BYTES):
            digest.update(block)

    after = path.stat()
    total_finite = sum(stat["finite_samples"] for stat in stats)
    total_non_finite = sum(stat["non_finite_samples"] for stat in stats)
    total_full_scale = sum(stat["full_scale_or_over_samples"] for stat in stats)
    total_squares = sum(stat["sum_squares"] for stat in stats)
    peak = max(stat["peak_linear"] for stat in stats)
    rms = math.sqrt(total_squares / total_finite) if total_finite else None
    for stat in stats:
        squares = stat.pop("sum_squares")
        stat["rms_linear"] = math.sqrt(squares / stat["finite_samples"]) if stat["finite_samples"] else None
        stat["peak_dbfs"] = amplitude_dbfs(stat["peak_linear"])
        stat["rms_dbfs"] = amplitude_dbfs(stat["rms_linear"]) if stat["rms_linear"] is not None else None
    checks = {
        "riff_size_matches_file_size": riff_size + 8 == before.st_size,
        "chunks_end_at_file_end": position == before.st_size,
        "byte_rate_matches_format": byte_rate == rate * alignment,
        "data_contains_complete_frames": all(size % alignment == 0 for _, size in data_chunks),
        "fact_matches_data_frame_count": fact_frames == frames if fact_frames is not None else None,
        "all_samples_finite": total_non_finite == 0,
        "no_full_scale_or_over_samples": total_full_scale == 0,
        "file_unchanged_during_inspection": before.st_size == after.st_size and before.st_mtime_ns == after.st_mtime_ns,
    }
    return {
        "schema_version": 1,
        "inspector": "tools/inspect_wav.py",
        "inspected_utc": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "context": context,
        "source": {"label": label, "file_size_bytes": before.st_size, "sha256": digest.hexdigest()},
        "format": {"container": "RIFF/WAVE", "format_tag": tag,
                   "encoding": "IEEE float" if encoding_tag == 3 else "integer PCM",
                   "sample_rate_hz": rate, "channels": channels, "bits_per_sample": bits,
                   "valid_bits_per_sample": valid_bits, "channel_mask": channel_mask,
                   "byte_rate": byte_rate, "block_align_bytes": alignment},
        "layout": {"riff_size_field_bytes": riff_size, "riff_extent_bytes": riff_size + 8,
                   "data_size_bytes": data_bytes, "non_data_size_bytes": before.st_size - data_bytes,
                   "frame_count": frames, "fact_frame_count": fact_frames,
                   "duration_seconds": frames / rate, "chunks": chunks},
        "samples": {"count": frames * channels, "finite_samples": total_finite,
                    "non_finite_samples": total_non_finite,
                    "full_scale_or_over_samples": total_full_scale,
                    "full_scale_threshold_linear": 1.0,
                    "peak_linear": peak, "peak_dbfs": amplitude_dbfs(peak),
                    "rms_linear": rms, "rms_dbfs": amplitude_dbfs(rms) if rms is not None else None,
                    "per_channel": stats},
        "checks": checks,
        "all_applicable_checks_passed": all(value is not False for value in checks.values()),
        "limitations": [
            "Duration is derived from this file; no expected capture interval was compared.",
            "This inspection does not validate the earlier 600-second retention setting or a 600-second WAV.",
            "Sample statistics do not establish absence of missing or duplicated frames, clicks, audible artifacts, or prior clipping.",
            "Full-scale counts describe decoded stored samples, using absolute value >= 1.0; they are not a complete clipping detector.",
            "This inspection does not exercise or validate selected-range export.",
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wav", type=Path)
    parser.add_argument("--output", type=Path, action="append", default=[])
    parser.add_argument("--label", default="snapshot-under-inspection.wav", help="public-safe source label")
    parser.add_argument("--context", help="public-safe test scope and provenance")
    args = parser.parse_args()
    report = inspect(args.wav, args.label, args.context)
    encoded = json.dumps(report, indent=2, allow_nan=False) + "\n"
    for output in args.output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    if not report["all_applicable_checks_passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
