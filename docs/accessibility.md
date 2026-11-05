# Accessibility progress

The desktop build enables eframe's native AccessKit bridge. The custom source editor
now exposes a named multiline text-input node, Unicode text runs, selection/caret ranges,
focus actions and read-only state. Assistive technology can select text using AccessKit;
ordinary typing/copying/navigation follows the editor's existing editing rules.

Text layout is cached by document version, context and font metrics, and is skipped while
AccessKit is inactive. Accessible source is bounded to 16,384 characters and 256 lines
around the caret; a description identifies this limit. Malformed selection node IDs and
character offsets are ignored before converting them to editor positions. Selection
requests refer to the current accessible layout, as with ordinary native text widgets.
This action does not run or save project code.

`cargo test -p rc-app accessibility_audit` reads the actual AccessKit tree for a real Rust
project, selects a literal after a Unicode identifier using a text-selection action,
edits it, verifies read-only protection and malformed-offset rejection, checks bounded
large-file context, and compiles/runs the resulting Cargo program. The native bridge also
compiles on Linux with its AT-SPI dependencies.

G-08 remains partial. No end-to-end Orca/NVDA/VoiceOver session has been validated. The
terminal, custom hierarchy/tree controls, announcements, folding and viewport geometry
need an accessibility audit; this is not a screen-reader compatibility claim. Large
source documents currently expose contextual text rather than their entire contents.
