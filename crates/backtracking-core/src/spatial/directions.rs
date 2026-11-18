use super::RenderError;

/// A listener-relative direction in degrees. Positive azimuth is right; positive elevation is up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Direction {
    pub azimuth: f32,
    pub elevation: f32,
}

/// Resolves a whole group's speaker labels, preserving channel order.
///
/// Labels are ASCII case-insensitive. Surrounds are at 90 degrees when a back pair is
/// present, and 110 degrees otherwise. Unknown labels, including headphone-ear names,
/// and LFE channels have no direction. Resolve before selecting a subset of channels.
///
/// The 22.2-only names follow the nominal positions in ITU-R BS.2051-3 Tables 1 and 10.
/// `FL`/`FR` are at 60 degrees when both front-centre speakers are present.
/// Windows' additional `TC`, `TFC`, and `TBC` names use 90, 45, and 45 degree elevation.
pub fn speaker_directions(labels: &[&str]) -> Vec<Option<Direction>> {
    let names: Vec<String> = labels.iter().map(|label| canonical(label)).collect();
    let has = |name: &str| names.iter().any(|label| label == name);
    let front = if has("flc") && has("frc") { 60.0 } else { 30.0 };
    let surround = if has("lb") && has("rb") { 90.0 } else { 110.0 };
    names
        .iter()
        .map(|label| {
            let (azimuth, elevation) = match label.as_str() {
                "fl" => (-front, 0.0),
                "fr" => (front, 0.0),
                "flc" => (-30.0, 0.0),
                "frc" => (30.0, 0.0),
                "fc" => (0.0, 0.0),
                "ls" => (-surround, 0.0),
                "rs" => (surround, 0.0),
                "lb" | "bl" => (-135.0, 0.0),
                "rb" | "br" => (135.0, 0.0),
                "lw" => (-60.0, 0.0),
                "rw" => (60.0, 0.0),
                "sil" => (-90.0, 0.0),
                "sir" => (90.0, 0.0),
                "bc" => (180.0, 0.0),
                "tfl" | "ltf" => (-45.0, 45.0),
                "tfr" | "rtf" => (45.0, 45.0),
                "ltm" => (-90.0, 60.0),
                "rtm" => (90.0, 60.0),
                "tbl" | "ltr" => (-135.0, 45.0),
                "tbr" | "rtr" => (135.0, 45.0),
                "tpfl" => (-45.0, 30.0),
                "tpfr" => (45.0, 30.0),
                "tpfc" => (0.0, 30.0),
                "tpc" | "tc" => (0.0, 90.0),
                "tpbl" => (-135.0, 30.0),
                "tpbr" => (135.0, 30.0),
                "tpsil" => (-90.0, 30.0),
                "tpsir" => (90.0, 30.0),
                "tpbc" => (180.0, 30.0),
                "btfc" => (0.0, -30.0),
                "btfl" => (-45.0, -30.0),
                "btfr" => (45.0, -30.0),
                "tfc" => (0.0, 45.0),
                "tbc" => (180.0, 45.0),
                _ => return None,
            };
            Some(Direction { azimuth, elevation })
        })
        .collect()
}

/// The name this module knows a speaker by: lower case, with the other common spellings of
/// the same speaker folded onto one. `L`, `R`, `C` are the fronts; `Lrs`/`Rrs` (and `BL`/`BR`,
/// `Lsr`/`Rsr`) the back pair; `Lss`/`Rss` (and `SL`/`SR`) the side surrounds; `Lc`/`Rc` the
/// front-centre pair; `Cs` the back centre; the height spellings map onto the top speakers.
fn canonical(label: &str) -> String {
    let lower = label.trim().to_ascii_lowercase();
    let known = match lower.as_str() {
        "l" | "left" => "fl",
        "r" | "right" => "fr",
        "c" | "center" | "centre" => "fc",
        "lc" => "flc",
        "rc" => "frc",
        "lss" | "sl" => "ls",
        "rss" | "sr" => "rs",
        "lrs" | "lsr" | "bl" => "lb",
        "rrs" | "rsr" | "br" => "rb",
        "cs" => "bc",
        "lfh" | "ltf" => "tfl",
        "rfh" | "rtf" => "tfr",
        "lts" | "tsl" => "ltm",
        "rts" | "tsr" => "rtm",
        "lrh" | "ltb" | "ltr" => "tbl",
        "rrh" | "rtb" | "rtr" => "tbr",
        _ => return lower,
    };
    known.to_owned()
}

pub(crate) fn is_lfe(label: &str) -> bool {
    ["LFE", "LFE1", "LFE2"]
        .iter()
        .any(|name| label.eq_ignore_ascii_case(name))
}

