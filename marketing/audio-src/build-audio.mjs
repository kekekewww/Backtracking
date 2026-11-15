// Cuts the campaign music from the source track. Usage: node marketing/audio-src/build-audio.mjs
// Needs ffmpeg (set FFMPEG to its full path if it is not on PATH). See README.md in this folder.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = join(here, 'Dyalla - Fun Vibe.mp3');
const OUT = join(here, 'out');
const FFMPEG = process.env.FFMPEG || 'ffmpeg';

// Measured on the source file: 122.00 BPM, main drop (first kick after the breakdown) at 79.55 s.
const BEAT = 60 / 122;
const DROP = 79.545; // about 5 ms before the kick so its attack is not clipped

const jobs = [
  {
    // Overview video, 57.5 s. The hard cut to "IT WAS ALREADY LISTENING." is at 5.6 s: the drop lands there.
    file: 'overview-music.m4a', start: DROP - 5.6, length: 57.5, gainDb: -1.7, // -12.3 -> -14 LUFS
    fadeIn: 0.4, fadeOut: 2.2, codec: ['-c:a', 'aac', '-b:a', '192k', '-ar', '48000'],
  },
  {
    // Vertical short, 21 s. The cut to the one-liner is at 3.6 s: the drop lands there.
    file: 'short-music.m4a', start: DROP - 3.6, length: 21, gainDb: -1.0, // -13.0 -> -14 LUFS
    fadeIn: 0.3, fadeOut: 1.4, codec: ['-c:a', 'aac', '-b:a', '192k', '-ar', '48000'],
  },
  {
    // Landing-page demo: 16 bars (64 beats) from the drop, with 0.5 s of real audio either side.
    // The page loops buffer time 0.5 .. 0.5 + 64 beats (31.97541 s); the margins absorb decoder delay.
    file: 'demo-loop.mp3', start: DROP - 0.5, length: 0.5 + 64 * BEAT + 0.5, gainDb: -3.5, // -12.5 -> -16 LUFS
    fadeIn: 0, fadeOut: 0, codec: ['-c:a', 'libmp3lame', '-b:a', '160k', '-ar', '44100'],
  },
];

if (!existsSync(SOURCE)) {
  console.error('Missing source track: ' + SOURCE + '\nSee README.md in this folder for where it comes from.');
  process.exit(1);
}
mkdirSync(OUT, { recursive: true });

for (const j of jobs) {
  const filters = ['volume=' + j.gainDb + 'dB'];
  if (j.fadeIn) filters.push('afade=t=in:st=0:d=' + j.fadeIn);
  if (j.fadeOut) filters.push('afade=t=out:st=' + (j.length - j.fadeOut).toFixed(4) + ':d=' + j.fadeOut);
  const out = join(OUT, j.file);
  const args = ['-hide_banner', '-loglevel', 'error', '-y', '-ss', j.start.toFixed(4), '-t', j.length.toFixed(4),
    '-i', SOURCE, '-vn', '-map_metadata', '-1', '-af', filters.join(','), ...j.codec, out];
  const r = spawnSync(FFMPEG, args, { stdio: 'inherit' });
  if (r.status !== 0) { console.error('ffmpeg failed for ' + j.file); process.exit(1); }
  console.log(j.file.padEnd(20) + (statSync(out).size / 1024).toFixed(0).padStart(6) + ' KB  from ' +
    j.start.toFixed(3) + ' s, ' + j.length.toFixed(3) + ' s');
}
console.log('Demo loop points (seconds in demo-loop.mp3): loopStart 0.5, loopEnd ' + (0.5 + 64 * BEAT).toFixed(5));
