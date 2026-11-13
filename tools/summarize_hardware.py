#!/usr/bin/env python3
"""Summarize monitor_backtracking.ps1 JSONL without third-party dependencies.

CPU/RAM samples are grouped by collector, session_id and the collector's
scenario_sequence. Transition periods and the first five seconds of each
stable phase are excluded. GPU data has its own observation time and phase;
cached repeats are counted once, after checking its collection start time.
"""

from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from datetime import datetime, timedelta, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import re
from typing import Any


def utc(value: Any) -> datetime | None:
    if not isinstance(value, str):
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(timezone.utc)
    except ValueError:
        return None


def iso(value: datetime | None) -> str | None:
    return value.isoformat().replace("+00:00", "Z") if value else None


def numeric(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def stats(values: list[float]) -> dict[str, Any]:
    """Linear interpolation between adjacent sorted ranks (Hyndman/Fan type 7)."""
    ordered = sorted(float(v) for v in values if numeric(v))
    if not ordered:
        return {"n": 0, "min": None, "median": None, "p95": None, "max": None, "mean": None}

    def percentile(q: float) -> float:
        rank = (len(ordered) - 1) * q
        lower = math.floor(rank)
        upper = math.ceil(rank)
        return ordered[lower] + (ordered[upper] - ordered[lower]) * (rank - lower)

    return {
        "n": len(ordered),
        "min": min(ordered),
        "median": percentile(0.5),
        "p95": percentile(0.95),
        "max": max(ordered),
        "mean": math.fsum(ordered) / len(ordered),
    }


def field_values(rows: list[dict[str, Any]], section: str, field: str) -> list[float]:
    return [row.get(section, {}).get(field) for row in rows if numeric(row.get(section, {}).get(field))]


def load_json(path: Path) -> Any:
    with path.open(encoding="utf-8-sig") as stream:
        return json.load(stream)


def public_text(value: Any) -> Any:
    """Redact local paths/user tokens; free-form notes/errors are never exported."""
    if not isinstance(value, str):
        return value
    if re.search(r"(?i)(?:[a-z]:[\\/]|\\\\|/(?:home|users)/|file://|(?:^|\s)/[^\s/]+)", value):
        return "<local-path-redacted>"
    username = os.environ.get("USERNAME") or os.environ.get("USER")
    if username:
        value = re.sub(rf"(?i)(?<![\w]){re.escape(username)}(?![\w])", "<user-redacted>", value)
    return value


def public_settings(settings: Any) -> dict[str, Any]:
    if not isinstance(settings, dict):
        return {}
    # Settings are scalar benchmark parameters, not arbitrary persisted app data.
    # Exporting no free-form notes, target names, IDs or paths avoids leaking
    # application/window titles or user-chosen folders.
    allowed = {
        "history_seconds", "waveform_only", "waveform_layout", "meter", "capture_source",
        "sources", "background_transparency_percent", "bars", "seconds_grid", "seconds_interval",
        "pinned", "options_visible", "options_page", "audio_fixture", "synthetic_audio",
        "window_visibility", "capture_state", "paused", "selection", "selection_visible", "source_a_route",
        "source_b_route", "export_sample_rate", "export_encoding", "border", "border_opacity_linked",
    }
    return {
        key: public_text(value)
        for key, value in settings.items()
        if key in allowed and (value is None or isinstance(value, (str, bool)) or numeric(value))
    }


def selected_scalars(value: Any, allowed: tuple[str, ...]) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {}
    return {key: public_text(value[key]) for key in allowed if key in value and not isinstance(value[key], (dict, list))}


def public_sample(row: dict[str, Any]) -> dict[str, Any]:
    result = selected_scalars(row, (
        "schema_version", "kind", "session_id", "timestamp_utc", "elapsed_seconds", "scenario",
        "scenario_sequence", "transition", "scenario_read_status",
    ))
    result["settings"] = public_settings(row.get("settings"))
    metadata = row.get("scenario_metadata") or {}
    result["scenario_metadata"] = selected_scalars(metadata, ("scenario", "scenario_sequence", "transition", "marked_utc"))
    result["process"] = selected_scalars(row.get("process"), (
        "status", "cpu_total_seconds", "cpu_percent_one_core", "cpu_percent_machine",
        "working_set_bytes", "working_set_mib", "private_bytes", "private_mib", "virtual_bytes", "virtual_mib",
        "thread_count", "handle_count",
    ))
    result["io"] = selected_scalars(row.get("io"), (
        "status", "source", "read_bytes", "write_bytes", "read_bytes_per_second", "write_bytes_per_second",
    ))
    result["system"] = selected_scalars(row.get("system"), (
        "source", "cpu_status", "memory_status", "cpu_percent", "available_memory_bytes", "available_memory_mib", "total_memory_bytes",
    ))
    result["gpu"] = selected_scalars(row.get("gpu"), (
        "status", "source", "engine_status", "memory_status", "observed_utc", "collection_started_utc",
        "scenario", "scenario_sequence", "transition", "age_seconds", "matches_current_scenario",
        "busiest_engine_percent", "engine_utilization_sum_percent", "dedicated_bytes", "shared_bytes", "dedicated_mib", "shared_mib",
    ))
    result["gpu"]["settings"] = public_settings((row.get("gpu") or {}).get("settings"))
    return result


def public_summary(result: dict[str, Any]) -> dict[str, Any]:
    # The summaries contain only aggregates plus these few local/context fields.
    clean = json.loads(json.dumps(result))
    clean.pop("run_root", None)
    clean["evidence_type"] = "sanitized scalar trace with reproducible aggregates"
    for collector in clean["collectors"]:
        collector.pop("file", None)
        for phase in collector["phases"]:
            phase.pop("notes", None)
            phase.pop("gpu_engine_instances", None)
            phase["scenario"] = public_text(phase["scenario"])
            phase["settings"] = public_settings(phase["settings"])
    return clean


def publish_evidence(run_root: Path, destination: Path, result: dict[str, Any]) -> None:
    """Explicit opt-in export after collectors stop; leaves raw local logs intact."""
    collector_dirs = sorted({path.parent for path in run_root.glob("*/telemetry.jsonl")})
    if not collector_dirs or any(
        not (directory / "monitor_status.json").exists()
        or load_json(directory / "monitor_status.json").get("state") != "stopped"
        for directory in collector_dirs
    ):
        raise ValueError("Public evidence requires all collectors to have stopped; monitoring is still running or status is missing")
    destination.mkdir(parents=True, exist_ok=True)

    def write_json(name: str, data: Any) -> None:
        (destination / name).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    write_json("summary.json", public_summary(result))
    for path in sorted(run_root.glob("*/telemetry.jsonl")):
        with path.open(encoding="utf-8-sig") as source, (destination / f"{path.parent.name}.scalar-trace.jsonl").open("w", encoding="utf-8") as output:
            for line in source:
                row = json.loads(line)
                if isinstance(row, dict) and row.get("kind") == "sample":
                    output.write(json.dumps(public_sample(row), ensure_ascii=False, separators=(",", ":")) + "\n")

    metadata_fields = (
        "schema_version", "session_id", "started_utc", "logical_processor_count", "sample_interval_seconds",
        "gpu_interval_seconds", "duration_limit_seconds", "powershell_version", "os_version",
        "process_architecture_bits", "native_counter_status",
    )
    for collector_dir in collector_dirs:
        write_json(f"{collector_dir.name}.collector-metadata.json", [
            selected_scalars(load_json(path), metadata_fields)
            for path in sorted(collector_dir.glob("monitor_metadata.*.json"))
        ])
        write_json(f"{collector_dir.name}.collector-status.json", selected_scalars(
            load_json(collector_dir / "monitor_status.json"),
            ("state", "stop_reason", "session_id", "stopped_utc", "samples", "elapsed_seconds"),
        ))

    hardware_path = run_root / "hardware.json"
    if hardware_path.exists():
        hardware = load_json(hardware_path)
        clean_hardware = selected_scalars(hardware, (
            "captured_utc", "source_commit", "source_has_uncommitted_changes", "desktop_exe_sha256", "plugin_dll_sha256",
        ))
        clean_hardware["cpu"] = [selected_scalars(item, ("Name", "NumberOfCores", "NumberOfLogicalProcessors", "MaxClockSpeed")) for item in hardware.get("cpu", [])]
        clean_hardware["operating_system"] = selected_scalars(hardware.get("operating_system"), ("Caption", "Version", "BuildNumber", "OSArchitecture", "TotalVisibleMemorySize"))
        clean_hardware["physical_memory"] = selected_scalars(hardware.get("physical_memory"), ("Manufacturer", "Model", "TotalPhysicalMemory"))
        clean_hardware["graphics"] = [selected_scalars(item, ("Name", "DriverVersion")) for item in hardware.get("graphics", [])]
        clean_hardware["graphics_note"] = "Adapter models only; engine-to-adapter mapping was not established. Win32_VideoController.AdapterRAM is omitted because its 32-bit field can truncate VRAM capacity."
        clean_hardware["app_launch_arguments"] = [arg for arg in hardware.get("app_launch_arguments", []) if arg in ("--new-capture-window", "--show-options")]
        write_json("hardware.json", clean_hardware)

    event_path = run_root / "events.jsonl"
    if event_path.exists():
        with event_path.open(encoding="utf-8-sig") as source, (destination / "settings-events.jsonl").open("w", encoding="utf-8") as output:
            for line in source:
                event = json.loads(line)
                clean_event = selected_scalars(event, ("scenario", "scenario_sequence", "transition", "marked_utc"))
                clean_event["settings"] = public_settings(event.get("settings"))
                output.write(json.dumps(clean_event, ensure_ascii=False, separators=(",", ":")) + "\n")
    write_json("evidence-manifest.json", {
        "schema_version": 1,
        "evidence_type": "sanitized scalar trace",
        "generated_utc": result["generated_utc"],
        "removed_fields": ["local absolute paths", "user name tokens", "process IDs and names", "free-form notes and error messages", "GPU instance identifiers and repeated engine arrays", "Win32 AdapterRAM"],
        "limitations": ["Scalar traces preserve per-second GPU caches; summary deduplicates observed_utc", "Source audio and desktop screenshots are not part of this evidence export", "No GPU adapter identity is inferred from engine utilization"],
        "files": [
            {"name": path.name, "size_bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for path in sorted(destination.iterdir()) if path.is_file() and path.name != "evidence-manifest.json"
        ],
        "tool_sources": [
            {"path": f"tools/{name}", "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for name in ("monitor_backtracking.ps1", "summarize_hardware.py", "generate_test_audio.py")
            if (path := Path(__file__).resolve().parent / name).exists()
        ],
    })


def summarize_file(path: Path, warmup_seconds: float, baseline_audio_start: datetime | None) -> dict[str, Any]:
    groups: dict[tuple[Any, Any], list[dict[str, Any]]] = defaultdict(list)
    skipped: Counter[str] = Counter()
    total_lines = 0
    with path.open(encoding="utf-8-sig") as stream:
        lines = stream.readlines()
        for line_index, line in enumerate(lines):
            total_lines += 1
            try:
                row = json.loads(line)
            except json.JSONDecodeError as error:
                # A live collector may be midway through its final write.
                # Malformed completed records must not disappear silently.
                if line_index == len(lines) - 1 and not line.endswith("\n"):
                    skipped["incomplete_tail_line"] += 1
                    continue
                raise ValueError(f"{path}:{line_index + 1}: malformed JSON record") from error
            if not isinstance(row, dict) or row.get("kind") != "sample":
                skipped["non_sample_records"] += 1
                continue
            if utc(row.get("timestamp_utc")) is None:
                skipped["invalid_timestamp"] += 1
                continue
            groups[(row.get("session_id"), row.get("scenario_sequence"))].append(row)

    phases = []
    for (session_id, sequence), rows in groups.items():
        rows.sort(key=lambda row: utc(row["timestamp_utc"]))
        first = rows[0]
        metadata = first.get("scenario_metadata") or {}
        first_at = utc(first["timestamp_utc"])
        marked_at = utc(metadata.get("marked_utc"))
        # A future or malformed marker cannot make earlier samples eligible.
        phase_start = marked_at if marked_at is not None and marked_at <= first_at else first_at
        steady_start = phase_start + timedelta(seconds=warmup_seconds)
        phase_end = utc(rows[-1]["timestamp_utc"])
        excluded: Counter[str] = Counter()
        stable = []
        for row in rows:
            at = utc(row["timestamp_utc"])
            if row.get("transition") is not False:
                excluded["transition"] += 1
            elif row.get("scenario_read_status") != "available":
                excluded["scenario_metadata_unavailable"] += 1
            elif (
                baseline_audio_start is not None
                and row.get("scenario") == "ambient_main_before_test_audio"
                and at >= baseline_audio_start
            ):
                excluded["stale_no_audio_label_after_playback_start"] += 1
            elif at < steady_start:
                excluded["warmup"] += 1
            elif row.get("process", {}).get("status") != "available":
                excluded["process_unavailable"] += 1
            else:
                stable.append(row)

        gpu_rows = []
        gpu_seen = set()
        gpu_excluded: Counter[str] = Counter()
        for row in stable:
            gpu = row.get("gpu") or {}
            observed = utc(gpu.get("observed_utc"))
            collected = utc(gpu.get("collection_started_utc"))
            if gpu.get("status") != "available":
                gpu_excluded["unavailable"] += 1
                continue
            if (
                gpu.get("matches_current_scenario") is not True
                or gpu.get("transition") is not False
                or gpu.get("scenario_sequence") != sequence
                or gpu.get("scenario") != row.get("scenario")
            ):
                gpu_excluded["phase_mismatch_or_transition"] += 1
                continue
            if observed is None or collected is None or observed < collected:
                gpu_excluded["invalid_gpu_timestamps"] += 1
                continue
            if collected < steady_start or observed > phase_end:
                gpu_excluded["collection_outside_steady_phase"] += 1
                continue
            key = (session_id, gpu["observed_utc"])
            if key in gpu_seen:
                gpu_excluded["cached_repeat"] += 1
                continue
            gpu_seen.add(key)
            gpu_rows.append(gpu)

        cpu_and_memory = {
            field: stats(field_values(stable, "process", field))
            for field in (
                "cpu_percent_one_core", "cpu_percent_machine", "working_set_mib", "private_mib",
                "virtual_mib", "thread_count", "handle_count",
            )
        }
        gpu_summary = {
            field: stats([
                gpu.get(field) for gpu in gpu_rows
                if numeric(gpu.get(field))
                and gpu.get("engine_status" if "percent" in field else "memory_status") == "available"
            ])
            for field in (
                "busiest_engine_percent", "engine_utilization_sum_percent", "dedicated_mib", "shared_mib",
            )
        }
        phases.append({
            "session_id": session_id,
            "scenario_sequence": sequence,
            "event_scenario_sequence": metadata.get("scenario_sequence"),
            "scenario": first.get("scenario"),
            "settings": first.get("settings"),
            "notes": metadata.get("notes"),
            "marked_utc": metadata.get("marked_utc"),
            "phase_start_utc": iso(phase_start),
            "steady_start_utc": iso(steady_start),
            "first_sample_utc": first["timestamp_utc"],
            "last_sample_utc": rows[-1]["timestamp_utc"],
            "observed_seconds": (phase_end - first_at).total_seconds(),
            "stable_observed_seconds": (utc(stable[-1]["timestamp_utc"]) - utc(stable[0]["timestamp_utc"])).total_seconds() if len(stable) > 1 else 0,
            "samples_total": len(rows),
            "samples_used": len(stable),
            "excluded_samples": dict(excluded),
            "process": cpu_and_memory,
            "io": {field: stats(field_values(stable, "io", field)) for field in ("read_bytes_per_second", "write_bytes_per_second")},
            "system_context": {field: stats(field_values(stable, "system", field)) for field in ("cpu_percent", "available_memory_mib")},
            "gpu_observations_used": len(gpu_rows),
            "gpu_exclusions": dict(gpu_excluded),
            "gpu_sources": sorted({gpu.get("source") for gpu in gpu_rows if gpu.get("source")}),
            "gpu_engine_instances": sorted({engine["instance"] for gpu in gpu_rows for engine in gpu.get("engines", []) if engine.get("instance")}),
            "gpu": gpu_summary,
        })
    return {"collector": path.parent.name, "file": str(path), "lines_total": total_lines, "skipped_records": dict(skipped), "phases": phases}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_root", nargs="?", type=Path, help="Defaults to target/active-hardware-run.txt")
    parser.add_argument("--warmup-seconds", type=float, default=5.0)
    parser.add_argument("--output", type=Path, help="Defaults to <run_root>/summary.json")
    parser.add_argument("--publish-directory", type=Path, help="After all collectors stop, also export sanitized scalar evidence to this directory")
    parser.add_argument("--baseline-audio-start", help="Exclude stale ambient_main_before_test_audio labels from this UTC instant; defaults to playback.json started_utc")
    args = parser.parse_args()
    if args.warmup_seconds < 0:
        parser.error("--warmup-seconds must be nonnegative")
    run_root = args.run_root or Path(Path("target/active-hardware-run.txt").read_text(encoding="utf-8-sig").strip())
    audio_start_text = args.baseline_audio_start
    if audio_start_text is None and (run_root / "playback.json").exists():
        audio_start_text = load_json(run_root / "playback.json").get("started_utc")
    audio_start = utc(audio_start_text)
    if audio_start_text is not None and audio_start is None:
        parser.error("invalid baseline audio start UTC timestamp")
    files = sorted(run_root.glob("*/telemetry.jsonl"))
    if not files:
        parser.error(f"no collector telemetry found in {run_root}")
    result = {
        "schema_version": 1,
        "generated_utc": iso(datetime.now(timezone.utc)),
        "run_root": str(run_root),
        "method": {
            "grouping": ["collector", "session_id", "scenario_sequence"],
            "warmup_seconds_excluded": args.warmup_seconds,
            "quantiles": "Hyndman/Fan type 7 (linear interpolation between sorted sample ranks)",
            "cpu_percent_one_core": "100% equals one logical CPU; may exceed 100%",
            "cpu_percent_machine": "Normalized by collector Environment.ProcessorCount; see monitor metadata",
            "memory_unit": "MiB = 1048576 bytes; RSS is working set, private is committed private bytes",
            "gpu": "Own phase must match, transition false, collection start after warmup; unique (session_id, observed_utc)",
            "gpu_engine_percent": "Busiest engine preferred; engine sums may exceed 100% and are not total GPU utilization",
            "io": "Process I/O includes cached/non-disk I/O; not physical disk throughput",
            "baseline_audio_started_utc": iso(audio_start),
            "stale_baseline_rule": "Exclude ambient_main_before_test_audio samples at or after playback start",
        },
        "collectors": [summarize_file(path, args.warmup_seconds, audio_start) for path in files],
    }
    output = args.output or run_root / "summary.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {output}: {sum(len(item['phases']) for item in result['collectors'])} phases")
    if args.publish_directory:
        publish_evidence(run_root, args.publish_directory, result)
        print(f"Exported sanitized scalar evidence to {args.publish_directory}")


if __name__ == "__main__":
    main()
