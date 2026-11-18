# Third-party data notices

## SADIE II D1 / Neumann KU100 head responses

`crates/backtracking-core/src/spatial/sadie-d1.bin` is a reduced, modified
representation of the SADIE II Database D1 head-related impulse responses
(48 kHz, 256 taps). Measurements are Copyright University of York.

- Publisher: The Audio Lab, Department of Electronic Engineering, University
  of York, United Kingdom.
- Dataset: SADIE II, D1 / Neumann KU100.
- Dataset page: <https://www.york.ac.uk/sadie-project/database.html>.
- Current D1 distribution: <https://zenodo.org/records/12092466>.
- Source used by the OpenJOC preparation pipeline:
  <https://sofacoustics.org/data/database/sadie/D1_48K_24bit_256tap_FIR_SOFA.sofa>.
- License: Apache License, Version 2.0. The full license text is in
  [LICENSE](LICENSE); the University of York/SADIE II attribution must be retained.
- Academic citation: Cal Armstrong, Lewis Thresh, and Gavin Kearney,
  "A Perceptual Evaluation of Individual and Non-Individual HRTFs: A Case
  Study of the SADIE II Database," <https://doi.org/10.3390/app8112029>.

This remains third-party measurement data. Its inclusion does not imply
endorsement by the University of York or the SADIE II authors.

### Source and hashes

The offline generator reads OpenJOC's version-2 direct-record asset from the
fork <https://github.com/kekekewww/OpenJOC>, revision
`3b7198cf3d15e0d8d4b0396c071ff77a11d24c49`:
`crates/openjoc-sofa/assets/sadie-ii-d1-ku100.ojhrtf`.
The source hash is checked before parsing; the payload's own SHA-256 is
also checked. No OpenJOC code or full asset is required to build or test Backtracking.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Original SOFA, as recorded by OpenJOC | — | `e6c72a84dd947b5ef75438ab96a9c2a32ed10f033472b9c4c11a49aff00a8a31` |
| OpenJOC CDF-1 intermediate, as recorded by OpenJOC | — | `b9bcecd8a07e7eed4474a9b063c47672384339e83605bd245ff0adc098869fab` |
| Input `.ojhrtf` | 18,374,724 | `78d048a68f84d34051578c262e401e35baa0e718901f85349afe0232f985d4df` |
| Backtracking `sadie-d1.bin` | 505,856 | `25aa55d53c9f2c00933cf195874dcf5c15b3083cb922d46845967a43366dbb3e` |

### Changes made for Backtracking

The generator is the separate, dependency-free Cargo project at
`crates/backtracking-core/tools/sadie-table`. Its own `[workspace]` prevents
membership in the application workspace. It parses the asset format directly,
without fetching or linking OpenJOC. The input has 8,802 measurements followed
by 15 OpenJOC virtual-speaker aliases; only the 8,802 measured rows are searched.
The checked source contains zero separate sample delays. Its acoustic delays
remain in the 256 taps; an input with nonzero separate delays is rejected.

The retained regular grid has azimuths 0 through 180 degrees in 10-degree
steps and elevations -90 through 90 degrees in 15-degree steps: 247 stored
rows, including repeated poles. Azimuth 0 is front, positive is right;
elevation is positive up. Each grid point selects the nearest measured unit
direction by maximum dot product. At azimuth 0 and 180, and at both poles,
the two ear responses are averaged to make them equal. All copies of each
pole use the same response. Negative azimuths reflect the stored response by
swapping ears. These symmetry changes deliberately discard the measured
left/right asymmetry to provide exact mirror behavior.

The file is headerless little-endian f32, ordered by ascending elevation,
then ascending azimuth, then left ear's 256 taps and right ear's 256 taps.
Its source-rate tap storage is 505,856 bytes in memory as well as on disk.
There is no compression, tap truncation, EQ, or amplitude normalization in
the table. Runtime uses bilinear interpolation of these taps and resamples
the bank once when a different sample rate is requested.

### Measured reduction error and format choice

The generator evaluates a common set of 864 directions between both grids:
azimuths `±(2.5 + 5*i)` degrees for `i = 0..35`, and elevations
`-82.5 + 15*j` degrees for `j = 0..11`. The reference for each direction
is the full set's nearest measured response, without symmetrization. Thus
the following errors include spatial reduction, interpolation, and the
intentional removal of measured asymmetry. All errors are before the
renderer’s factor-of-0.5 output gain.

| Measure | 15° azimuth / 15° elevation | Selected 10° / 15° |
| --- | ---: | ---: |
| Stored rows | 169 | 247 |
| f32 bytes | 346,112 | 505,856 |
| Maximum grid-to-nearest-measurement angle | 0.000000854° | 0.000000854° |
| Maximum absolute tap error | 1.052253067 | 1.028232694 |
| RMS tap error | 0.036419281 | 0.035481868 |
| RMS error / reference RMS | 0.925868852 | 0.902037487 |
| Maximum peak-index ITD error | 21 samples | 17 samples |
| RMS peak-index ITD error | 5.522157 samples | 5.413995 samples |
| Maximum energy ILD error | 6.125233 dB | 5.920092 dB |
| RMS energy ILD error | 2.386662 dB | 2.289462 dB |
| Maximum sampled magnitude error | 36.654535 dB | 35.619334 dB |
| RMS sampled magnitude error | 5.530371 dB | 5.412372 dB |

ITD here is left minus right peak-absolute-tap index, not a perceptual
delay estimate. ILD is `10*log10(left_energy/right_energy)`. Magnitude
error uses the discrete-time Fourier transform at 200, 500, 1,000, 2,000,
4,000, 8,000, 12,000, and 16,000 Hz for both ears, with each magnitude
floored at -60 dB relative to amplitude 1 before subtraction.

The selected grid gives smaller errors while leaving 542,720 bytes of a
1 MiB data budget unused. It is a compact approximation: the phase-sensitive
tap error is large, and the sampled spectral errors show that interpolation
can also change coloration. These figures do not establish perceptual
equivalence to the full dataset. No listening test was performed.

f32 encoding preserves every selected f32 tap exactly; the measured maximum
rounding error from the median-plane averages was zero. For comparison,
globally scaled signed 16-bit encoding of the selected table (peak
0.943166852) would need 252,928 bytes plus its scale and introduce maximum
tap error 0.000014391854 and RMS error 0.000008210801. f32 was retained
because it already fits the size budget and avoids this extra quantization.

Regenerate and print these measurements from the repository root:

```powershell
cargo +1.95.0 run --release --manifest-path crates/backtracking-core/tools/sadie-table/Cargo.toml -- D:\Developing\OpenJOC\crates\openjoc-sofa\assets\sadie-ii-d1-ku100.ojhrtf crates/backtracking-core/src/spatial/sadie-d1.bin
```

Only this explicit offline regeneration command needs the source asset;
ordinary builds and tests use the checked-in reduced table.
