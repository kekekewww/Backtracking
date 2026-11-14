#!/usr/bin/env python3
"""Generate quiet, deterministic WAV fixtures for Backtracking capture checks.

Run from any directory:
    python tools/generate_test_audio.py
    python tools/generate_test_audio.py --output-dir target/perf-audio

The generator only writes audio and a manifest; it never starts playback or
changes device volume. Samples peak at approximately -21.94 dBFS (0.08).
"""

from __future__ import annotations

import argparse
from array import array
import hashlib
import json
import math
from pathlib import Path
import sys
import wave


SAMPLE_RATE = 48_000
CHANNELS = 2
SAMPLE_WIDTH = 2
TARGET_PEAK = 0.08
BLOCK_FRAMES = 4096
FADE_FRAMES = SAMPLE_RATE // 100  # 10 ms, contained inside each audio segment.
TAU = math.tau


def sample_pair(frame: int, gated: bool) -> tuple[float, float]:
    """Return a bounded stereo mixture, before per-file peak normalization."""
    gain = 1.0
    if gated:
        position = frame % (10 * SAMPLE_RATE)
        if position >= 5 * SAMPLE_RATE:
            return 0.0, 0.0
        edge = min(position, 5 * SAMPLE_RATE - 1 - position)
        if edge < FADE_FRAMES:
            gain = 0.5 - 0.5 * math.cos(math.pi * edge / FADE_FRAMES)

    time = frame / SAMPLE_RATE
    envelope = (
        0.70
        + 0.15 * math.sin(TAU * time / 4)
        + 0.10 * math.sin(TAU * time / 10)
    )
    pulse_position = (frame % (5 * SAMPLE_RATE // 2)) / (0.15 * SAMPLE_RATE)
    pulse = math.sin(math.pi * pulse_position) ** 2 if pulse_position < 1 else 0.0
    left_phase = TAU * 440 * time + 0.8 * math.sin(TAU * time / 5)
    right_phase = TAU * 660 * time + 0.55 * math.sin(TAU * time / 4)
    left = envelope * (
        0.83 * math.sin(left_phase) + 0.12 * math.sin(TAU * 880 * time)
    ) + 0.05 * pulse * math.sin(TAU * 1760 * time)
    right = envelope * (
        0.76 * math.sin(right_phase) + 0.14 * math.sin(TAU * 1320 * time)
    ) + 0.10 * pulse * math.sin(TAU * 1980 * time)
    return left * gain, right * gain


def decibels(amplitude: float) -> float | None:
    """Use JSON null for digital silence instead of nonstandard -Infinity."""
    return 20 * math.log10(amplitude) if amplitude > 0 else None


def segments(duration: int, gated: bool) -> list[dict]:
    ranges = []
    step = 5 if gated else duration
    for start in range(0, duration, step):
        audio = not gated or (start // 5) % 2 == 0
        ranges.append(
            {
                "kind": "audio" if audio else "digital_silence",
                "start_frame": start * SAMPLE_RATE,
                "end_frame_exclusive": (start + step) * SAMPLE_RATE,
                "start_seconds": start,
                "end_seconds_exclusive": start + step,
                "fade_in_frames": FADE_FRAMES if gated and audio else 0,
                "fade_out_frames": FADE_FRAMES if gated and audio else 0,
            }
        )
    return ranges


def write_fixture(output_dir: Path, name: str, duration: int, gated: bool) -> dict:
    frame_count = duration * SAMPLE_RATE
    raw_peak = 0.0
    for frame in range(frame_count):
        left, right = sample_pair(frame, gated)
        raw_peak = max(raw_peak, abs(left), abs(right))
    scale = TARGET_PEAK * 32768 / raw_peak
    peaks = [0, 0]
    sum_squares = [0, 0]
    audio_hash = hashlib.sha256()
    path = output_dir / name

    with wave.open(str(path), "wb") as wav_file:
        wav_file.setparams((CHANNELS, SAMPLE_WIDTH, SAMPLE_RATE, frame_count, "NONE", "PCM"))
        for start in range(0, frame_count, BLOCK_FRAMES):
            samples = array("h")
            for frame in range(start, min(start + BLOCK_FRAMES, frame_count)):
                for channel, value in enumerate(sample_pair(frame, gated)):
                    sample = round(value * scale)
                    samples.append(sample)
                    peaks[channel] = max(peaks[channel], abs(sample))
                    sum_squares[channel] += sample * sample
            if sys.byteorder != "little":
                samples.byteswap()
            block = samples.tobytes()
            wav_file.writeframesraw(block)
            audio_hash.update(block)

    channel_peak = [peak / 32768 for peak in peaks]
    channel_rms = [math.sqrt(total / frame_count) / 32768 for total in sum_squares]
    peak = max(channel_peak)
    rms = math.sqrt(sum(sum_squares) / (frame_count * CHANNELS)) / 32768
    return {
        "path": name,
        "description": (
            "Three 5-second stereo mixtures alternating with exact 5-second digital silence."
            if gated
            else "Uninterrupted stereo mixture; all modulations repeat exactly every 20 seconds."
        ),
        "sample_rate_hz": SAMPLE_RATE,
        "channels": CHANNELS,
        "sample_width_bytes": SAMPLE_WIDTH,
        "encoding": "PCM signed 16-bit little-endian",
        "duration_seconds": duration,
        "frame_count": frame_count,
        "file_size_bytes": path.stat().st_size,
        "peak_linear": peak,
        "peak_dbfs": decibels(peak),
        "rms_linear": rms,
        "rms_dbfs": decibels(rms),
        "channel_peak_linear": channel_peak,
        "channel_rms_linear": channel_rms,
        "pcm_sha256": audio_hash.hexdigest(),
        "segments": segments(duration, gated),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "target" / "perf-audio",
        help="destination for the two WAV files and manifest.json",
    )
    arguments = parser.parse_args()
    output_dir = arguments.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    fixtures = [
        write_fixture(output_dir, "continuous-stereo.wav", 20, False),
        write_fixture(output_dir, "gated-stereo.wav", 30, True),
    ]
    manifest = {
        "schema_version": 1,
        "generator": "tools/generate_test_audio.py",
        "target_peak_linear": TARGET_PEAK,
        "target_peak_dbfs": decibels(TARGET_PEAK),
        "boundary_convention": "Frame ranges are zero-based and end-exclusive.",
        "signal": {
            "left_fundamental_hz": 440,
            "right_fundamental_hz": 660,
            "character": "Slow amplitude/frequency variation, harmonics, and faded 150 ms pulses every 2.5 s.",
            "continuous_loop_seconds": 20,
            "gated_loop_seconds": 30,
            "gated_audio_fade_frames": FADE_FRAMES,
        },
        "fixtures": fixtures,
    }
    manifest_path = output_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    for fixture in fixtures:
        print(
            f"{output_dir / fixture['path']}: {fixture['duration_seconds']} s, "
            f"peak {fixture['peak_dbfs']:.2f} dBFS, RMS {fixture['rms_dbfs']:.2f} dBFS"
        )
    print(manifest_path)


if __name__ == "__main__":
    main()
