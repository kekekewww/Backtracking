# Backtracking add-on contract, version 1

An add-on is a separate program that Backtracking starts and talks to. Backtracking never loads
add-on code into its own process. This page is everything an add-on author needs: the package, how
the program is started and stopped, the messages, and how audio reaches Backtracking.

An add-on can be written in any language. Rust authors can use the `backtracking-addon` crate
(`crates/backtracking-addon`), which implements the manifest and the messages below, and
`backtracking-core` for the audio stream. `crates/backtracking-addon/examples/test_tone.rs` is a
complete add-on in one file.

Status: contract 1 is implemented by the Windows desktop app. The macOS app does not host add-ons
yet.

## 1. What an add-on can do

An add-on declares **capabilities** in its manifest. Backtracking shows them to the user before
installing and offers nothing else to the add-on.

| Capability | What it allows |
| --- | --- |
| `source` | Provide audio sources that appear in Backtracking's source list, each with a list of formats and one or more groups of labelled channels. |
| `open-files` | Register file types. Backtracking shows an "Open file" button and passes the chosen path to the add-on. |
| `settings` | Declare settings. Backtracking draws them, stores the values and sends changes. |
| `actions` | Declare named buttons. Backtracking shows them and tells the add-on when one is pressed. |
| `clips` | Be told the paths of every clip Backtracking exports. |
| `appearance` | Offer colors and a few drawing values that the user can choose to apply. |

An add-on runs with the user's own permissions and is not sandboxed. The capabilities describe
what Backtracking does for the add-on, not what the program is able to do on the computer. The
install screen says so.

## 2. Package

A package is a ZIP file with the extension `.btaddon`. `addon.json` is at its root.

```json
{
  "contract": 1,
  "id": "com.example.test-tone",
  "name": "Test tone",
  "version": "0.1.0",
  "author": "Example",
  "description": "Plays a sine wave as a Backtracking source.",
  "homepage": "https://example.com/test-tone",
  "license": "MIT",
  "capabilities": ["source", "settings", "actions"],
  "run": {
    "windows-x86_64": { "program": "bin/test-tone.exe", "args": [] }
  }
}
```

| Field | Rule |
| --- | --- |
| `contract` | Required. `1`. A package with a higher number than the app supports is refused. |
| `id` | Required. Lower-case letters, digits, `.` and `-`; starts and ends with a letter or digit; at most 64 characters. Reverse-domain form is recommended. It names the install folder and must stay the same across versions. |
| `name`, `version`, `author`, `description` | Required. Plain text without control characters. `name` and `author` at most 80 characters, `version` at most 32, `description` at most 400. `name` and `version` must not be empty. |
| `homepage`, `license` | Optional. `homepage` must start with `https://`. |
| `capabilities` | Required. A list of the names in section 1. A name this version of Backtracking does not know makes the package invalid, with a message that a newer Backtracking is needed. |
| `open_files` | Required when `open-files` is declared, otherwise absent: `{ "description": "Audio files", "extensions": ["wav"] }`. `extensions` has 1 to 32 entries of 1 to 12 lower-case letters or digits, without the dot. |
| `run` | Required. Keys are `windows-x86_64`, `windows-aarch64`, `macos-aarch64`, `macos-x86_64`, `linux-x86_64`. A package without an entry for the running platform is refused with a message that says so. |
| `run.*.program` | A relative path inside the package (see the path rules below). The file must exist in the package. |
| `run.*.args` | Optional list of strings passed to the program. |

Every length in this document counts Unicode characters (scalar values), not bytes.

Paths, for `program` and for every entry in the package: `/` separators; no `..`, `.` or empty
component; no drive, root, backslash or colon; no name Windows cannot hold (control characters,
`< > " | ? *`, a trailing dot or space, device names such as `CON` or `COM1`); no two entries
that differ only by letter case. An `id` that is a Windows device name is refused for the same
reason.

The ZIP uses UTF-8 entry names, stored or deflate compression, and no encryption. Limits: at most
4096 entries (folders count), at most 512 MiB unpacked, no symbolic links or other special
entries. A package that breaks a rule is refused and nothing is written.

