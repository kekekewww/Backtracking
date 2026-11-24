//! Turns the host's control replies into what a model reads: the state trimmed to what helps
//! it act, the files a save wrote, and errors with their stable code, the host's message and
//! what to do about it.

use backtracking_addon::protocol::{control_codes, ControlError, ControlResult, ControlState};
use serde_json::{json, Map, Value};

/// What a tool returns besides the state. Every reply also says when the history was
/// restarted, as `add_source` and `remove_source` do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Only the state.
    State,
    /// `list_sources`: the sources that can be added, then the state.
    Sources,
    /// `save`: the folder and the files written, then the state.
    Saved,
}

/// The state as a model needs it: lanes without their opaque source ids, no empty or null
/// fields, and no drag folder (a model saves with `save`).
pub fn state(state: &ControlState) -> Value {
    let mut out = Map::new();
    out.insert("capturing".into(), state.capturing.into());
    out.insert("recording".into(), state.recording.into());
    if let Some(problem) = &state.problem {
        out.insert("problem".into(), problem.as_str().into());
    }
    out.insert("sample_rate".into(), state.sample_rate.into());
    let lanes: Vec<Value> = state
        .lanes
        .iter()
        .map(|lane| {
            let mut entry = Map::new();
            entry.insert("lane".into(), lane.lane.as_str().into());
            entry.insert("source".into(), lane.source.name.as_str().into());
            entry.insert("kind".into(), lane.source.kind.as_str().into());
            if let Some(channels) = lane.channels {
                entry.insert("channels".into(), channels.into());
            }
            entry.insert("route".into(), lane.route.as_str().into());
            if let Some(problem) = &lane.problem {
                entry.insert("problem".into(), problem.as_str().into());
            }
            Value::Object(entry)
        })
        .collect();
    out.insert("lanes".into(), lanes.into());
    out.insert(
        "held".into(),
        json!({
            "seconds": round(state.history.seconds),
            "start_frame": state.history.start_frame,
            "end_frame": state.history.end_frame,
        }),
    );
    out.insert(
        "selection".into(),
        match &state.selection {
            Some(selection) => json!({
                "seconds": round(selection.seconds),
                "from_seconds_ago": round(selection.from_seconds_ago),
                "to_seconds_ago": round(selection.to_seconds_ago),
                "start_frame": selection.start_frame,
                "end_frame": selection.end_frame,
            }),
            None => Value::Null,
        },
    );
    out.insert(
        "view".into(),
        json!({
            "seconds": round(state.view.seconds),
            "start_frame": state.view.start_frame,
            "end_frame": state.view.end_frame,
        }),
    );
    if state.separate_rows {
        out.insert("separate_rows".into(), true.into());
        let rows: Vec<Value> = state
            .chosen_rows
            .iter()
            .map(|row| json!({"lane": row.lane, "row": row.row, "label": row.label}))
            .collect();
        out.insert("chosen_rows".into(), rows.into());
    }
    if state.preview_playing {
        out.insert("preview_playing".into(), true.into());
    }
    if !state.save_folder.is_empty() {
        out.insert("save_folder".into(), state.save_folder.as_str().into());
    }
    if let Some(busy) = &state.busy {
        out.insert("busy".into(), busy.as_str().into());
    }
    Value::Object(out)
}

/// What a successful tool call returns, from the host's raw `result`. A result this version
/// cannot read is passed on as it came.
pub fn success(shape: Shape, raw: &Value) -> Value {
    let Ok(result) = serde_json::from_value::<ControlResult>(raw.clone()) else {
        return raw.clone();
    };
    let mut out = Map::new();
    match shape {
        Shape::State => {}
        Shape::Sources => {
            let sources: Vec<Value> = result
                .sources
                .iter()
                .flatten()
                .map(|source| json!({"id": source.id, "kind": source.kind, "name": source.name}))
                .collect();
            out.insert("sources".into(), sources.into());
        }
        Shape::Saved => {
            if let Some(folder) = &result.folder {
                out.insert("folder".into(), folder.as_str().into());
            }
            let files: Vec<Value> = result
                .files
                .iter()
                .flatten()
                .map(|file| {
                    let mut entry = Map::new();
                    entry.insert("path".into(), file.path.as_str().into());
                    if file.kind != "wav" {
                        entry.insert("kind".into(), file.kind.as_str().into());
                    }
                    if let Some(lane) = &file.lane {
                        entry.insert("lane".into(), lane.as_str().into());
                    }
                    if let Some(channels) = file.channels {
                        entry.insert("channels".into(), channels.into());
                    }
                    if let Some(rate) = file.sample_rate {
                        entry.insert("sample_rate".into(), rate.into());
                    }
                    if let Some(frames) = file.frames {
                        entry.insert("frames".into(), frames.into());
                        if let Some(rate) = file.sample_rate.filter(|rate| *rate > 0) {
                            entry.insert(
                                "seconds".into(),
                                round(frames as f64 / f64::from(rate)).into(),
                            );
                        }
                    }
                    Value::Object(entry)
                })
                .collect();
            out.insert("files".into(), files.into());
        }
    }
    if result.history_restarted {
        out.insert("history_restarted".into(), true.into());
        out.insert(
            "note".into(),
            "The audio held before this was thrown away; capture started a new, empty history."
                .into(),
        );
    }
    out.insert("state".into(), self::state(&result.state));
    Value::Object(out)
}

