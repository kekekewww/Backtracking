"""Summarizes the timing files of both passes in this folder.

Every recorded run is used: nothing is discarded, warmed up or averaged away. Each case
reports the median and the observed minimum and maximum. Writes summary.json next to
this script and prints the Markdown tables used in README.md.

    python summarize-timings.py
"""

import json
import statistics
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent

PASSES = [
    ("back_to_back", "export-timings-back-to-back.jsonl", "disk-writes-back-to-back.jsonl"),
    ("paused", "export-timings-paused.jsonl", "disk-writes-paused.jsonl"),
]

# Headings and labels as README.md shows them, which lists the second pass first.
HEADINGS = {
    "paused": "### Second pass: 3-second pause before each run",
    "back_to_back": "### First pass: runs back to back",
}
PATTERNS = {
    "4KiB_pieces_8KiB_BufWriter": "4 KiB pieces, 8 KiB `BufWriter`",
    "one_call": "one call",
}


def records(name, mode):
    # PowerShell may add a byte-order mark to each block of lines it appends.
    with open(HERE / name, encoding="utf-8-sig") as lines:
        rows = [json.loads(line.lstrip("﻿")) for line in lines if line.strip()]
    return [row for row in rows if row["mode"] == mode]


def pause_of(name, runs):
    pauses = {run.get("pause_before_ms", 0) for run in runs}
    if len(pauses) != 1:
        sys.exit(f"{name}: runs with different pauses {sorted(pauses)}")
    return pauses.pop()


def spread(values):
    return {
        "runs": len(values),
        "median_s": round(statistics.median(values), 6),
        "min_s": round(min(values), 6),
        "max_s": round(max(values), 6),
    }


def export_cases(name):
    runs = records(name, "export")
    timings = {}
    sizes = {}
    for run in runs:
        key = (run["clip_seconds"], run["source_rate"], run["export_rate"], run["lanes"])
        timings.setdefault((key, run["build"]), []).append(run["elapsed_s"])
        sizes.setdefault(key, set()).add(run["bytes"])
    cases = []
    for key in sorted(sizes, key=lambda k: (-k[3], k[0], -k[1], -k[2])):
        clip_seconds, source_rate, export_rate, lanes = key
        if len(sizes[key]) != 1:
            sys.exit(f"{name} {key}: the builds wrote different sizes {sorted(sizes[key])}")
        base = spread(timings[(key, "base")])
        change = spread(timings[(key, "change")])
        cases.append(
            {
                "clip_seconds": clip_seconds,
                "source_rate": source_rate,
                "export_rate": export_rate,
                "lanes": lanes,
                "bytes_written": next(iter(sizes[key])),
                "base": base,
                "change": change,
                "median_speedup": round(base["median_s"] / change["median_s"], 2),
            }
        )
    return cases, len(runs), pause_of(name, runs)


def write_cases(name):
    runs = records(name, "write")
    timings = {}
    for run in runs:
        timings.setdefault((run["bytes"], run["pattern"]), []).append(run["elapsed_s"])
    cases = []
    for (size, pattern), values in sorted(timings.items()):
        case = {"bytes": size, "pattern": pattern, **spread(values)}
        case["median_mb_per_s"] = round(size / 1e6 / case["median_s"], 1)
        cases.append(case)
    return cases, len(runs), pause_of(name, runs)


def rate(hz):
    return f"{hz / 1000:g} kHz"


def seconds(timing):
    return f"{timing['median_s']:.3f} ({timing['min_s']:.3f}–{timing['max_s']:.3f})"


def print_tables(summary):
    print(f"{HEADINGS[summary['name']]}\n")
    for lanes, title in [(2, "A/B pair, two stereo files:"), (1, "Lane A alone, one stereo file:")]:
        print(f"{title}\n")
        print("| Clip | Source → export | Runs | Base | Change | Speedup |")
        print("| ---: | --- | ---: | ---: | ---: | ---: |")
        for case in summary["exports"]:
            if case["lanes"] == lanes:
                print(
                    f"| {case['clip_seconds']} s | {rate(case['source_rate'])} → "
                    f"{rate(case['export_rate'])} | {case['base']['runs']} | "
                    f"{seconds(case['base'])} | {seconds(case['change'])} | "
                    f"{case['median_speedup']:.1f}× |"
                )
        print()
    print("Plain writes of new files:\n")
    print("| Bytes | Pattern | Runs | Time | MB/s at median |")
    print("| ---: | --- | ---: | ---: | ---: |")
    for case in summary["writes"]:
        print(
            f"| {case['bytes']:,} | {PATTERNS[case['pattern']]} | {case['runs']} | "
            f"{seconds(case)} | {case['median_mb_per_s']:,.0f} |"
        )
    print()


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    passes = []
    for name, exports_file, writes_file in PASSES:
        exports, export_runs, export_pause = export_cases(exports_file)
        writes, write_runs, write_pause = write_cases(writes_file)
        if export_pause != write_pause:
            sys.exit(f"{name}: exports paused {export_pause} ms, writes {write_pause} ms")
        passes.append(
            {
                "name": name,
                "export_file": exports_file,
                "write_file": writes_file,
                "pause_before_run_ms": export_pause,
                "export_runs": export_runs,
                "write_runs": write_runs,
                "exports": exports,
                "writes": writes,
            }
        )
    summary = {
        "schema_version": 1,
        "generated_by": "summarize-timings.py",
        "statistic": "median, minimum and maximum of every recorded run; nothing discarded",
        "speedup": "base median divided by change median",
        "passes": passes,
    }
    with open(HERE / "summary.json", "w", encoding="utf-8", newline="\n") as out:
        json.dump(summary, out, indent=2, ensure_ascii=False)
        out.write("\n")
    for summary in reversed(passes):
        print_tables(summary)


if __name__ == "__main__":
    main()
