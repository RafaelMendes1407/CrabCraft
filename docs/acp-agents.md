# ACP agents

The native **ACP Agents** tool window is an ACP v1 stdio client. Opening a project or
restoring a tool-window layout starts no agent. Enter an executable and arguments, one
argument per line, trust the workspace, then choose **Start ACP Agent**. The child receives
the canonical project working directory, but no environment from project session settings.
Arguments are passed separately, without shell expansion. Executable/argument configuration,
conversation text, authentication metadata and permissions are memory-only.

The selected external process has the user's OS permissions and can use network or execute
its own tools. This is not a sandbox. Use only agents you trust. The native permission UI
controls requests the agent actually sends; it cannot enforce permission checks over an
arbitrary external program. No commercial agent, credentials or model service is bundled.

The client negotiates protocol version 1, initializes the agent and creates a new session
with no MCP servers. Agent-managed `authenticate` methods are explicit buttons. Terminal
agent authentication is declared unsupported and visibly disabled. Agents already logged in
through their own CLI can create sessions directly. Text prompts are explicit; plain text
response chunks, tool/plan updates and bounded stderr logs appear natively. Agent-authored
content is data, never markup or executable code. Hidden thought streams are not displayed.

The client advertises filesystem reads/writes and terminal delegation as **false**. Requests
for those methods receive JSON-RPC method-not-found errors; client code does not read or
write their supplied paths. Attachments, agent registries, session replay/resume, mode/model
switching, logout, remote ACP transports and v2 draft support are outside this implementation.
**New Agent Session** creates a fresh conversation; **Clear ACP Display** only clears the
visible buffers, not the agent's retained context.

`session/request_permission` opens the tool window and shows the complete bounded tool-call
JSON and agent-supplied labels. Buttons use canonical **Allow this call once** and
**Reject this call once**, validated against the offered option's ID and kind. There is no
automatic approval. Persistent allow/reject options are disabled as unsupported. Malformed,
duplicate, oversized or unrelated permission requests are refused; they cannot authorize a
client-side tool. Pending requests stay pending until a choice or cancellation.

**Cancel ACP Prompt** replies `cancelled` to pending permissions and sends `session/cancel`.
The client accepts final updates until the prompt returns its stop reason. If the agent
ignores cancellation for two seconds, its process tree is stopped. **Stop ACP Agent**,
project close, trust revocation and dropped handles disconnect and kill descendants; closing
the workspace cannot leave an old permission attached to a new workspace. An active agent
can continue while the panel is hidden, but permission requests bring the pane forward.

The UTF-8 newline JSON-RPC decoder preserves partial frames across cancellation of a read
future. Bounds include a 1 MiB frame, 16 MiB traffic/10,000 frames per connection, 16 queued
commands, eight permissions/eight options each, 64 KiB tool descriptions and prompts, 1 MiB
conversation display and bounded logs. Initialization/session creation have a 30-second
limit, agent authentication 120 seconds and prompt turns five minutes. All process I/O is
asynchronous; stalled writes are bounded and cancellation kills the process group.

Linux evidence uses a real ACP subprocess fixture and Rust project. Native controls prove
untrusted/open startup makes no process, explicit start negotiates v1 with disabled file/
terminal capabilities, rejecting a permission executes nothing, and allowing one reviewed
`cargo test --offline` call runs real Rust tests/build.rs. A later cancelled permission does
not run another command. Protocol tests exercise agent authentication, refused delegated
file reads without leaking file content, persistent-permission refusal, malformed/oversized
frames, cancellation-safe framing, version mismatch, cancellation acknowledgment and the
process-stop fallback when an agent ignores cancellation. No authenticated commercial agent
or remote model is used; cross-platform UI and screenshots have not been verified.

References: ACP v1 [transport](https://agentclientprotocol.com/protocol/v1/transports),
[initialization](https://agentclientprotocol.com/protocol/v1/initialization),
[session setup](https://agentclientprotocol.com/protocol/v1/session-setup),
[authentication](https://agentclientprotocol.com/protocol/v1/authentication),
[tool permissions](https://agentclientprotocol.com/protocol/v1/tool-calls) and
[prompt cancellation](https://agentclientprotocol.com/protocol/v1/prompt-turn).
This implements the documented core protocol without claiming RustRover parity.
