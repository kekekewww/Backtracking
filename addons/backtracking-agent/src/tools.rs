//! The MCP tools. Each one is a control command of the add-on contract (section 4.4 of
//! `docs/ADDON_CONTRACT.md`), described for a model that has never seen Backtracking.

use backtracking_addon::protocol::{ControlCommand, RowRef};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::Deserialize;

use crate::bridge::{Bridge, Outcome};
use crate::compact::{self, Shape};

/// What the server tells a client about Backtracking when it connects.
pub const INSTRUCTIONS: &str = "Backtracking is a retrospective audio sampler running on this \
computer. While capture is on, it keeps the last minutes of audio from its sources (system \
audio, microphones, single applications, plug-ins, add-ons) in a rolling history, so a stretch of \
something that has already been heard can still be taken. Each source is a lane named by a \
letter (A, B, C, ...), and each lane has one or more rows, one per channel. To take audio: call \
get_state to see what is held; pause_capture if the history should stop moving; select a \
stretch counted back from the newest audio (from_seconds_ago 10 is the last ten seconds); then \
save, which writes one WAV file per lane and returns their paths, the way dragging the clip out \
of the window does. clear_selection and start_capture carry on recording. The user sees \
everything you do in the window. Saving keeps the history; clear_history, add_source and \
remove_source throw it away.";

/// The server's name and version as clients see them.
pub fn server_config() -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(
            Implementation::new("backtracking", env!("CARGO_PKG_VERSION"))
                .with_title("Backtracking Agent"),
        )
        .with_instructions(INSTRUCTIONS)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectParams {
    /// Where the selection starts, in seconds back from the newest audio held. 10 selects
    /// from ten seconds ago. A value beyond what is held is cut to the oldest audio.
    #[serde(default)]
    pub from_seconds_ago: Option<f64>,
    /// Where it ends, in seconds back from the newest audio. Leave it out (0) to end at the
    /// newest audio. Must be smaller than from_seconds_ago.
    #[serde(default)]
    pub to_seconds_ago: Option<f64>,
    /// Instead of seconds: the first frame, as get_state reports frames (held.start_frame to
    /// held.end_frame). Give it together with end_frame.
    #[serde(default)]
    pub start_frame: Option<u64>,
    /// Instead of seconds: the frame after the last one selected.
    #[serde(default)]
    pub end_frame: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(inline)]
