//! Runs the built add-on against a stand-in Backtracking (the SDK's host side), then talks MCP
//! to it as a client would: over Streamable HTTP with rmcp's client, and through `--stdio`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use backtracking_addon::host::{Connection, Host, HostEvent};
use backtracking_addon::protocol::{
    control_codes, AddonMessage, ControlCommand, ControlError, ControlReply, ControlResult,
    ControlState, FrameSpan, HostMessage, LaneState, MessageLevel, SavedFile, SelectionState,
    SourceInfo,
};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Value};

const ID: &str = "com.kamelkyp.backtracking.agent";
const PROGRAM: &str = env!("CARGO_BIN_EXE_backtracking-agent");

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// What the stand-in host saw and what the test asks it to send.
struct FakeHost {
    seen: Arc<Mutex<Vec<ControlCommand>>>,
    messages: Arc<Mutex<Vec<(MessageLevel, String)>>>,
    send: mpsc::Sender<HostMessage>,
    done: thread::JoinHandle<()>,
}

fn state(selection: Option<SelectionState>, lanes: usize) -> ControlState {
    ControlState {
        capturing: true,
        recording: true,
        sample_rate: 48_000,
        lanes: (0..lanes)
            .map(|index| LaneState {
                lane: ["A", "B", "C"][index].into(),
                source: SourceInfo {
                    id: "addon\tcom.example.test-tone\tmain".into(),
                    kind: "addon".into(),
                    name: "Test tone".into(),
                },
                channels: Some(2),
                route: "merged".into(),
                problem: None,
            })
            .collect(),
        history: FrameSpan {
            start_frame: 0,
            end_frame: 480_000,
            seconds: 10.0,
        },
        selection,
        view: FrameSpan {
            start_frame: 0,
            end_frame: 480_000,
            seconds: 10.0,
        },
        save_folder: "C:\\Clips".into(),
        drag_folder: "C:\\drag".into(),
        ..ControlState::default()
    }
}

