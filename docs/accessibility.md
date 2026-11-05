# Linux accessibility

The native eframe build enables AccessKit and its Linux AT-SPI bridge. This scope covers
the source editor, the Project files Explorer tree, the terminal's visible output and
status/error announcements. It is not a claim about every assistive technology, tool window,
desktop environment or operating system. NVDA/Windows and VoiceOver/macOS are not exercised.

The code editor exposes a named multiline input, Unicode text runs, selection/caret ranges,
focus actions and read-only state. Accessible source is bounded to 16,384 characters and
256 lines around the caret, with a description identifying that limit. Text layout is cached
and skipped while AccessKit is inactive. Run identity includes the buffer version and context
window, so requests from an earlier edit or large-file window cannot select different source.
Malformed node IDs/offsets are refused before conversion to editor positions.

Text-run geometry uses the same line layout as painting: tabs, inlays, horizontal scrolling
and visual fold rows are reflected in character bounds. Logical text inside a fold remains
readable, but has no fabricated painted rectangle; selecting it expands the containing fold
and scrolls to the caret. Truncated boundary lines in a large-file window also have no guessed
rectangle. The normal Collapse/Expand commands remain available through the menus/shortcuts.
Accessible selection never saves or executes a file, and read-only documents refuse editing.

The Explorer exposes a named tree, stable path-based item identities, levels, selected and
expanded states, actual nested parent relationships, focus/activation and idempotent expand/collapse actions. Keyboard navigation
still uses the existing tree logic. Activation opens files through the normal asynchronous
file-opening path; it does not trust or execute the project. Virtualized items outside the
viewport are not all materialized into the accessibility tree at once; up to 64 ancestor
levels are retained for the visible rows. The tree describes that bound.

The terminal exposes the Terminal role, Unicode text runs with cell-based geometry (including
wide/combining characters), cursor position and polite output updates. Its read-only text
interface has no SetValue action and never turns accessibility text into PTY commands. Normal
keyboard input retains the existing terminal controls and trust rules. Output remains readable
while PTY input is temporarily blocked. Accessible output is bounded to 16,384 characters,
256 visible rows and 4,096 columns per row; it does not expose the entire scrollback. Status
messages use polite live regions and errors use assertive alerts, with accessible message
text bounded to 2,048 characters.

## Validation

`cargo test -p rc-app accessibility_audit --offline` exercises the actual native widgets in
real Rust projects. It selects/edits after a Unicode identifier, compiles/runs the result,
checks read-only/malformed/stale-window guards and bounded large-file context, verifies tab
and folded-row geometry, expands a hidden selection, drives tree accessibility actions and
opens source without running build.rs, traverses 80 source items with keyboard reveal and checks
real ancestor relationships across virtualization. Terminal tests display actual compiled Rust output,
check Unicode/read-only text semantics and verify live status/error regions. These editing
and activation actions are **headless egui_kittest events**, never input sent to a real window.

The real-reader probe is `scripts/accessibility/orca_session.py`. It uses an installed Orca,
AT-SPI, Metacity and speech-dispatcher, plus an explicitly supplied Xvfb/xauth directory. It
creates private X11 authorization/display, D-Bus session, XDG settings/cache/runtime and IDE
state under a new 0700 `/tmp` directory. It neither changes nor reuses HOME, the user's display,
bus, reader preferences or audio session. Speech uses a private dummy backend with an ALSA null
device; the evidence is Orca's generated speech log, not a claim about physical audio hardware.
No third-party binaries/assets are copied into this repository.

After `cargo build -p rc-app --offline`, run from an environment permitted to start local X11
and D-Bus services:

```sh
/usr/bin/python3 scripts/accessibility/orca_session.py \
  --binary target/debug/crabcraft \
  --x-tools /tmp/crabcraft-accessibility/root/usr/bin \
  --duration 20
```

The probe initializes a real Rust file/caret through the CLI, then **only reads** AT-SPI tree,
text/caret/bounds and focus/text events. It sends no keys, clicks, accessibility actions or
other input. It never uses Orca's `--replace`/`--setup` options, which could affect another
reader belonging to the same UID. Cleanup targets only processes created by the probe. The
untrusted fixture's build.rs marker must remain absent. Successful evidence requires the
actual Rust text and positive character bounds in AT-SPI, nested Explorer tree items and an Orca `SPEECH OUTPUT` utterance containing
`accessible_value`, rather than only the app title.

The validated environment used Orca 46.1, AT-SPI 2.52 and Xvfb 21.1.12. The private run
recorded six source snapshots, positive character bounds `[584, 158, 8, 21]`, an
actual second-level `src` tree item, and the reader utterance
`Source code: main.rs entry     let accessible_value = 42;`, with no build.rs marker.
The reproducible probe writes `atspi.json`, `speech-output.log` and raw reader/service logs
to its printed private directory. An initial AT-SPI cache `GetItems` warning was present;
per-node text, tree and focus observation and actual source reading still succeeded. This
is evidence for the documented X11 scope, not a claim about Wayland or all Linux environments.

Primary references: [AccessKit](https://accesskit.dev/),
[GNOME AT-SPI interfaces](https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/), and
[Orca documentation](https://help.gnome.org/orca/).
