# Local process plugins

The **Local Plugins** native toolwindow installs user-selected executable packages implementing CrabCraft plugin protocol version 1. It is independent of External Tools and the ACP client. It supports commands that propose a complete replacement of the active, unsaved document. This is an original, deliberately small extension protocol; it does not claim compatibility with RustRover or any other plugin ecosystem.

## Trust and lifecycle

Opening a project or restoring a toolwindow does not inspect packages or start plugins. Inspect reads a selected manifest and executable on a background worker. Install requires an explicit executable-trust checkbox and copies the verified bytes into CrabCraft's application-data `plugins-v1` directory. New installations are disabled. Refresh, Enable and Remove are explicit local operations; enablement survives restart, but execution never happens automatically. Installation does not compile or run a package.

Run requires both an enabled, explicitly trusted plugin and a trusted project. The UI identifies the active path and explains that its complete unsaved text is sent to the plugin. It checks the installed executable hash again and copies verified bytes into a private temporary directory before execution. Arguments are not parsed through a shell. The process receives a JSON request on stdin and writes one JSON response to stdout. Its working directory is the temporary directory; project Runner environment overrides are not supplied. Processes still inherit the IDE's ordinary OS environment.

**This is not an operating-system sandbox.** An installed binary runs with the user's OS permissions. It can independently read files, use credentials, access the network and start programs. The protocol's narrow context and edit contract constrain what the IDE supplies and applies; they do not constrain a malicious executable. Install only binaries whose source and publisher you trust. A SHA-256 match proves consistency with the selected manifest, not publisher identity or authenticity. No remote marketplace or signature trust service is provided.

Inspect, installation copies and catalog refresh check cancellation between bounded file blocks/entries; cancelling installation before the final rename discards its temporary package. Enable atomically replaces metadata; Remove explicitly deletes only the validated package files. Run is cancellable and has a 30-second deadline. Cancellation, closing the workspace, losing project trust or dropping the model stops the process tree. Input is limited to 1 MiB of document text, stdout to 2 MiB, stderr to 64 KiB, the executable to 32 MiB and manifests to 64 KiB. File operations are bounded and run off the UI thread. The catalog accepts at most 128 packages, with at most 16 commands each. Manifest and executable must be regular files, with no symlinked or noncanonical parent path, traversal or multi-component executable name. Installed metadata is validated when used. Native executable dependencies/resources alongside the binary are not bundled; use a standalone executable.

## Manifest and wire protocol

Select an absolute path to a UTF-8 JSON manifest. Its executable must be a single filename in the same directory. Compute the SHA-256 from the final compiled binary before distributing this pair of files:

```json
{
  "protocol": 1,
  "id": "example-transform",
  "name": "Example transformation",
  "version": "1.0.0",
  "executable": "example-transform",
  "sha256": "<64 hexadecimal digits>",
  "commands": [{"id": "transform", "title": "Transform active document"}]
}
```

IDs contain only ASCII letters, digits, `_` and `-`, up to 64 bytes. Unknown manifest fields, duplicate command IDs, unsupported versions and the reserved executable filename `installed.json` are rejected. On Windows select a native `.exe` binary. Updating requires removing the previous installation first; installation does not overwrite another package.

The plugin reads stdin until EOF. Example request:

```json
{"protocol":1,"command":"transform","document":{"path":"/project/src/lib.rs","text":"pub fn answer() -> u32 { 1 }\n","sha256":"<hash of this exact UTF-8 text>"}}
```

It must exit successfully and return exactly one JSON value with these fields:

```json
{"protocol":1,"base_sha256":"<the request's document hash>","replacement":"pub fn answer() -> u32 { 2 }\n","message":"Optional explanation, limited to 4096 bytes"}
```

`message` may be absent or null. Unknown response fields, mismatched hashes, invalid JSON and unsuccessful exits produce typed errors and no edit. The document path is context, not a command for the IDE to write a file. There are no host filesystem, terminal, network or arbitrary edit RPCs.

The IDE shows the original text, proposed replacement and any plugin message. Applying requires a separate review checkbox. It rechecks the active document path and content hash immediately before replacing the buffer. A changed document is rejected; rerun against the new snapshot. Apply changes the unsaved editor buffer in one undoable operation; it never saves the file. Discard leaves the editor unchanged. Marketplace discovery, dynamic library loading, automatic activation and broader host APIs are visibly unsupported.

## Evidence

`cargo test -p rc-app plugins::tests -- --nocapture` passed four tests using a compiled Rust plugin and real Cargo project. They prove no execution at inspect/install/disabled/untrusted stages, explicit native Run and review flow, stale-document rejection, unsaved buffer replacement and Undo, and a real offline `cargo test` after applying the proposed Rust fix. Separate tests exercise executable tampering, traversal/reserved-name/symlink rejection cancellation before package inspection/copy/installation without creating files, and cancellation of a running Rust plugin without producing an edit. The fixture's build script remains unexecuted before the test explicitly trusts and builds the project. No third-party marketplace or plugin binary was executed.