Installing unpacks the package to `<data>/addons/<id>/`, replacing an earlier version of the same
`id`; if installing fails, the earlier version stays. `<data>` is `%LOCALAPPDATA%\Backtracking` on
Windows. Removing an add-on deletes that folder.
The add-on's own files go in `<data>/addon-data/<id>/`, which survives updates and is deleted only
when the user removes the add-on and chooses to delete its data.

## 3. Starting and stopping

Backtracking starts an enabled add-on's program when the app starts, and when the user enables it.
The working directory is the install folder. Three environment variables are set:

| Variable | Value |
| --- | --- |
| `BACKTRACKING_ADDON_PORT` | The TCP port on `127.0.0.1` to connect to. |
| `BACKTRACKING_ADDON_TOKEN` | A random token for this run. Send it in `hello`. It is accepted once. |
| `BACKTRACKING_ADDON_DATA` | The add-on's writable data folder. It exists. |

The program has no console window and its standard streams are not read. It must connect and send
`hello` within 10 seconds, or it is stopped and shown as "Not responding".

To stop an add-on, Backtracking sends `quit`, waits up to 2 seconds for the process to exit, then
ends it. All add-on processes are ended when Backtracking exits, also if Backtracking crashes. An
add-on that loses its connection must exit.

If the program exits by itself, Backtracking restarts it. After 3 exits within 60 seconds it stays
stopped and the Add-ons page shows the reason.

## 4. Messages

The add-on opens one TCP connection to `127.0.0.1:<BACKTRACKING_ADDON_PORT>`. Each message is one
JSON object on one line, UTF-8, ended by `\n`, at most 65 536 bytes including the `\n`. Every
message has a `type`.
Both sides ignore message types and fields they do not know, so later contract versions can add
them.

There are no request numbers. The add-on reports its state with `describe`; when something the host
asked for changes that state, the add-on sends `describe` again. When it cannot do what was asked,
it sends `message` with level `error`.

### 4.1 Handshake

Add-on to host, first line:

```json
{"type":"hello","contract":1,"token":"3f9c…","id":"com.example.test-tone","version":"0.1.0"}
```

Host to add-on:

```json
{"type":"welcome","contract":1,"app":"Backtracking","app_version":"0.3.0","settings":{"level":"-12"}}
```

`settings` holds the stored value of every setting the user has changed; a missing key means the
default. If the token, id or contract number is wrong the host answers
`{"type":"rejected","reason":"…"}` and closes the connection.

### 4.2 Add-on to host

**`describe`** replaces the add-on's lists of sources, settings and actions. Send it after
`welcome` and again whenever any of those lists changes. Omitted lists count as empty.

```json
{"type":"describe",
 "sources":[{
   "id":"main",
   "name":"Test tone",
   "sender":"1a2b-3c4d-0",
   "formats":[{"id":"stereo","label":"Stereo"},{"id":"5.1","label":"5.1"}],
   "format":"5.1",
   "groups":[{"id":"channels","label":"Channels","channels":["L","R","C","LFE","Ls","Rs"]}],
   "transport":{"state":"playing","position":12.5,"duration":60.0}
 }],
 "settings":[
   {"key":"level","label":"Level","kind":"choice","default":"-12",
    "options":[{"value":"-12","label":"-12 dB"},{"value":"-6","label":"-6 dB"}],
    "help":"How loud the tone is."},
   {"key":"mute","label":"Mute","kind":"toggle","default":false},
   {"key":"frequency","label":"Frequency (Hz)","kind":"number","default":440,"min":20,"max":20000,"step":1},
   {"key":"title","label":"Title","kind":"text","default":""}
 ],
 "actions":[{"id":"restart","label":"Restart"}]}
```

Sources (capability `source`):

