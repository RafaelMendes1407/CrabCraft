# Rust REPL variable panel

A Rust REPL tab started explicitly in a trusted Cargo project has a native **REPL
Variables** panel above its terminal. **Refresh variables** asks that same evcxr
session for its current names and types using only the public :vars command.
The table can be filtered by either column. It does not evaluate values, invoke
Debug implementations, expand object members or inspect a separate process.
Snapshots belong to their original terminal ID, including when a tab is renamed.

Refresh is available only at an empty direct-readline REPL prompt after output has
settled for 100 ms. An unfinished command remains untouched. Terminal input is
temporarily disabled during capture so keystrokes cannot interleave with the
inspector command. Any unexpected input epoch change discards the reply. Further
user input clears an earlier snapshot; shadowing, moved bindings and :clear are
reflected by the next explicit refresh. Exiting the process clears its snapshot;
closing its tab or project removes the panel state.

A reader-thread journal is enabled only for managed evcxr sessions, with a 128 KiB
limit and monotonic output cursors. The panel reads bytes after its own command
rather than reinterpreting terminal screen rows or old scrollback. It requires
the command echo and a completed next prompt, accepts UTF-8 text and SGR colors,
and rejects unsupported controls, duplicate names and malformed rows. A table
has at most 512 variables, with names up to 256 bytes and types up to 4096 bytes.
Output loss, changed input, invalid replies, process exit and a five-second
timeout produce explicit errors without keeping partial/stale results.
**Cancel variable refresh** discards the pending snapshot; it does not kill a
user's REPL or replay the request. evcxr remains usable through its terminal.
Arbitrary stdout from trusted user programs remains untrusted display content.

Validated against evcxr_repl 0.22.0 (MIT/Apache-2.0) using its installed public
source as the protocol reference: src/bin/evcxr.rs uses the direct-readline
prompt, and evcxr/src/command_context.rs implements :vars as names and types.

The native test opens a real Cargo library, loads it through :dep, creates values
from that library, clicks Refresh and inspects real rendered variable rows.
It verifies an unfinished command is untouched, a shadowed binding changes type,
a moved String disappears under its old name, cancellation never repopulates
rows, :clear empties the table, and closing the session releases its state:

    CRABCRAFT_TEST_EVCXR=/path/to/evcxr cargo test -p rc-app repl_variables::tests::native -- --ignored --nocapture

Unit tests additionally cover bounded output journals, duplicate/malformed
protocol rows, unsupported controls, timeouts, changed input and lost output.
This documents the supported subset rather than claiming parity with another IDE.