/// Builds normalised stereo weights in the same order as `labels`.
///
/// Front left/right have unit weights; LFE feeds both sides at 0.5. Other side
/// channels feed their own side at 1/sqrt(2); centre and unknown names feed both
/// sides at 1/sqrt(2). All weights share the larger column sum as their divisor
/// when it exceeds one. This matches the Windows capture downmix, including its
/// f32 rounding (a full-scale sum can exceed one by a few rounding bits).
pub fn stereo_weights(labels: &[&str]) -> Vec<[f32; 2]> {
    let centre = std::f32::consts::FRAC_1_SQRT_2;
    let mut weights: Vec<_> = labels
        .iter()
        .map(|label| {
            if is_lfe(label) {
                return [0.5, 0.5];
            }
            match canonical(label).as_str() {
                "fl" => [1.0, 0.0],
                "fr" => [0.0, 1.0],
                "ls" | "lb" | "bl" | "lw" | "flc" | "sil" | "tfl" | "ltf" | "ltm" | "tbl"
                | "ltr" | "tpfl" | "tpbl" | "tpsil" | "btfl" => [centre, 0.0],
                "rs" | "rb" | "br" | "rw" | "frc" | "sir" | "tfr" | "rtf" | "rtm" | "tbr"
                | "rtr" | "tpfr" | "tpbr" | "tpsir" | "btfr" => [0.0, centre],
                _ => [centre, centre],
            }
        })
        .collect();
    let left: f32 = weights.iter().map(|weight| weight[0]).sum();
    let right: f32 = weights.iter().map(|weight| weight[1]).sum();
    let divisor = left.max(right).max(1.0);
    for weight in &mut weights {
        weight[0] /= divisor;
        weight[1] /= divisor;
    }
    weights
}

