# Import preferences from VS Code or RustRover

Settings → Backup and Sync → Import from another editor reads a selected preferences file,
shows recognized values and unsupported keys, and applies only the reviewed values to the
Settings draft. OK saves the draft; Cancel discards it. Reading runs on a cancellable worker.
Closing Settings or changing the source editor/path discards pending results and previews.

VS Code input is `settings.json` (JSON with comments and trailing commas). `[rust]` overrides
take precedence. Supported keys cover font size/line spacing, built-in light/dark theme,
zoom, rulers, inlay hints, indentation guides, sticky lines, format on save and focus-based
autosave. The VS Code reference keymap is selected. Custom theme names, timer-based autosave
and other keys appear as unsupported and retain existing CrabCraft preferences. Inlay-hint
modifier modes map to enabled hints because CrabCraft has a boolean setting.

RustRover input is an individual options XML file, such as `options/editor-font.xml`,
`options/editor.xml` or `options/colors.scheme.xml`, or a color-scheme XML. Recognized values
include DefaultFont FONT_SIZE/LINE_SPACING, EditorSettings RIGHT_MARGIN/IS_INDENT_GUIDES_SHOWN,
GeneralSettings saveOnFrameDeactivation, built-in Default/Light/Darcula/Dark color schemes,
and EDITOR_FONT_SIZE/EDITOR_LINE_SPACING in standalone scheme files. The RustRover reference
keymap family is selected for the host. Import files separately to combine these preferences.
This does not inspect the installed product or assert shortcut/theme parity.

Commands, tool paths, plugins/extensions, credentials, histories, workspace paths and project
trust decisions are excluded. Custom keybindings, arbitrary color palettes, font-family
installation, extension equivalents, ZIP exports and code-style migration are unsupported.
Source files are never modified and no external editor or project program is launched.

Input must be a regular UTF-8 file of at most 1 MiB. Reads check cancellation between chunks
and reject special files, including a FIFO without a writer on Unix. XML declarations do
not enable DTD/entity resolution; DTDs and general entity references are rejected. Depth,
node count, attributes and unsupported-key previews are bounded. Invalid supported values
reject the import rather than silently changing their meaning.

`cargo test -p rc-core editor_import::tests` verifies JSONC/string handling, Rust overrides,
XML validation, bounds, cancellation and the allowlist. `cargo test -p rc-app
settings_migration::tests` drives both source formats through the native Settings UI in real
Cargo projects. It verifies preview-before-apply, OK-before-persistence, preserved trust/tool
settings, unchanged Rust sources and no build script execution; explicitly running each
Cargo program afterwards succeeds.