| Field | Meaning |
| --- | --- |
| `id` | Stable within the add-on. Up to 64 characters of letters, digits, `.`, `-`, `_`. |
| `name` | Shown in the source list. Up to 120 characters. |
| `sender` | The id of the Sender endpoint that carries this source's audio (section 5). |
| `formats` | The formats the user can choose for this source, in display order. 1 to 32 entries. `label` is what the user sees, up to 40 characters. |
| `format` | The `id` of the format now in use. |
| `groups` | What the audio stream contains now: one or more groups of channels, in stream order. The channel counts of all groups add up to the Sender's channel count. Backtracking stacks one waveform per channel of the group on show and lets the user switch groups. Labels are up to 24 characters. A source without `groups` is treated as one group named "Channels" with numbered channels. |
| `transport` | Optional. Present when the source plays something with a position. `state` is `playing`, `paused` or `stopped`. `position` and `duration` are seconds; `duration` may be absent for an endless source. |

Each entry in `groups`:

| Field | Meaning |
| --- | --- |
| `id` | Stable within this source. |
| `label` | The group name shown by the host, up to 24 characters. |
| `channels` | Channel labels in stream order, up to 24 characters each. A group must have at least one channel. |
| `positioned` | Optional boolean, default `false`. With `true`, the add-on sends `positions` for this group's channels. |

**Channel labels and where they are heard.** Backtracking shows a label as it is. It also reads
it as a speaker name, to fold a group down to its L/R rows and to place each channel around the
listener in binaural preview and export. Case and spaces around the name do not matter.

| Speaker | Labels | Placed at |
| --- | --- | --- |
| Front left, right | `FL` `FR`, `L` `R`, `Left` `Right` | 30° left and right (60° when the group also has `FLc` `FRc`) |
| Front centre | `FC`, `C`, `Center`, `Centre` | straight ahead |
| Low frequency | `LFE`, `LFE1`, `LFE2` | no place: both ears, quieter |
| Side surround | `Ls` `Rs`, `Lss` `Rss`, `SL` `SR` | 110° (90° when the group also has a back pair) |
| Back | `Lb` `Rb`, `Lrs` `Rrs`, `Lsr` `Rsr`, `BL` `BR` | 135° |
| Wide | `Lw` `Rw` | 60° |
| Front centre pair | `FLc` `FRc`, `Lc` `Rc` | 30° |
| Back centre | `BC`, `Cs` | behind |
| Top front | `TFL` `TFR`, `Ltf` `Rtf`, `Lfh` `Rfh` | 45° to the side, 45° up |
| Top middle | `Ltm` `Rtm`, `Lts` `Rts`, `TSL` `TSR` | 90° to the side, 60° up |
| Top back | `TBL` `TBR`, `Ltr` `Rtr`, `Ltb` `Rtb`, `Lrh` `Rrh` | 135° to the side, 45° up |

The 22.2 names of ITU-R BS.2051 system H (`SiL`, `TpFL`, `TpC`, `BtFC` and the others) are known
as well. A label Backtracking does not know has no place: the channel is heard equally in both
ears and feeds both L/R rows. Channels of a `positioned` group take their place from `positions`
instead, whatever their labels.

Settings (capability `settings`): `kind` is `toggle` (boolean), `choice` (one of `options`, a
string), `number` (with `min` ≤ `default` ≤ `max` and a `step` above zero, which is how far one
click moves the value) or `text` (up to 200 characters). `key` is 1 to 40 characters of lower-case
letters, digits and `_`. Up to 32 settings. `help` is optional, up to 200 characters.

Backtracking keeps the values. `describe` carries only defaults; the add-on learns the stored
values from `welcome` and every later change from `set_setting`, and does not report them back.

Actions (capability `actions`): up to 16, `label` up to 40 characters.

**`transport`** updates one source's position without resending everything. At most 4 per second.
The host ignores updates for unknown sources and sources whose latest `describe` did not declare
`transport`.

```json
{"type":"transport","source":"main","state":"playing","position":13.0,"duration":60.0}
```

**`positions`** gives the places of a source's positioned channels at one moment of its audio.

```json
{"type":"positions","source":"main","time":8123456789012,
 "channels":[{"channel":12,"x":-0.5,"y":0.8,"z":0.0,"size":0.0,"gain":1.0}]}
```