/// Answers control requests the way Backtracking would, refusing them while control is off.
fn serve(mut connection: Connection, folder: PathBuf) -> FakeHost {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let messages = Arc::new(Mutex::new(Vec::new()));
    let (send, outgoing) = mpsc::channel::<HostMessage>();
    let (seen2, messages2) = (seen.clone(), messages.clone());
    let done = thread::spawn(move || {
        let mut allowed = false;
        let mut lanes = 2;
        loop {
            for message in outgoing.try_iter() {
                if let HostMessage::ControlAccess { allowed: now } = &message {
                    allowed = *now;
                }
                let quit = matches!(message, HostMessage::Quit);
                connection.send(&message).unwrap();
                if quit {
                    return;
                }
            }
            match connection.try_recv() {
                Ok(Some(AddonMessage::Control { id, command })) => {
                    seen2.lock().unwrap().push(command.clone());
                    let reply = if !allowed {
                        ControlReply::failure(
                            Some(id),
                            ControlError::new(
                                control_codes::NOT_ALLOWED,
                                "The user has not let Backtracking Agent control Backtracking",
                            ),
                        )
                    } else {
                        let mut result = ControlResult::default();
                        let mut selection = None;
                        match &command {
                            ControlCommand::Select { .. } => {
                                selection = Some(SelectionState {
                                    start_frame: 384_000,
                                    end_frame: 480_000,
                                    seconds: 2.0,
                                    from_seconds_ago: 2.0,
                                    to_seconds_ago: 0.0,
                                })
                            }
                            ControlCommand::Save { folder: wanted } => {
                                let folder = wanted.clone().map_or(folder.clone(), PathBuf::from);
                                result.folder = Some(folder.display().to_string());
                                result.files = Some(
                                    ["A", "B"]
                                        .iter()
                                        .map(|lane| SavedFile {
                                            path: folder
                                                .join(format!("{lane}.wav"))
                                                .display()
                                                .to_string(),
                                            kind: "wav".into(),
                                            lane: Some((*lane).into()),
                                            channels: Some(2),
                                            sample_rate: Some(48_000),
                                            frames: Some(96_000),
                                        })
                                        .collect(),
                                );
                            }
                            ControlCommand::ListSources => {
                                result.sources = Some(vec![SourceInfo {
                                    id: "addon\tcom.example.test-tone\tmain".into(),
                                    kind: "addon".into(),
                                    name: "Test tone".into(),
                                }])
                            }
                            ControlCommand::AddSource { .. } => {
                                lanes = 3;
                                result.history_restarted = true;
                            }
                            ControlCommand::RemoveSource { .. } => {
                                lanes = 2;
                                result.history_restarted = true;
                            }
                            _ => {}
                        }
                        result.state = state(selection, lanes);
                        ControlReply::success(id, &result)
                    };
                    connection.send(&HostMessage::ControlReply(reply)).unwrap();
                }
                Ok(Some(AddonMessage::Message { level, text })) => {
                    messages2.lock().unwrap().push((level, text))
                }
                Ok(Some(_)) => {}
                Ok(None) => {
                    if connection.is_disconnected() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("the add-on sent something unreadable: {error}"),
            }
        }
    });
    FakeHost {
        seen,
        messages,
        send,
        done,
    }
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_of(result: &CallToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "{}", text(result));
    serde_json::from_str(&text(result)).unwrap()
}

async fn call(
    client: &RunningService<RoleClient, ()>,
    name: &'static str,
    arguments: Value,
) -> CallToolResult {
    let mut params = CallToolRequestParams::new(name);
    if let Value::Object(arguments) = arguments {
        params = params.with_arguments(arguments);
    }
    client.call_tool(params).await.unwrap()
}

async fn http_client(url: &str, token: &str) -> RunningService<RoleClient, ()> {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .unwrap()
}

fn read_endpoint(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_clients_reach_backtracking_through_http_and_stdio() {
    let data = std::env::temp_dir().join(format!("backtracking-agent-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&data);
    fs::create_dir_all(&data).unwrap();
    let endpoint_file = data.join("endpoint.json");

    let mut host = Host::bind("test").unwrap();
    // Port 0: any free port, so that the test does not depend on the default one.
    host.expect_addon(
        ID,
        "agent-test",
        BTreeMap::from([("port".into(), json!(0))]),
    )
    .unwrap();
    let mut child = ChildGuard(
        Command::new(PROGRAM)
            .env("BACKTRACKING_ADDON_PORT", host.port().to_string())
            .env("BACKTRACKING_ADDON_TOKEN", "agent-test")
            .env("BACKTRACKING_ADDON_DATA", &data)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    let mut connection = loop {
        if let Some(event) = host.poll().unwrap().into_iter().next() {
            match event {
                HostEvent::Connected(connection) => break connection,
                HostEvent::Rejected { reason, .. } => panic!("handshake rejected: {reason}"),
            }
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "the add-on exited early"
        );
        assert!(started.elapsed() < Duration::from_secs(10), "no connection");
        thread::sleep(Duration::from_millis(5));
    };
    // The first thing it says: its setting and buttons.
    let describe = loop {
        if let Some(message) = connection.try_recv().unwrap() {
            break message;
        }
        thread::sleep(Duration::from_millis(5));
    };
    let AddonMessage::Describe {
        settings, actions, ..
    } = describe
    else {
        panic!("expected describe first, got {describe:?}");
    };
    assert_eq!(settings[0].key, "port");
    assert!(actions
        .iter()
        .any(|action| action.label == "Show connection details"));

    let folder = data.join("saved");
    let fake = serve(connection, folder.clone());
    fake.send
        .send(HostMessage::ControlAccess { allowed: false })
        .unwrap();
    wait_for("the connection file", || endpoint_file.is_file());
    let endpoint = read_endpoint(&endpoint_file);
    let url = endpoint["url"].as_str().unwrap().to_owned();
    let token = endpoint["token"].as_str().unwrap().to_owned();
    assert!(
        url.starts_with("http://127.0.0.1:") && url.ends_with("/mcp"),
        "{url}"
    );
    assert_eq!(token.len(), 64);
    // The page says the server runs, warns that control is off, and does not show the token.
    wait_for("the warning on the Add-ons page", || {
        fake.messages.lock().unwrap().iter().any(|(level, text)| {
            *level == MessageLevel::Warning && text.contains("Let this add-on control")
        })
    });
    assert!(fake
        .messages
        .lock()
        .unwrap()
        .iter()
        .all(|(_, text)| !text.contains(&token)));

    // Requests without the token, with another token, from a web page or for another host.
    let raw = reqwest::Client::new();
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
    let post = |token: Option<&str>, extra: Option<(&str, &str)>| {
        let mut request = raw
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(initialize);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some((name, value)) = extra {
            request = request.header(name, value);
        }
        request.send()
    };
    assert_eq!(post(None, None).await.unwrap().status(), 401);
    assert_eq!(
        post(Some("0".repeat(64).as_str()), None)
            .await
            .unwrap()
            .status(),
        401
    );
    let port = url
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches("/mcp");
    assert_eq!(
        post(Some(&token), Some(("origin", "https://evil.example")))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        post(
            Some(&token),
            Some(("host", &format!("evil.example:{port}")))
        )
        .await
        .unwrap()
        .status(),
        403
    );
    assert!(post(
        Some(&token),
        Some(("origin", &format!("http://127.0.0.1:{port}")))
    )
    .await
    .unwrap()
    .status()
    .is_success());

    // An rmcp client over HTTP.
    let client = http_client(&url, &token).await;
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 14);
    let refused = call(&client, "get_state", json!({})).await;
    assert_eq!(refused.is_error, Some(true));
    let refused = text(&refused);
    assert!(refused.contains("not_allowed"), "{refused}");
    assert!(refused.contains("Options > Add-ons"), "{refused}");

    fake.send
        .send(HostMessage::ControlAccess { allowed: true })
        .unwrap();
    wait_for("the running message", || {
        fake.messages
            .lock()
            .unwrap()
            .last()
            .is_some_and(|(level, text)| *level == MessageLevel::Info && text.contains("--stdio"))
    });
    let state = json_of(&call(&client, "get_state", json!({})).await);
    assert_eq!(state["state"]["lanes"].as_array().unwrap().len(), 2);
    assert!(state["state"].get("drag_folder").is_none());

    let selected = json_of(&call(&client, "select", json!({"from_seconds_ago": 2})).await);
    assert_eq!(selected["state"]["selection"]["seconds"], 2.0);
    // Arguments that cannot be right are refused here; Backtracking never sees them.
    let before = fake.seen.lock().unwrap().len();
    let bad = call(
        &client,
        "select",
        json!({"from_seconds_ago": 1, "to_seconds_ago": 2}),
    )
    .await;
    assert_eq!(bad.is_error, Some(true));
    assert!(text(&bad).contains("Invalid arguments"));
    assert_eq!(fake.seen.lock().unwrap().len(), before);

    let saved = json_of(
        &call(
            &client,
            "save",
            json!({"folder": folder.display().to_string()}),
        )
        .await,
    );
    assert_eq!(saved["folder"], folder.display().to_string());
    assert_eq!(
        saved["files"][1]["path"],
        folder.join("B.wav").display().to_string()
    );
    assert_eq!(saved["files"][0]["seconds"], 2.0);

    let sources = json_of(&call(&client, "list_sources", json!({})).await);
    let source = sources["sources"][0]["id"].as_str().unwrap().to_owned();
    assert!(source.contains('\t'));
    let added = json_of(&call(&client, "add_source", json!({"source": source})).await);
    assert_eq!(added["history_restarted"], true);
    assert_eq!(added["state"]["lanes"].as_array().unwrap().len(), 3);
    let removed = json_of(&call(&client, "remove_source", json!({"lane": "C"})).await);
    assert_eq!(removed["history_restarted"], true);

    {
        let seen = fake.seen.lock().unwrap();
        assert!(seen.contains(&ControlCommand::select_seconds_ago(2.0, 0.0)) || seen.iter().any(|c| matches!(c, ControlCommand::Select { from_seconds_ago: Some(f), .. } if *f == 2.0)));
        assert!(seen.contains(&ControlCommand::save(Some(folder.display().to_string()))));
        assert!(seen.contains(&ControlCommand::AddSource {
            source: source.clone()
        }));
        assert!(seen.contains(&ControlCommand::RemoveSource { lane: "C".into() }));
    }

    // Through --stdio, as an MCP client starts it.
    let mut forwarder = tokio::process::Command::new(PROGRAM)
        .args(["--stdio", "--endpoint"])
        .arg(&endpoint_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdio = ()
        .serve((
            forwarder.stdout.take().unwrap(),
            forwarder.stdin.take().unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(stdio.list_all_tools().await.unwrap().len(), 14);
    let state = json_of(&call(&stdio, "get_state", json!({})).await);
    assert_eq!(state["state"]["sample_rate"], 48_000);

    // A new token: the HTTP client set up with the old one is refused, the stdio forwarder
    // reads the new one by itself.
    fake.send
        .send(HostMessage::RunAction {
            id: "new-token".into(),
        })
        .unwrap();
    wait_for("the new token", || {
        read_endpoint(&endpoint_file)["token"].as_str() != Some(&token)
    });
    assert_eq!(post(Some(&token), None).await.unwrap().status(), 401);
    let state = json_of(&call(&stdio, "get_state", json!({})).await);
    assert_eq!(state["state"]["lanes"][0]["lane"], "A");
    drop(client);

    // Closing the forwarder's stdin ends it.
    stdio.cancel().await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), forwarder.wait())
        .await
        .expect("the forwarder ends when its stdin closes")
        .unwrap();
    assert!(status.success(), "{status}");

    // Quit: the add-on exits promptly and takes its connection file with it.
    fake.send.send(HostMessage::Quit).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "the add-on exited with {status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the add-on did not exit promptly"
        );
        thread::sleep(Duration::from_millis(5));
    }
    fake.done.join().unwrap();
    assert!(
        !endpoint_file.exists(),
        "the connection file is left behind"
    );
    assert!(
        data.join("token").is_file(),
        "the token is kept for the next start"
    );
    let log = fs::read_to_string(data.join("agent.log")).unwrap();
    assert!(log.contains("asked to quit"), "{log}");
    assert!(!log.contains(&token));
    let _ = fs::remove_dir_all(&data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_mode_says_when_backtracking_is_not_running() {
    let data =
        std::env::temp_dir().join(format!("backtracking-agent-absent-{}", std::process::id()));
    let _ = fs::remove_dir_all(&data);
    fs::create_dir_all(&data).unwrap();
    let endpoint_file = data.join("endpoint.json");
    let mut forwarder = tokio::process::Command::new(PROGRAM)
        .arg("--stdio")
        .env("BACKTRACKING_AGENT_ENDPOINT", &endpoint_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdio = ()
        .serve((
            forwarder.stdout.take().unwrap(),
            forwarder.stdin.take().unwrap(),
        ))
        .await
        .unwrap();
    // It connects and lists its tools all the same.
    assert_eq!(stdio.list_all_tools().await.unwrap().len(), 14);
    let started = Instant::now();
    let result = call(&stdio, "get_state", json!({})).await;
    assert_eq!(result.is_error, Some(true));
    let said = text(&result);
    assert!(said.starts_with("Backtracking is not running"), "{said}");
    assert!(said.contains("Options > Add-ons"), "{said}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "it must not hang"
    );

    // A file left behind by a run that ended is recognised as such.
    fs::write(
        &endpoint_file,
        json!({"url": "http://127.0.0.1:9/mcp", "token": "x", "pid": u32::MAX - 1,
               "program": PROGRAM, "version": "0"})
        .to_string(),
    )
    .unwrap();
    let said = text(&call(&stdio, "save", json!({})).await);
    assert!(said.contains("from a run that has ended"), "{said}");

    stdio.cancel().await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), forwarder.wait())
        .await
        .expect("the forwarder ends when its stdin closes")
        .unwrap();
    assert!(status.success(), "{status}");
    let _ = fs::remove_dir_all(&data);
}