pub struct RowParam {
    /// The lane letter, for example "A".
    pub lane: String,
    /// The row in that lane, counted from 1 at the top.
    pub row: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChooseRowsParams {
    /// Every row of this lane only, for example "B".
    #[serde(default)]
    pub lane: Option<String>,
    /// The first of a run of rows, counted top to bottom across lanes. Give it with last.
    #[serde(default)]
    pub first: Option<RowParam>,
    /// The last of that run.
    #[serde(default)]
    pub last: Option<RowParam>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetViewParams {
    /// Show the newest this many seconds.
    #[serde(default)]
    pub seconds: Option<f64>,
    /// Instead: the first frame to show. Give it with end_frame.
    #[serde(default)]
    pub start_frame: Option<u64>,
    /// The frame after the last one to show.
    #[serde(default)]
    pub end_frame: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveParams {
    /// An absolute folder to write into, made when missing, for example "D:\\Clips". Leave it
    /// out to use the folder Backtracking's own Save uses (save_folder in get_state).
    #[serde(default)]
    pub folder: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddSourceParams {
    /// A source id exactly as list_sources returns it. Ids are opaque and may contain tab
    /// characters; pass them back unchanged.
    pub source: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemoveSourceParams {
    /// The lane letter to remove, for example "C".
    pub lane: String,
}

/// The tools of one MCP session, answered through the connection to Backtracking.
#[derive(Clone)]
pub struct AgentServer {
    bridge: Bridge,
    tool_router: ToolRouter<Self>,
}

impl AgentServer {
    pub fn new(bridge: Bridge) -> Self {
        Self {
            bridge,
            tool_router: Self::tool_router(),
        }
    }

    /// Every tool's definition, for the stdio forwarder, which answers `tools/list` itself.
    pub fn definitions() -> Vec<rmcp::model::Tool> {
        Self::tool_router().list_all()
    }

    async fn run(&self, command: ControlCommand, shape: Shape) -> Result<CallToolResult, McpError> {
        // Say what is wrong with the arguments before anything reaches Backtracking.
        if let Err(error) = command.validate() {
            return Ok(tool_error(format!("Invalid arguments: {error}")));
        }
        Ok(match self.bridge.run(command).await {
            Outcome::Done(raw) => CallToolResult::success(vec![ContentBlock::text(
                compact::success(shape, &raw).to_string(),
            )]),
            Outcome::Refused(error) => tool_error(compact::refusal(&error)),
            Outcome::Lost(reason) => tool_error(reason),
        })
    }
}

pub fn tool_error(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text.into())])
}

#[tool_router]
impl AgentServer {
    #[tool(
        description = "Read what Backtracking is doing now: whether capture is on and audio is \
        arriving, the sample rate, the lanes (letter, source, channels), how much audio is held \
        (held.seconds, and its frames), the selection and the view, and the folder save uses. \
        Changes nothing. Call it first.",
        annotations(title = "Get state", read_only_hint = true, open_world_hint = false)
    )]
    async fn get_state(&self) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::GetState, Shape::State).await
    }

    #[tool(
        description = "List the sources that can be added as a lane with add_source: system \
        audio, microphones, applications, plug-in senders and add-on sources, each with an \
        opaque id, a kind and a name. Also returns the state. Changes nothing.",
        annotations(title = "List sources", read_only_hint = true, open_world_hint = false)
    )]
    async fn list_sources(&self) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::ListSources, Shape::Sources).await
    }

    #[tool(
        description = "Start or resume capture, as the toolbar's start button does. New audio \
        is added to the history from now on. The audio already held stays.",
        annotations(
            title = "Start capture",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn start_capture(&self) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::StartCapture, Shape::State).await
    }

    #[tool(
        description = "Pause capture, as the toolbar's pause button does, so the history stops \
        moving while you select and save. The audio held stays. Remember to start_capture \
        again afterwards unless the user wants it paused.",
        annotations(
            title = "Pause capture",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn pause_capture(&self) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::PauseCapture, Shape::State).await
    }

    #[tool(
        description = "Throw away all the audio held, as Clear does. It cannot be undone: save \
        first whatever should be kept. Only do this when the user asks for it.",
        annotations(
            title = "Clear history",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn clear_history(&self) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::ClearHistory, Shape::State).await
    }

    #[tool(
        description = "Select a stretch of the audio held, as dragging across the waveform \
        does. Usually give from_seconds_ago (and to_seconds_ago, default 0): from_seconds_ago 5 \
        selects the last five seconds. Or give start_frame and end_frame from get_state. A \
        stretch longer than what is held is cut to it; the returned selection says what was \
        selected. While capture runs the newest audio moves on, so pause_capture first when \
        the stretch must not move.",
        annotations(
            title = "Select",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn select(
        &self,
        Parameters(params): Parameters<SelectParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(
            ControlCommand::Select {
                from_seconds_ago: params.from_seconds_ago,
                to_seconds_ago: params.to_seconds_ago,
                start_frame: params.start_frame,
                end_frame: params.end_frame,
            },
            Shape::State,
        )
        .await
    }

    #[tool(
        description = "Drop the selection, as Escape does. The audio held stays.",
        annotations(
            title = "Clear selection",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn clear_selection(&self) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::ClearSelection, Shape::State).await
    }

    #[tool(
        description = "Choose which rows (channels) the selection and save cover. Give lane for \
        every row of one lane, or first and last for a run of rows, or nothing to go back to \
        every row of every lane. Turns \"Edit separately\" on when needed.",
        annotations(
            title = "Choose rows",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn choose_rows(
        &self,
        Parameters(params): Parameters<ChooseRowsParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = |row: RowParam| RowRef {
            lane: row.lane,
            row: row.row,
        };
        self.run(
            ControlCommand::ChooseRows {
                lane: params.lane,
                first: params.first.map(row),
                last: params.last.map(row),
            },
            Shape::State,
        )
        .await
    }

    #[tool(
        description = "Change what the window shows, as zooming does: the newest seconds, or \
        start_frame to end_frame, or with nothing everything held. Only the view changes, not \
        the audio or the selection.",
        annotations(
            title = "Set view",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_view(
        &self,
        Parameters(params): Parameters<SetViewParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(
            ControlCommand::SetView {
                seconds: params.seconds,
                start_frame: params.start_frame,
                end_frame: params.end_frame,
            },
            Shape::State,
        )
        .await
    }

    #[tool(
        description = "Write the selection, or everything held when nothing is selected, as WAV \
        files: one file per lane, exactly as Backtracking's Save writes them. This is how to get \
        audio out of Backtracking, the equivalent of dragging the clip out of the window. Returns \
        each file's absolute path, lane, channels, sample rate, frames and seconds once they are \
        completely written. Writes into folder, or into the folder Save uses. The audio held is \
        kept.",
        annotations(
            title = "Save",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn save(
        &self,
        Parameters(params): Parameters<SaveParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::save(params.folder), Shape::Saved)
            .await
    }

    #[tool(
        description = "Play the selection through the user's speakers, as Space does. It makes \
        sound in the room, so only do it when the user asks to hear it.",
        annotations(
            title = "Play preview",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn play_preview(&self) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::PlayPreview, Shape::State).await
    }

    #[tool(
        description = "Stop playing the selection that play_preview started. Nothing else changes.",
        annotations(
            title = "Stop preview",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn stop_preview(&self) -> Result<CallToolResult, McpError> {
        self.run(ControlCommand::StopPreview, Shape::State).await
    }

    #[tool(
        description = "Add a source from list_sources as the next lane. This starts a new, \
        empty history, as changing the sources on Backtracking's Sources page does: all audio \
        held until now is thrown away (the reply says history_restarted). Save first whatever \
        should be kept, and only change sources when the user asks.",
        annotations(
            title = "Add source",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn add_source(
        &self,
        Parameters(params): Parameters<AddSourceParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(
            ControlCommand::AddSource {
                source: params.source,
            },
            Shape::State,
        )
        .await
    }

    #[tool(
        description = "Remove a lane; the lanes after it move up a letter. The only lane cannot \
        be removed. This starts a new, empty history: all audio held until now is thrown away \
        (the reply says history_restarted). Save first whatever should be kept, and only change \
        sources when the user asks.",
        annotations(
            title = "Remove source",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn remove_source(
        &self,
        Parameters(params): Parameters<RemoveSourceParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(
            ControlCommand::RemoveSource { lane: params.lane },
            Shape::State,
        )
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AgentServer {
    fn get_info(&self) -> ServerConfig {
        server_config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_command_has_a_tool_with_annotations() {
        let tools = AgentServer::definitions();
        let mut names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        names.sort_unstable();
        let mut commands = backtracking_addon::protocol::CONTROL_COMMANDS.to_vec();
        commands.sort_unstable();
        assert_eq!(names, commands);
        for tool in &tools {
            let annotations = tool.annotations.as_ref().expect("annotated");
            let read_only = annotations.read_only_hint == Some(true);
            let destructive = annotations.destructive_hint == Some(true);
            match tool.name.as_ref() {
                "get_state" | "list_sources" => assert!(read_only, "{}", tool.name),
                "clear_history" | "add_source" | "remove_source" => {
                    assert!(!read_only && destructive, "{}", tool.name)
                }
                _ => assert!(!read_only && !destructive, "{}", tool.name),
            }
            assert!(tool
                .description
                .as_ref()
                .is_some_and(|text| text.len() > 30));
        }
        let select = tools.iter().find(|tool| tool.name == "select").unwrap();
        let properties = select.input_schema.get("properties").unwrap();
        assert!(properties.get("from_seconds_ago").is_some());
        assert!(properties.get("end_frame").is_some());
    }
}