/// The text of a tool error for a refusal from Backtracking: the code, the host's message, and
/// what to do next. A code this version does not know is shown as it came, like `failed`.
pub fn refusal(error: &ControlError) -> String {
    let hint = match error.code.as_str() {
        control_codes::NOT_ALLOWED => Some(
            "Ask the user to switch on \"Let this add-on control Backtracking\" for Backtracking \
             Agent in Backtracking's Options > Add-ons, then try again.",
        ),
        control_codes::BUSY => {
            Some("The user is dragging or a save is being written. Wait a moment and try again.")
        }
        control_codes::NO_AUDIO => Some(
            "Nothing has been captured there yet. Call start_capture, let some audio arrive, \
             and try again.",
        ),
        control_codes::NO_SELECTION => Some("Select a stretch with select first."),
        control_codes::NOT_FOUND => Some("Call get_state or list_sources to see what exists."),
        control_codes::TIMEOUT => {
            Some("Backtracking did not answer in time; call get_state to see whether it happened.")
        }
        control_codes::TOO_MANY_REQUESTS => Some("Wait for earlier calls to finish."),
        _ => None,
    };
    let mut text = format!("Backtracking refused ({}): {}", error.code, error.message);
    if let Some(hint) = hint {
        // The host's message may end without a full stop.
        if !text.ends_with(['.', '!', '?']) {
            text.push('.');
        }
        text.push(' ');
        text.push_str(hint);
    }
    text
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtracking_addon::protocol::{
        ControlReply, FrameSpan, LaneState, RowState, SavedFile, SelectionState, SourceInfo,
    };

    fn sample() -> ControlState {
        ControlState {
            capturing: false,
            recording: false,
            problem: None,
            sample_rate: 48_000,
            lanes: vec![LaneState {
                lane: "A".into(),
                source: SourceInfo {
                    id: "addon\tcom.example.test-tone\tmain".into(),
                    kind: "addon".into(),
                    name: "Test tone".into(),
                },
                channels: Some(2),
                route: "merged".into(),
                problem: None,
            }],
            history: FrameSpan {
                start_frame: 0,
                end_frame: 327_840,
                seconds: 6.83,
            },
            selection: Some(SelectionState {
                start_frame: 231_840,
                end_frame: 327_840,
                seconds: 2.0,
                from_seconds_ago: 2.0,
                to_seconds_ago: 0.0,
            }),
            view: FrameSpan {
                start_frame: 0,
                end_frame: 327_840,
                seconds: 6.83,
            },
            separate_rows: false,
            chosen_rows: vec![],
            preview_playing: false,
            save_folder: "D:\\Clips".into(),
            drag_folder: "C:\\Users\\me\\AppData\\Local\\Backtracking\\exports\\output".into(),
            busy: None,
        }
    }

    #[test]
    fn the_state_keeps_what_a_model_acts_on_and_drops_the_rest() {
        let value = state(&sample());
        assert_eq!(
            value,
            json!({
                "capturing": false, "recording": false, "sample_rate": 48000,
                "lanes": [{"lane": "A", "source": "Test tone", "kind": "addon", "channels": 2,
                           "route": "merged"}],
                "held": {"seconds": 6.83, "start_frame": 0, "end_frame": 327840},
                "selection": {"seconds": 2.0, "from_seconds_ago": 2.0, "to_seconds_ago": 0.0,
                              "start_frame": 231840, "end_frame": 327840},
                "view": {"seconds": 6.83, "start_frame": 0, "end_frame": 327840},
                "save_folder": "D:\\Clips"
            })
        );
        let text = value.to_string();
        assert!(!text.contains("drag_folder") && !text.contains('\t'));

        let mut other = sample();
        other.selection = None;
        other.separate_rows = true;
        other.chosen_rows = vec![RowState {
            lane: "A".into(),
            row: 2,
            label: "R".into(),
        }];
        other.busy = Some("a save is being written".into());
        other.problem = Some("No audio device".into());
        let value = state(&other);
        assert_eq!(value["selection"], Value::Null);
        assert_eq!(
            value["chosen_rows"],
            json!([{"lane": "A", "row": 2, "label": "R"}])
        );
        assert_eq!(value["busy"], "a save is being written");
        assert_eq!(value["problem"], "No audio device");
    }

    #[test]
    fn a_save_lists_its_files_with_lanes_and_lengths() {
        let result = ControlResult {
            state: sample(),
            folder: Some("D:\\Clips".into()),
            files: Some(vec![
                SavedFile {
                    path: "D:\\Clips\\A.wav".into(),
                    kind: "wav".into(),
                    lane: Some("A".into()),
                    channels: Some(2),
                    sample_rate: Some(48_000),
                    frames: Some(96_000),
                },
                SavedFile {
                    path: "D:\\Clips\\B.positions.json".into(),
                    kind: "positions".into(),
                    lane: Some("B".into()),
                    channels: None,
                    sample_rate: None,
                    frames: None,
                },
            ]),
            ..ControlResult::default()
        };
        let raw = ControlReply::success("1", &result).result.unwrap();
        let value = success(Shape::Saved, &raw);
        assert_eq!(value["folder"], "D:\\Clips");
        assert_eq!(
            value["files"],
            json!([
                {"path": "D:\\Clips\\A.wav", "lane": "A", "channels": 2, "sample_rate": 48000,
                 "frames": 96000, "seconds": 2.0},
                {"path": "D:\\Clips\\B.positions.json", "kind": "positions", "lane": "B"}
            ])
        );
        assert_eq!(value["state"]["lanes"][0]["lane"], "A");
        assert!(value.get("history_restarted").is_none());
    }

    #[test]
    fn sources_keep_their_ids_and_a_restart_is_said_plainly() {
        let result = ControlResult {
            state: sample(),
            sources: Some(vec![SourceInfo {
                id: "addon\tcom.example.test-tone\tmain".into(),
                kind: "addon".into(),
                name: "Test tone".into(),
            }]),
            ..ControlResult::default()
        };
        let raw = serde_json::to_value(&result).unwrap();
        let value = success(Shape::Sources, &raw);
        assert_eq!(
            value["sources"][0]["id"],
            "addon\tcom.example.test-tone\tmain"
        );

        let restarted = ControlResult {
            state: sample(),
            history_restarted: true,
            ..ControlResult::default()
        };
        let value = success(Shape::State, &serde_json::to_value(&restarted).unwrap());
        assert_eq!(value["history_restarted"], true);
        assert!(value["note"].as_str().unwrap().contains("thrown away"));
    }

    #[test]
    fn unknown_fields_are_ignored_and_an_unreadable_result_passes_through() {
        let mut raw = serde_json::to_value(ControlResult {
            state: sample(),
            ..ControlResult::default()
        })
        .unwrap();
        raw["later_field"] = json!(1);
        raw["state"]["later_state"] = json!("x");
        let value = success(Shape::State, &raw);
        assert_eq!(value["state"]["sample_rate"], 48000);
        let odd = json!({"something": "else"});
        assert_eq!(success(Shape::State, &odd), odd);
    }

    #[test]
    fn refusals_carry_code_message_and_what_to_do() {
        let text = refusal(&ControlError::new(
            control_codes::NOT_ALLOWED,
            "Control is not allowed",
        ));
        assert!(text.starts_with("Backtracking refused (not_allowed): Control is not allowed. Ask"));
        assert!(text.contains("Let this add-on control Backtracking"));
        assert!(text.contains("Options > Add-ons"));
        // A code from a later Backtracking is shown as it came, without a hint.
        let text = refusal(&ControlError::new("something_new", "Later reason"));
        assert_eq!(text, "Backtracking refused (something_new): Later reason");
    }
}