| Field | Meaning |
| --- | --- |
| `source` | The source id from `describe`. |
| `time` | Nanoseconds on the clock of the Sender packets (section 5): the moment in the audio at which these positions hold. It may lie up to 5 seconds in the future. |
| `channels` | Every positioned channel whose position is known, at most 64 entries. This is the full state, not a list of differences. |
| `channel` | Zero-based index into the Sender stream, counted across groups. Must lie in a group with `positioned: true`. |
| `x`, `y`, `z` | The listener is at 0, 0, 0. `x` runs from -1 (left wall) to 1 (right wall), `y` from -1 (back wall) to 1 (front wall), and `z` from -1 (floor) to 1 (ceiling); 0 is ear level. |
| `size` | Optional, 0 to 1, default 0 (a point). Stored, not used by the renderer yet. |
| `gain` | Optional linear level, default 1. The add-on has **not** already applied it to the audio. For rendering and exported positions, the host clamps gain to 0 through 4 (+12 dB). |

Send one when anything changes, at most 50 per second per source, and at least one per second
while the source plays. The host thins faster updates without stopping the add-on. A channel's
position moves in a straight line from one update to the next. Before its first update it has no
position.

The host ignores messages for unknown sources or more than 5 seconds in the future. It drops
entries outside a positioned group or with any number that is not finite, clamps `x`, `y`, `z`
to -1 through 1, and clamps `size` to 0 through 1. These do not stop the add-on.
JSON has no NaN or Infinity, so a line that contains them is not JSON and closes the
connection like any other. Python's `json.dumps` writes them unless `allow_nan=False`; send
`null` for a value you do not know, and that entry is dropped.

Positions belong to the audio at `time`. Seeking, pausing and format changes need nothing
special: stamp positions with the time that audio is heard. Rust authors can call
`Client::send_positions(source, time, channels)` with `ChannelPosition` entries. The example's
`objects` format shows a stereo group followed by two positioned objects, so their stream
indices are 2 and 3.

**`appearance`** offers a set of drawing values with the `appearance` capability.
Backtracking draws the app. An appearance can change only the drawing values listed here.
Backtracking controls capture, selection, export, files, shortcuts, layout, text content and
status indicators.

Appearance is off until the user chooses an add-on in Options > Appearance. The user can choose
at most one add-on's appearance. Backtracking uses its own look when the user turns the appearance
off or the chosen add-on has no offer.

```json
{"type":"appearance",
 "colors":{"waveform":"#67e7b5","border":"#ffffff80"},
 "numbers":{"border_width":2},
 "rows":{"LFE":"#ff8800","Ltf":"#88ccff"}}
```

Each message replaces the add-on's entire offer. Every map is optional and missing entries use the
app's own look. Send `{"type":"appearance"}` or empty maps to clear the offer. Backtracking
keeps the offer in memory while the add-on is running and clears it when the add-on stops.

`colors` values are `#rrggbb` (opaque) or `#rrggbbaa` (with alpha). Hex digits may be
upper- or lower-case.

| Color name | What it colors |
| --- | --- |
| `waveform` | The waveform. |
| `waveform_selected` | The selected part of the waveform. |
| `plot_background` | The background behind the waveform. |
| `center_line` | The center line of a waveform row. |
| `border` | The border around the plot. |
| `lane_divider` | The divider between lanes. |
| `bar_line` | The line marking a musical bar. |
| `second_line` | The line marking a second. |
| `row_label` | The channel label text. |
| `flag` | The flag in the plot. |
| `playhead` | The playhead line. |
| `panel_background` | The panel background. |
| `chip_background` | The background of a chip. |
| `chip_text` | The text of a chip. |
| `chip_current` | The current chip's highlight. |
| `scroll_thumb` | The scrollbar thumb. |
| `preview_highlight` | The highlight under the rows a drag-out would take. |

Backtracking clamps `numbers` to these inclusive ranges and drops values that are not finite.

| Number name | Range | Meaning |
| --- | --- | --- |
| `border_width` | 0 to 8 | Width of the plot border. |
| `corner_radius` | 0 to 16 | Radius of rounded corners. |
| `waveform_stroke_scale` | 0.5 to 3 | Multiplier for the waveform stroke width. |
| `waveform_fill_opacity` | 0 to 1 | Opacity of the waveform fill. |