/// Folds interleaved N-channel input into interleaved stereo, without allocating.
///
/// `weights.len()` is N and must be nonzero. Both buffers must contain exactly the
/// same number of complete frames. Invalid buffers are left untouched. Weights
/// normally come from [`stereo_weights`]; this function does not renormalise them.
pub fn fold_down(
    input: &[f32],
    weights: &[[f32; 2]],
    output: &mut [f32],
) -> Result<(), RenderError> {
    let channels = weights.len();
    if channels == 0
        || input.len() % channels != 0
        || (input.len() / channels).checked_mul(2) != Some(output.len())
    {
        return Err(RenderError::InvalidBuffer);
    }
    for (frame, stereo) in input.chunks_exact(channels).zip(output.chunks_exact_mut(2)) {
        let mut sum = [0.0f64; 2];
        for (&sample, weight) in frame.iter().zip(weights) {
            sum[0] += f64::from(sample) * f64::from(weight[0]);
            sum[1] += f64::from(sample) * f64::from(weight[1]);
        }
        stereo[0] = sum[0] as f32;
        stereo[1] = sum[1] as f32;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent catalog copied from joc-decode's format.rs. Neither the build
    // nor these tests reads that checkout or depends on OpenJOC.
    const FORMATS: &[&[&str]] = &[
        &["Left Ear", "Right Ear"],
        &["FL", "FR"],
        &["FL", "FR"],
        &["FL", "FR", "FC", "LFE", "Ls", "Rs"],
        &["FL", "FR", "FC", "LFE", "Ls", "Rs", "TFL", "TFR"],
        &[
            "FL", "FR", "FC", "LFE", "Ls", "Rs", "TFL", "TFR", "TBL", "TBR",
        ],
        &["FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs"],
        &[
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "TFL", "TFR",
        ],
        &[
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "TFL", "TFR", "TBL", "TBR",
        ],
        &[
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "Ltf", "Rtf", "Ltm", "Rtm", "Ltr",
            "Rtr",
        ],
        &["FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "Lw", "Rw"],
        &[
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "Lw", "Rw", "Ltm", "Rtm",
        ],
        &[
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "Lw", "Rw", "Ltf", "Rtf", "Ltr", "Rtr",
        ],
        &[
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "Lw", "Rw", "Ltf", "Rtf", "Ltm",
            "Rtm", "Ltr", "Rtr",
        ],
        &[
            "FL", "FR", "FC", "LFE1", "BL", "BR", "FLc", "FRc", "BC", "LFE2", "SiL", "SiR", "TpFL",
            "TpFR", "TpFC", "TpC", "TpBL", "TpBR", "TpSiL", "TpSiR", "TpBC", "BtFC", "BtFL",
            "BtFR",
        ],
    ];

    #[test]
    fn every_format_label_has_the_expected_direction_or_explicit_fallback() {
        let expected = [
            ("FL", -30.0, 0.0),
            ("FR", 30.0, 0.0),
            ("FC", 0.0, 0.0),
            ("Lb", -135.0, 0.0),
            ("Rb", 135.0, 0.0),
            ("Lw", -60.0, 0.0),
            ("Rw", 60.0, 0.0),
            ("TFL", -45.0, 45.0),
            ("TFR", 45.0, 45.0),
            ("Ltf", -45.0, 45.0),
            ("Rtf", 45.0, 45.0),
            ("Ltm", -90.0, 60.0),
            ("Rtm", 90.0, 60.0),
            ("TBL", -135.0, 45.0),
            ("TBR", 135.0, 45.0),
            ("Ltr", -135.0, 45.0),
            ("Rtr", 135.0, 45.0),
            ("BL", -135.0, 0.0),
            ("BR", 135.0, 0.0),
            ("FLc", -30.0, 0.0),
            ("FRc", 30.0, 0.0),
            ("BC", 180.0, 0.0),
            ("SiL", -90.0, 0.0),
            ("SiR", 90.0, 0.0),
            ("TpFL", -45.0, 30.0),
            ("TpFR", 45.0, 30.0),
            ("TpFC", 0.0, 30.0),
            ("TpC", 0.0, 90.0),
            ("TpBL", -135.0, 30.0),
            ("TpBR", 135.0, 30.0),
            ("TpSiL", -90.0, 30.0),
            ("TpSiR", 90.0, 30.0),
            ("TpBC", 180.0, 30.0),
            ("BtFC", 0.0, -30.0),
            ("BtFL", -45.0, -30.0),
            ("BtFR", 45.0, -30.0),
        ];
        for labels in FORMATS {
            let resolved = speaker_directions(labels);
            for (label, actual) in labels.iter().zip(resolved) {
                let wanted = if ["LFE", "LFE1", "LFE2", "Left Ear", "Right Ear"].contains(label) {
                    None
                } else if labels.contains(&"FLc")
                    && labels.contains(&"FRc")
                    && matches!(*label, "FL" | "FR")
                {
                    Some(Direction {
                        azimuth: if *label == "FL" { -60.0 } else { 60.0 },
                        elevation: 0.0,
                    })
                } else if *label == "Ls" || *label == "Rs" {
                    let side = if *label == "Ls" { -1.0 } else { 1.0 };
                    Some(Direction {
                        azimuth: side * if labels.contains(&"Lb") { 90.0 } else { 110.0 },
                        elevation: 0.0,
                    })
                } else {
                    let (_, azimuth, elevation) = expected
                        .iter()
                        .find(|entry| entry.0 == *label)
                        .expect(label);
                    Some(Direction {
                        azimuth: *azimuth,
                        elevation: *elevation,
                    })
                };
                assert_eq!(actual, wanted, "{label} in {labels:?}");
            }
            let lower: Vec<_> = labels.iter().map(|s| s.to_ascii_lowercase()).collect();
            let lower: Vec<_> = lower.iter().map(String::as_str).collect();
            assert_eq!(speaker_directions(labels), speaker_directions(&lower));
            assert_eq!(stereo_weights(labels), stereo_weights(&lower));
        }
        assert_eq!(
            speaker_directions(&["Object 1", "Ch27", "", "unrecognised"]),
            [None; 4]
        );
    }

    #[test]
    fn all_left_right_pairs_mirror_and_surrounds_depend_on_the_whole_group() {
        for [left, right] in [
            ["FL", "FR"],
            ["Ls", "Rs"],
            ["Lb", "Rb"],
            ["Lw", "Rw"],
            ["TFL", "TFR"],
            ["Ltf", "Rtf"],
            ["Ltm", "Rtm"],
            ["TBL", "TBR"],
            ["Ltr", "Rtr"],
            ["BL", "BR"],
            ["FLc", "FRc"],
            ["SiL", "SiR"],
            ["TpFL", "TpFR"],
            ["TpBL", "TpBR"],
            ["TpSiL", "TpSiR"],
            ["BtFL", "BtFR"],
        ] {
            let pair = speaker_directions(&[left, right]);
            let left = pair[0].unwrap();
            let right = pair[1].unwrap();
            assert_eq!(left.azimuth, -right.azimuth);
            assert_eq!(left.elevation, right.elevation);
        }
        for back in [["lb", "RB"], ["BL", "br"]] {
            let labels = ["ls", back[0], "RS", back[1]];
            assert_eq!(speaker_directions(&labels)[0].unwrap().azimuth, -90.0);
            assert_eq!(speaker_directions(&labels)[2].unwrap().azimuth, 90.0);
        }
        assert_eq!(
            speaker_directions(&["Ls", "Lb"])[0].unwrap().azimuth,
            -110.0
        );
    }

    // Windows WAVEFORMATEXTENSIBLE bits in ascending order. Unassigned bits
    // still follow loopback.rs's centre-like rule and get unknown labels.
    const WINDOWS_LABELS: [&str; 32] = [
        "FL", "FR", "FC", "LFE", "Lb", "Rb", "FLc", "FRc", "BC", "Ls", "Rs", "TC", "TFL", "TFC",
        "TFR", "TBL", "TBC", "TBR", "Bit18", "Bit19", "Bit20", "Bit21", "Bit22", "Bit23", "Bit24",
        "Bit25", "Bit26", "Bit27", "Bit28", "Bit29", "Bit30", "Bit31",
    ];

    #[test]
    fn fold_weights_match_independent_windows_numbers_for_masks() {
        // Literal expected values, not the production matching code. 0.70710677
        // is the f32 value used by the existing Windows downmix.
        const C: f32 = 0.707_106_77;
        const EXPECTED: [[f32; 2]; 32] = [
            [1.0, 0.0],
            [0.0, 1.0],
            [C, C],
            [0.5, 0.5],
            [C, 0.0],
            [0.0, C],
            [C, 0.0],
            [0.0, C],
            [C, C],
            [C, 0.0],
            [0.0, C],
            [C, C],
            [C, 0.0],
            [C, C],
            [0.0, C],
            [C, 0.0],
            [C, C],
            [0.0, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
            [C, C],
        ];
        let check = |mask: u32| {
            let bits: Vec<_> = (0..32).filter(|bit| mask & (1 << bit) != 0).collect();
            let labels: Vec<_> = bits.iter().map(|&bit| WINDOWS_LABELS[bit]).collect();
            let mut expected: Vec<_> = bits.iter().map(|&bit| EXPECTED[bit]).collect();
            let divisor = expected
                .iter()
                .map(|w| w[0])
                .sum::<f32>()
                .max(expected.iter().map(|w| w[1]).sum::<f32>())
                .max(1.0);
            for weight in &mut expected {
                weight[0] /= divisor;
                weight[1] /= divisor;
            }
            assert_eq!(stereo_weights(&labels), expected, "mask {mask:08x}");
        };
        // Exhaust every combination of the 18 named Windows speaker bits.
        for mask in 0..(1 << 18) {
            check(mask);
        }
        for bit in 18..32 {
            check(1 << bit);
            check((1 << bit) | 0x3ffff);
        }
        // Include arbitrary combinations of reserved and named bits.
        let mut mask = u32::MAX;
        for _ in 0..4096 {
            check(mask);
            mask = mask.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        }
    }

    #[test]
    fn fold_down_preserves_length_and_full_scale_within_float_rounding() {
        for labels in FORMATS.iter().copied().chain([WINDOWS_LABELS.as_slice()]) {
            let weights = stereo_weights(labels);
            let mut input = vec![1.0; labels.len() * 3];
            input[labels.len()..labels.len() * 2].fill(-1.0);
            for (i, sample) in input[labels.len() * 2..].iter_mut().enumerate() {
                *sample = if i % 2 == 0 { -1.0 } else { 1.0 };
            }
            let mut output = [99.0; 6];
            fold_down(&input, &weights, &mut output).unwrap();
            assert!(output.iter().all(|sample| sample.abs() <= 1.0 + 1e-6));
            assert_eq!(output[0], -output[2]);
            assert_eq!(output[1], -output[3]);
        }
        let mut output = [0.0; 4];
        fold_down(
            &[0.5, -0.25, -0.5, 0.25],
            &stereo_weights(&["FL", "FR"]),
            &mut output,
        )
        .unwrap();
        assert_eq!(output, [0.5, -0.25, -0.5, 0.25]);
    }

    #[test]
    fn fold_down_rejects_partial_or_mismatched_frames_without_writing() {
        let mut output = [9.0; 2];
        for (input, weights) in [
            (&[1.0][..], &[][..]),
            (&[1.0][..], &[[1.0, 0.0], [0.0, 1.0]][..]),
            (&[1.0, 2.0][..], &[[1.0, 0.0]][..]),
        ] {
            assert_eq!(
                fold_down(input, weights, &mut output),
                Err(RenderError::InvalidBuffer)
            );
            assert_eq!(output, [9.0; 2]);
        }
        assert_eq!(fold_down(&[], &[[1.0, 0.0]], &mut []), Ok(()));
    }
}
