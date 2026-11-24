# backtracking-addon

This standalone crate implements the add-on manifest and line-based control
protocol. `client` connects an add-on to Backtracking. `host` provides a polled,
nonblocking listener for the desktop app. The optional `install` feature adds
package inspection, installation, removal, and the `btaddon-pack` command.
It has no dependency on the capture core or an async runtime.

Write your first add-on (20 steps):

1. Read [the contract](../../docs/ADDON_CONTRACT.md).
2. Copy [test_tone.rs](examples/test_tone.rs) into your project's examples folder.
3. Add `backtracking-addon` as a dependency with default features.
4. Add `serde_json` if your settings need JSON values.
5. Give your add-on a stable lower-case reverse-domain id.
6. Use the same id in `Client::connect_from_env` and `addon.json`.
7. Let Backtracking provide the port, token, and writable data folder.
8. Read stored overrides from `Client::settings` after connecting.
9. Send `describe` after the welcome handshake.
10. List only the capabilities declared in your manifest.
11. Give every source stable ids and its available formats.
12. Keep the declared channel groups equal to the audio stream.
13. Adapt the example's Sender registration and packet writer.
14. Render only the selected format at real-time speed.
15. Poll `Client::try_recv` regularly while rendering.
16. Handle setting changes, actions, format changes, and `quit`.
17. Exit promptly when `Client::is_disconnected` becomes true.
18. Copy [addon.json](examples/test-tone-package/addon.json) and update its metadata.
19. Build your executable and put it at every path listed under `run`.
20. Run `cargo run --features install --bin btaddon-pack -- <folder> <out.btaddon>`.

The supplied package manifest targets Windows x86-64. For another supported
platform, change its `run` key and copy that platform's executable into the
package folder. Build the example with `cargo build --release --example test_tone`.

The example finds the Sender registration folder the way contract section 5
lists it for each platform. Only the Windows path has been run against the app.

## Appearance example

Declare `"appearance"` in `addon.json` and send a complete `protocol::Appearance`
with `Client::send_appearance`. Its color maps use RGBA bytes. Use
`APPEARANCE_COLOR_NAMES` and `APPEARANCE_NUMBER_RANGES` from `protocol` for the
supported names and values. `Appearance::spectrum` is an optional gradient for
coloring the waveform by frequency: 2 to 8 `SpectrumStop`s, positions from 0 (low)
to 1 (high) in order. `Client::send_appearance` returns an error for a spectrum
the host would drop. Each message replaces the previous offer. Send
`Appearance::default()` to clear it. The client allows up to ten updates per second.
The user chooses whether to apply the offer in Options > Appearance.

Build with `cargo build --locked --release --example theme_example`. Copy the
resulting `theme_example.exe` to `examples/theme-package/bin/theme-example.exe`
and run `btaddon-pack` on `examples/theme-package`. The example offers waveform
colors and a three-stop spectrum, and updates its border color four times per
second until the host tells it to quit. [Example source](examples/theme_example.rs) · [Package manifest](examples/theme-package/addon.json)

## Control example

Declare `"control"` in `addon.json` to drive Backtracking: read its state, pause
and start capture, select, save files and change sources (contract section 4.4).
Nothing works until the user switches control on for your add-on on the Add-ons
page; until then every request is refused with `not_allowed`.
`Client::control_allowed` says what the host last sent in `control_access`.

`Client::control(&command)` sends one request and waits for its reply. The outer
`Result` is a connection problem; the inner one is the host's answer, a
`ControlResult` with the state after the command, or a `ControlError` whose `code`
is one of `protocol::control_codes`. It waits the host's own limit plus five
seconds: ten seconds, or two minutes for `save`, whose reply comes once every file
is written. To have several requests waiting, use `Client::send_control`, which
returns the id it chose, and `Client::wait_control(id, timeout)`. At most
`MAX_CONTROL_IN_FLIGHT` (8) wait at once. Other host messages that arrive while
you wait are kept for `Client::try_recv`, so poll it as usual. `Client` is
synchronous: use it from one thread.

```rust
use backtracking_addon::protocol::ControlCommand;

// In a function returning Result<_, Box<dyn std::error::Error>>.
client.control(&ControlCommand::PauseCapture)??;
client.control(&ControlCommand::select_seconds_ago(2.0, 0.0))??;
let saved = client.control(&ControlCommand::save(Some(r"D:\Clips".into())))??;
for file in saved.files.unwrap_or_default() {
    println!("{} lane {:?}, {:?} frames", file.path, file.lane, file.frames);
}
```

Build with `cargo build --locked --release --example control_example`, copy
`control_example.exe` to `examples/control-package/bin/control-example.exe` and
run `btaddon-pack` on `examples/control-package`. Once allowed, the example reads
the state, pauses, selects the last two seconds, saves them, clears the selection
and starts capture again, writing every request and reply to `transcript.log` in
its data folder. It saves to the folder in its `folder` setting, else to
`BACKTRACKING_CONTROL_EXAMPLE_FOLDER`, else to `saved` in its data folder. Its
"Run again" button repeats the sequence. [Example source](examples/control_example.rs) · [Package manifest](examples/control-package/addon.json)

Contract 1 hosts from before `control` refuse a package that declares it. In this
crate, `Capability`, `AddonMessage` and `HostMessage` gained variants
(`Capability::Control`, `AddonMessage::Control`, `AddonMessage::ControlRefused`,
`HostMessage::ControlReply`, `HostMessage::ControlAccess`), so an exhaustive
`match` on them needs a new arm.