`panel_background` and `corner_radius` apply only in the full window. `border` and `border_width`
apply only when the user shows a border. The user's own border color takes precedence over the
look's. While the look is on, a `border_width` it supplies replaces the user's width when
drawing the border. The window keeps its size. Backtracking uses the user's transparency
setting for `plot_background` and `panel_background` and ignores the alpha in those two colors.

`rows` gives waveform colors by channel label. Use the source's channel labels, each 1 to 24
characters. Row colors have the same format as `colors`. Backtracking keeps up to 64 valid
entries and drops the rest.

Backtracking checks legibility using WCAG 2 contrast, measured after drawing a color over what
is behind it. Where the rules below call for contrast, graphics need at least 2.0 and text needs
at least 3.0. Where alpha is checked, the minimum is 102 of 255. Two drawn colors are apart if
at least one red, green or blue channel differs by 80 or more. The distance from a status mark
is the sum of the absolute red, green and blue differences and must be at least 90.

Backtracking checks plot colors on both the plot background and the panel background. When the
look leaves out `panel_background`, Backtracking uses the window's own dark panel. In the full
window the plot sits on the panel when the user's transparency is above 0.

- `plot_background` must stay at least 90 away from every capture status mark.
- `panel_background` must stay at least 90 away from every status light and give the window's
  text a contrast of at least 3.0.
- `waveform`, `flag` and `playhead` need alpha of at least 102 and contrast of at least 2.0
  on both backgrounds.
- `waveform_selected` has the same alpha and contrast requirements and must be apart from
  the waveform on both backgrounds.
- Each row color has the same alpha and contrast requirements and must be apart from the
  selection on both backgrounds.
- `row_label` needs contrast of at least 3.0 on both backgrounds. `chip_text` needs contrast
  of at least 3.0 on `chip_background` drawn over each of those backgrounds.
- White text needs contrast of at least 3.0 on `chip_background` and on `chip_current`
  drawn over `chip_background`, on both backgrounds. The current chip must also be apart
  from `chip_background` on both backgrounds.
- `preview_highlight` needs alpha of at least 102 and must be apart from the waveform on
  both backgrounds.
- `lane_divider` and `scroll_thumb` need alpha of at least 102 and contrast of at least 2.0
  on both backgrounds. The thumb is checked in both its normal and active states.
- Backtracking skips legibility checks on `center_line`, `bar_line`, `second_line` and `border`.

When a pair fails, Backtracking ignores the value the look set for what is drawn on top. If the
look set only what is behind it, Backtracking ignores that value. It falls back to its own value,
checks again and keeps the rest of the look.

Backtracking drops unknown color and number names and individual invalid entries. A message with
a map supplied in the wrong form is ignored. Backtracking applies at most 10 appearance updates
per add-on in any rolling second. If more arrive, it keeps the newest waiting update and applies
it as soon as the rate limit allows. Backtracking ignores messages from add-ons without the
capability. Rust add-ons can use `Client::send_appearance(&appearance)`, which returns an error
if called faster than the limit.

**`message`** shows one line next to the add-on on the Add-ons page, replacing the previous one.
`level` is `info`, `warning` or `error`; `text` is up to 300 characters. An empty `text` clears it.
The host replaces control characters in `text`, including newlines and tabs, with spaces before
showing it. They do not stop the add-on.

```json
{"type":"message","level":"error","text":"This file has no object audio."}
```

### 4.3 Host to add-on

| Message | Sent when | Fields |
| --- | --- | --- |
| `set_setting` | The user changes a setting. | `key`, `value` |
| `run_action` | The user presses an action button. | `id` |
| `open_file` | The user picks a file of a registered type (`open-files`). | `path` (absolute) |
| `select_format` | The user picks a format for a source. | `source`, `format` |
| `transport` | The user presses play, pause or stop, or moves the position. | `source`, `command` (`play`, `pause`, `stop`, `seek`), `position` (seconds, with `seek`) |
| `clip_exported` | Backtracking has written a clip (`clips`). | `files` (absolute paths), `sample_rate`, `frames` |
| `quit` | The add-on should exit. | none |

