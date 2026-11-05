# Native editor and navigation verification

`cargo test -p rc-app inherited_ui_audit -- --nocapture` exercises the native egui
application against temporary Cargo workspaces with real Rust source files. Four
headless tests cover the following shipped subset:

- Islands Light and Islands Dark change both actual application visuals and rendered
  Rust keyword colors. Saving Appearance settings persists the choice. Custom color
  schemes and system-theme synchronization are outside this verification.
- Separate editor/tool-window islands, stripe show/hide, moving a tool window between
  sides, replacement by another tool on the same side, and per-workspace layout
  restoration. At the supported 900×560 minimum window size, overflowing stripe
  buttons scroll instead of overlapping; a native wheel event exposes and opens the
  final tool. This is behavioral verification, not a RustRover visual comparison.
- Recent Files filters and scrolls the session's MRU list (at most 50 files). Recent
  Locations scrolls up to 30 navigation entries and restores the original character
  position, including its column after reopening a closed file. Positions are clamped
  if the file changed. Closed-file previews show the path without reading the file
  just to construct a preview. These histories do not persist across application
  sessions. Keyboard filtering resets stale selection; mouse rows remain stationary.
- After explicit trust, rust-analyzer produces a real E0308 error in a Cargo project.
  The painted scrollbar mark shows its diagnostic on hover. Clicking it brings the
  exact source line into view and moves the caret there; a mark inside a folded
  function also unfolds it. Navigation uses the diagnostic's source row rather than
  the scrollbar thumb's padded scroll range, preventing overshoot near end of file.

The fixture includes a build script that writes a sentinel. Theme, layout and recent
navigation tests assert that the sentinel was not created: opening those workspaces
and operating the UI must not execute project code. Only the diagnostic test grants
trust. Tests inspect rendered native shapes, accessibility nodes, actual persisted
settings/session state and real rust-analyzer diagnostics; no fabricated completion
or diagnostic data is used.
