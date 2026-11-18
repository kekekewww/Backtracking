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
| Backtracking `sadie-d1.bin` | 843,744 | `f9d228c0e3382fac23362941603fb82be2484c3e9aa209f552126a5b86a968ba` |

### Changes made for Backtracking

The generator is the separate, dependency-free Cargo project at
`crates/backtracking-core/tools/sadie-table`. Its own `[workspace]` prevents
membership in the application workspace. It parses the asset format directly,
without fetching or linking OpenJOC. The input has 8,802 measurements followed
by 15 OpenJOC virtual-speaker aliases; only the 8,802 measured rows are searched.
The checked source contains zero separate sample delays. Its acoustic delays
remain in the 256 taps; an input with nonzero separate delays is rejected.

The reduced bank retains 816 directions. The 22 non-polar elevation rings each
contain azimuths 0 through 180 degrees every 5 degrees. Each pole has one
record. The elevations in ascending order are -90, -81, -75, -64.8, -60,
-54, -45, -35.3, -30, -25, -17.5, -15, 0, 15, 17.5, 25, 30, 35.3, 45, 54,
60, 64.8, 75, and 90 degrees. Positive azimuth is right, zero is ahead, and
positive elevation is up. Each grid node uses the closest measured unit
direction by dot product. On the median planes and the poles, the two measured
ears are averaged. Negative azimuth uses the opposite ear of the stored row.

Records are ordered by elevation, azimuth, then ear (left first). Each ear
stores a little-endian f32 scale (maximum absolute tap / 32767) followed by
all 256 taps rounded to signed little-endian i16 at that scale. Maximum tap
quantization error is 0.000014320016 and RMS is 0.000002973434. The largest
change at the 247 previous grid nodes equals that maximum quantization error.

The last 1,632 bytes hold one unsigned integer alignment delay in 48 kHz
samples per ear, in record order. To obtain the delays, the generator finds
the strongest positive normalized cross-correlation over lags -16 through
16 samples for each horizontal and vertical grid edge. A weighted least
squares fit gives each node one delay shared by its incident edges. The
weight is the fourth power of the correlation floored at 0.1. The fit starts
at absolute peak indices and uses 500 Gauss-Seidel sweeps before rounding.
Median-plane and pole ears share a delay.

At other sample rates the renderer resamples each FIR once and scales and
rounds its alignment delay. To look up a direction it uses linear weights on each axis,
aligns the four FIRs to the integer part of the weighted
delay, blends them, and applies the fractional part with four-point Lagrange
interpolation. Taps beyond the FIR ends are zero. The interpolated ear
response is scaled to the weighted mean of the four measured FIR norms.
Grid nodes return the prepared taps directly.

### Measured reduction error

Against W1-A's 864 between-grid directions, RMS peak-index interaural timing
error fell from 5.414 to 4.651 samples, RMS interaural level error from 2.289
to 1.287 dB, and RMS sampled magnitude error from 5.412 to 3.038 dB.
The reference is the nearest original measurement before symmetry changes.
The generator also prints the results on a separate development set, maximum
errors, and lower bounds from the original measurements' left/right asymmetry.
All measurements use the source rate before the render gain. Magnitudes use
both ears at 200, 500, 1000, 2000, 4000, 8000, 12000 and 16000 Hz with a
-60 dB absolute magnitude floor. Nobody listened to these comparisons.

Regenerate and print these measurements from the repository root:

```powershell
cargo +1.95.0 run --release --manifest-path crates/backtracking-core/tools/sadie-table/Cargo.toml -- D:\Developing\OpenJOC\crates\openjoc-sofa\assets\sadie-ii-d1-ku100.ojhrtf crates/backtracking-core/src/spatial/sadie-d1.bin
```

Only this explicit offline regeneration command needs the source asset;
ordinary builds and tests use the checked-in reduced table.
