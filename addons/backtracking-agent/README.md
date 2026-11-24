# Backtracking Agent

Backtracking Agent is a Backtracking add-on that lets an AI agent, or any other
[MCP](https://modelcontextprotocol.io) client, use the Windows desktop app. Once you allow it,
the client can read what Backtracking holds, pause and start recording, select a stretch, save
it as WAV files, and add or remove sources.

It is one program, `backtracking-agent.exe`, with two modes:

- **Add-on mode.** Backtracking starts it like any other add-on. It serves MCP over Streamable
  HTTP on `127.0.0.1` only, and passes each tool call to Backtracking through the add-on
  contract's `control` capability.
- **`--stdio` mode.** What an MCP client starts. It answers the client over standard input and
  output and forwards each tool call to the add-on that Backtracking is running. It finds that
  add-on by itself, so the client needs no port and no token.

The add-on can only do what the **Let this add-on control Backtracking** switch allows. While
the switch is off, every tool call fails with a message that says so.

## Tools

| Tool | What it does |
| --- | --- |
| `get_state` | Whether capture is on, the lanes, how much audio is held, the selection, the view, the save folder. Read-only. |
| `list_sources` | The sources that `add_source` can add. Read-only. |
| `start_capture`, `pause_capture` | Start or pause recording, as the toolbar does. |
| `clear_history` | Throw away all the audio held. Marked destructive. |
| `select` | Select by seconds ago (`from_seconds_ago`, `to_seconds_ago`) or by frame. |
| `clear_selection` | Drop the selection. |
| `choose_rows` | Choose the lanes or channels the selection and save cover. |
| `set_view` | Zoom the window to the newest seconds or to a range. |
| `save` | Write the selection (or everything held) as WAV files, to `folder` or the drag-and-drop folder. Returns each file's path, lane, channels, sample rate and length. |
| `play_preview`, `stop_preview` | Play the selection through the speakers, and stop it. |
| `add_source`, `remove_source` | Add a source as a new lane, or remove a lane. Both start a new, empty history, so they are marked destructive; the result says `history_restarted`. |

Results are compact JSON. When Backtracking refuses a call, the tool result is an error whose
text starts `Backtracking refused (<code>): <message>`, for example `not_allowed` or `busy`.

## Build and pack

You need Rust 1.88 or later and Python 3 (for the licence notices). From the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File addons\backtracking-agent\pack.ps1
```

This builds the release program, writes `THIRD_PARTY_LICENSES.md` with `licenses.py`, and packs
`addons\backtracking-agent\target\backtracking-agent.btaddon` with the repository's
`btaddon-pack`. The package holds `addon.json`, `bin\backtracking-agent.exe`, `LICENSE.txt` and
`THIRD_PARTY_LICENSES.md`. Its id is `com.kamelkyp.backtracking.agent`.

`cargo test --locked` in this folder runs the unit tests and an end-to-end test against a fake
Backtracking.

## Install and allow control

1. Drag `backtracking-agent.btaddon` onto a Backtracking window, or press **Install from file…**
   on the **Add-ons** page of Options, and confirm. The confirmation lists the `control`
   capability.
2. On the **Add-ons** page, switch on **Let this add-on control Backtracking** under
   Backtracking Agent.

The card then says `MCP server running at http://127.0.0.1:47615/mcp` and has these buttons:

- **Copy Claude Code command** copies `claude mcp add backtracking -- "<program>" --stdio`.
- **Copy Claude Desktop entry** copies an `mcpServers` entry that starts `--stdio`.
- **Show connection details** shows the HTTP address and the token for 2 minutes.
- **Copy HTTP command with token** copies a Claude Code command for the HTTP address, with the
  token in an `Authorization` header. Keep it private.
- **New token** makes a new token. HTTP clients given the old one stop working; `--stdio`
  clients pick up the new one by themselves.

**Port on 127.0.0.1** sets the port (47615 by default). If that port is taken, the add-on uses a
free one and says so on the card. `0` picks a free port at each start.

The program is installed at
`%LOCALAPPDATA%\Backtracking\addons\com.kamelkyp.backtracking.agent\bin\backtracking-agent.exe`.

## Connect a client

Backtracking must be running with the add-on enabled whenever the client uses the tools. If it
is not, each call fails at once with `Backtracking is not running, or its Backtracking Agent
add-on is not switched on`.

### Claude Code

Press **Copy Claude Code command** and run what it copied in a terminal:

```powershell
claude mcp add backtracking -- "%LOCALAPPDATA%\Backtracking\addons\com.kamelkyp.backtracking.agent\bin\backtracking-agent.exe" --stdio
```

(The button gives the full path, without `%LOCALAPPDATA%`.) Add `--scope user` to use it in
every project.

### Claude Desktop

Press **Copy Claude Desktop entry**, open **Settings → Developer → Edit Config**, and merge the
entry into `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "backtracking": {
      "command": "C:\\Users\\you\\AppData\\Local\\Backtracking\\addons\\com.kamelkyp.backtracking.agent\\bin\\backtracking-agent.exe",
      "args": ["--stdio"]
    }
  }
}
```

Restart Claude Desktop.

### Another MCP client

- **Over stdio** (simplest): run `backtracking-agent.exe --stdio`. It needs no other settings.
- **Over HTTP**: use the address `http://127.0.0.1:<port>/mcp` with the header
  `Authorization: Bearer <token>` from **Show connection details**. The server answers only to
  `Host` `127.0.0.1:<port>` or `localhost:<port>`, refuses requests that carry an `Origin` from
  anywhere else (so web pages cannot reach it), and answers `401` without the right token.

## How `--stdio` finds Backtracking

While the add-on runs, it writes `endpoint.json` (address, token, process id) to its data folder,
`%LOCALAPPDATA%\Backtracking\addon-data\com.kamelkyp.backtracking.agent\`. Only your Windows
account can read it, and it is removed when the add-on stops. `--stdio` reads it at each tool
call, so it follows Backtracking restarts, port changes and new tokens.

It looks for the file in this order:

1. `--endpoint <file>` on the command line.
2. The `BACKTRACKING_AGENT_ENDPOINT` environment variable.
3. Beside the install, when the program runs from a Backtracking `addons` folder.
4. `%LOCALAPPDATA%\Backtracking\addon-data\com.kamelkyp.backtracking.agent\endpoint.json`.

## The token

The token is 32 random bytes, kept in `token` in the add-on's data folder (readable by your
account only) and reused at each start, so an HTTP client you set up once keeps working after
Backtracking restarts. **New token** replaces it. A new token at every start would not make the
token safer: any program running as you can read the file either way. Clients started with
`--stdio` never need it.

The add-on's own log, `agent.log` in the same folder, lists the commands it passed on. It never
holds the token.

## Limits

- Windows only, like add-ons. English only: the add-on contract does not tell an add-on the
  user's language.
- Backtracking refuses calls while you drag, drop or save, with `busy`.
- A client started with `--stdio` keeps `backtracking-agent.exe` open. Close the client before
  you update or remove the add-on.

## Licence

Apache-2.0, like Backtracking. The package carries the licences of the crates it is built from
in `THIRD_PARTY_LICENSES.md`.