Backtracking remembers the format the user picked for each source. After an add-on's first
`describe` it sends `select_format` again when the remembered format is offered and differs from
the one in use, so an add-on does not have to store that choice itself.

## 5. Audio: the Sender stream

A source's audio travels on Backtracking's Sender protocol, the one the Backtracking plug-in uses.
Rust add-ons use `backtracking_core::SenderEndpoint`. The wire format is small enough to implement
directly:

**Registration.** The Sender picks an id (up to 72 characters of hex digits and `-`), listens on a
TCP port on `127.0.0.1`, and writes the file `<senders>/<id>.sender` containing five
tab-separated fields: port, `1` if audio is available or `0`, sample rate, channel count, name.
It rewrites the file at least every 2 seconds; a file older than 8 seconds is treated as gone.
Write to a temporary name and rename. Delete the file on exit.

`<senders>` is `%LOCALAPPDATA%\Backtracking\senders-v1` on Windows,
`~/Library/Application Support/Backtracking/senders-v1` on macOS, and
`$XDG_RUNTIME_DIR/Backtracking/senders-v1` on Linux (`~/.local/state/Backtracking/senders-v1`
when that variable is not set).

**Connection.** Backtracking connects and sends the byte `A` followed directly by the id, with
nothing after it, to receive audio. Close a connection that sends anything else. (The byte `S`
followed by the id is a request Backtracking makes to its own plug-in; an add-on closes it too.)

**Packets.** After `A` the Sender writes packets, all little-endian:

| Bytes | Field |
| --- | --- |
| 4 | `BTA1` |
| 8 | Time of the packet's first frame in nanoseconds on the monotonic clock (`QueryPerformanceCounter` on Windows, `mach_absolute_time` on macOS, `CLOCK_MONOTONIC` on Linux). |
| 4 | Sample rate. Must equal the registered rate. |
| 4 | Frame count, 1 to 8192. |
| 4 | Bar marker count. |
| 4 × markers | Frame offsets of bar starts inside this packet, ascending. |
| 4 × frames × channels | 32-bit float samples, interleaved. |

**Channels.** A Sender carries 1 to 64 channels at 8 000 to 384 000 Hz. A packet holds at most
65 536 samples, so at most `65536 / channels` frames when that is below 8192. Changing the channel
count or sample rate re-registers the Sender; Backtracking reconnects and starts a new capture,
which clears the audio it was holding in both lanes because they share one history layout.
When a source disappears, both lanes keep their retained audio and the affected lane reports that
the source is gone. Capture resumes when it returns with the same sample rate and channel count.
Changing only groups or channel labels updates the display without clearing audio.
Backtracking 0.2.0 and earlier list only 1- and 2-channel Senders.

**Timing.** Publish audio at the pace it should be heard, stamped with the time it is heard.
Backtracking aligns sources by these timestamps. Read the clock when a stretch of audio starts
(the first packet, and the first after a gap or a format change) and count frames from there:
each later packet's time is the previous packet's time plus its length. The example does it
this way. A clock read for every packet moves with how late the add-on's thread woke up, and
two things go wrong. When another source is captured beside the add-on, Backtracking takes a
packet whose time is more than about two frames away from where the previous packet ended as
the start of a new stretch, and fades the audio out and in for 2 ms around it: with a clock
read per packet that happens at every packet. And `positions` stamped with such times move
with them.

## 6. Being a good guest

- Produce only what is asked for. Render the selected format, not all of them, and offer costly
  extras (for example separate objects) as a setting that is off by default.
- Keep `describe` truthful: the groups must match the stream at the moment it is sent. Re-register
  the Sender first, then send `describe`.
- Send `positions` only when something changed, apart from the once-per-second update while
  playing. Each message still contains the full known state.
- Give audio threads priority over everything else you do, and back off background work under load.
- Exit promptly on `quit` and when the connection closes.

## 7. Versions

`contract` is a whole number. Additions that old add-ons and old hosts can ignore (new message
types, new optional fields) do not change it. A new capability does not change it either: an
add-on that declares one is refused by a Backtracking that does not know it, and installs on one
that does. A change that would break an existing add-on raises the number, and the host keeps
accepting the previous number for at least one release.
