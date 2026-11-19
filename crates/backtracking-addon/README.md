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
