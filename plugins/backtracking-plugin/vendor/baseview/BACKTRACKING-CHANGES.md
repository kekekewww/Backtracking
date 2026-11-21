# baseview 0.4.2 with a Windows paint fix

This is baseview 0.4.2 from crates.io (via egui-baseview 0.9.0 and nice-plug-egui 0.6.0), used
through `[patch.crates-io]` in the plug-in's `Cargo.toml`. Carry this change forward when updating,
or drop the copy once upstream validates the whole update region.

- `src/platform/win/window.rs`, `WM_PAINT`: the window now validates its whole update region
  before drawing (`validate_window`, new in `src/wrappers/win32/window/handle.rs`, replacing
  `validate_rect`), as `BeginPaint` does. What is invalidated while drawing stays for the next
  WM_PAINT. Before, it validated only `GetUpdateRect`'s rectangle, after drawing. That rectangle is
  the region's bounding box clipped to what shows of the window, and when the window reaches past
  its parent, as a plug-in editor does when it grows before, or without, the host growing its own
  window, it can miss the region entirely: Windows reported (0,0,640,212) while `GetUpdateRgn`
  found nothing inside it. The region then never emptied, Windows sent WM_PAINT forever, and
  because WM_PAINT comes before WM_TIMER the redraw timer never fired: the editor stopped drawing
  and taking input while the host went on. Nuendo 14 did this when Options opened (S3-6); a test
  host that never resizes its parent shows it on Options and on a wide capture's growth.

The test is the editor host in `src/vst3_host.rs` (`count_paint`), which fails when WM_PAINT keeps
coming within one pump. It fails with baseview 0.4.2 as published and passes with this copy.
