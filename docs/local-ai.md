# Local Rust assistance

Open **More actions → Local AI assistant**, enable it for this session, set the
loopback Ollama address, click **Refresh installed local models**, and select an
installed local GGUF model. The project must be trusted before sending a prompt
or source. Opening a project, enabling this panel and editing files do not send
requests. Install and start Ollama and download an appropriate model separately;
CrabCraft never starts a server, installs a model, or invokes model tools.

The panel provides three explicit actions:

- **Send local chat** sends your message and the visible session history. Including
  the current Rust file is optional and disabled initially. Replies are plain text;
  commands, links, images and tool calls are never executed or fetched.
- **Suggest rest of Rust line** sends the current Rust file and asks for one line
  at the caret. The caret must be at the end of a line. The result appears as muted
  inline text. Focus the editor and press Tab to accept or Escape to reject, or
  inspect the proposal in the panel and apply it after checking the review box.
  Requests are on demand; there is no background typing telemetry or automatic
  trigger. Suggestions are at most 256 characters and cannot contain newlines.
- **Propose next Rust edit** sends the current Rust file and your instruction. The
  response must identify one exact, unique source substring and its replacement.
  Review the before/after text, check **I reviewed this AI edit**, and apply it.
  Ambiguous, empty-source, invalid or oversized edits are rejected. This is one
  explicit buffer edit, not an autonomous agent or repository-wide patch.

Applying a suggestion is one undo step. It neither saves nor builds the file.
Changing the source, switching files or closing the panel cancels a pending edit
request and discards its proposal. Moving the caret also invalidates line
suggestions. Chat is detached from the live buffer once sent. Cancel aborts the
HTTP future; it does not terminate a separately managed Ollama service. Closing
the workspace resets all assistance state. Configuration, history and model
responses are kept only in memory.

The endpoint must be HTTP or HTTPS on a literal loopback address, or localhost
(normalized to 127.0.0.1). Credentials, query strings, fragments and URL paths are
rejected. Requests bypass system proxies and cannot follow redirects. Known
cloud tags and remote model metadata are rejected before prompting. An installed
local GGUF model is required. The local service itself remains a program you
choose to trust: use Ollama with `OLLAMA_NO_CLOUD=1` to disable its cloud features.
CrabCraft cannot control the behavior of a different or modified local server.

Only normal, writable Rust documents up to 32 KiB are supported as context for
line and edit suggestions. Generated documents, scratch files, larger files,
multiple files and repository retrieval are unsupported. Chat history is bounded
to 20 entries, requests to 64 KiB of serialized messages, replies to 1 MiB, output
text to 32 KiB, and each edit's old/new text to 8 KiB. Network calls have a 5-second
connect timeout and a 120-second overall timeout. Invalid model output is surfaced
as an error and never silently substituted with generated fallback content.

## Validation and external provenance

The headless native test uses an actual Cargo workspace to check initial opt-out,
trust, review-before-apply, stale edits, unchanged disk contents, inline Tab accept
and undo. A transport test checks cloud rejection, redirects, oversized replies
and aborting an in-flight request. The opt-in model test talks to a separately
started local Ollama, asks a Rust type question and requests structured line/edit
proposals; accepted generated Rust is compiled and executed with rustc.

The validation service uses official Ollama v0.40.14 (MIT) and
`qwen2.5-coder:0.5b` (approximately 398 MB; model ID `4ff64a7f502a`, Apache-2.0).
The release archive was checked against the official SHA-256 manifest. These
external tools and model weights live outside the repository and are not
redistributed in CrabCraft. Model quality and latency depend on the selected
model and machine; no comparison against RustRover was performed.

Public references:

- [Ollama Linux manual installation](https://docs.ollama.com/linux)
- [Ollama chat API](https://docs.ollama.com/api/chat)
- [Installed models API](https://docs.ollama.com/api/tags)
- [Model details API](https://docs.ollama.com/api-reference/show-model-details)
- [Ollama API types, v0.40.14](https://github.com/ollama/ollama/blob/v0.40.14/api/types.go)
- [Cloud and disabling cloud features](https://docs.ollama.com/cloud)
- [Ollama MIT license](https://github.com/ollama/ollama/blob/v0.40.14/LICENSE)
- [Qwen2.5-Coder-0.5B-Instruct Apache-2.0 license](https://huggingface.co/Qwen/Qwen2.5-Coder-0.5B-Instruct/blob/main/LICENSE)
