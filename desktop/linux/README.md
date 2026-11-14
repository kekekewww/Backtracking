# Linux snapshot shortcut

Build and install the shortcut helper from the repository root. The build uses a C compiler, `pkg-config`, X11 development headers and GLib/GIO development headers. On Debian or Ubuntu, install `build-essential pkg-config libx11-dev libglib2.0-dev`.

```sh
cargo install --locked --path desktop/linux
install -Dm644 desktop/linux/tw.kamelyang.backtracking.shortcut.desktop \
  "${XDG_DATA_HOME:-$HOME/.local/share}/applications/tw.kamelyang.backtracking.shortcut.desktop"
```

Start `backtracking-shortcut` in your desktop session and leave it running in the background. Add the same command to your desktop's startup applications to start it on login.

- On X11, press Super and the physical key to the left of 1. This key is labeled `~` on US keyboards. An occupied shortcut is reported when the helper starts.
- On Wayland, approve the global shortcut in your desktop's portal prompt. The helper suggests Super+grave. The desktop controls the final binding, so choose the physical key to the left of 1 if your layout displays another symbol. If the desktop has no Global Shortcuts portal, create a compositor shortcut for that combination and use `backtracking-shortcut --save` as its command. Configure it to activate once per press.

With one active Backtracking plug-in, the shortcut routes to that instance automatically. If you use several instances, run `backtracking-shortcut --list` and launch the helper with `backtracking-shortcut --target INSTANCE_ID`. For a compositor shortcut, use `backtracking-shortcut --target INSTANCE_ID --save`. Each activation sends one snapshot request to the chosen instance.
