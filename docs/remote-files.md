# Native SSH terminals and reviewed SFTP files

Remote files is a native egui tool window. Opening a project never discovers hosts, reads SSH
configuration, authenticates, downloads, uploads or runs a remote command. The workspace must
be trusted, and every connection/transfer/terminal starts from an explicit button. Connection
fields, private key paths, previews and buffers remain in memory; none are project settings.

Install compatible OpenSSH `ssh` and `sftp` clients on the IDE host. Enter the host name/IPv4,
port, user, private key and an independently verified known_hosts file. Host-key checking is
mandatory. Unknown/changed keys fail; there is no accept-any-host button. Password prompts,
interactive key passphrases, inherited SSH configuration/aliases, agents, jump hosts, proxies,
forwarding and IPv6 literals are currently unsupported. FTP is visibly unsupported.

The worker makes private temporary copies of the key and known_hosts contents. OpenSSH runs
with `-F none`, BatchMode/IdentitiesOnly/StrictHostKeyChecking enabled, IdentityAgent disabled,
and proxy, control socket, local commands, X11 and agent/port forwarding disabled. No local
shell interprets fields or batch commands. Remote paths must be absolute and cannot contain
control characters, quotes, backslashes, glob syntax or parent traversal. Host/user values use
an explicit restricted character set. No extension executable is selected from project files.

## Review, edit and deploy an existing file

Load remote file reads an existing UTF-8 text file into a read-only snapshot and a proposed
editable buffer. Preview remote edit first downloads again and rejects a changed snapshot.
Preview local deployment compares the selected regular local file with an existing remote
file. Neither preview uploads data. Review the before/after panes and destination, then choose
Confirm remote overwrite. Confirmation checks both the remote contents and (for deployment)
the original local bytes again. A mismatch fails without overwriting the destination.

The transfer uploads to a unique adjacent temporary name and renames it over the destination.
OpenSSH automatically uses the `posix-rename@openssh.com` extension when available; servers
without compatible rename replacement may fail safely. There is no remove-and-write fallback.
Replacement creates a new remote file: ownership, ACLs, hard links, timestamps and custom mode
bits are not preserved. A destination symlink is replaced, not a promise to edit its referent.
Use this on files whose replacement semantics and permissions you have reviewed.

SFTP provides no atomic content compare-and-swap or lock. Another writer can change a file
between verification and rename. The UI states this limitation; stale checks are not a lock.
Cancellation/error can leave the remote temporary file behind, and a completed rename may
already have committed. There is no silent retry, automatic upload on save, new-file creation,
directory synchronization, binary transfer, remote filesystem browser or remote Rust toolchain.
Y-05 remains partial because FTP and broader deployment workflows are not implemented.

## Explicit terminal

Open SSH terminal prepares the same strict connection on a worker and starts the normal native
PTY terminal asynchronously. The login shell, remote login startup files and commands typed
there run on the server only after this explicit action. No command is added automatically.
Normal terminal close/interrupt controls apply. At most eight private terminal credential
snapshots are retained for a project session; close the project to discard them. Closing the
project stops its terminals and pending work.

## Bounds and evidence

Transfers permit UTF-8 without NUL, up to 2 MB, bounded local regular non-symlink files, private
keys up to 64 KB, known_hosts up to 256 KB, 32 KB process output and a 15-second batch deadline.
A worker monitors downloads and cancels the whole process group when the file exceeds its
budget. The disk limit has a polling/buffering overshoot; this is not an OS quota or sandbox.
Cancel/project close/pane close discards stale generations and cancels pending transfers.
Native previews show up to 16,384 characters. In-pane editing is limited to 64 KB; larger text
files can be deployed from an explicitly selected local file after reviewing that full file.

`cargo test -p rc-app remote_command_paths --offline -- --nocapture` checks batch-injection
refusal and special-file refusal. The ignored integration tests require an explicitly started
private OpenSSH server at 127.0.0.1:55222, user rafael, with the exclusive key/known_hosts under
/tmp/crabcraft-ssh:

- `real_sftp_rust_file_preview_stale_deploy_hostkey_and_cancel` exercises actual SFTP reads,
  edits, deployment, both stale guards, unknown-host refusal, and cancellation against a socket
  that stalls the SSH handshake. A standalone Rust project is then compiled/run offline from
  the transferred source, and its build.rs marker stays absent before explicit compilation.
- `native_remote_preview_confirmation_stale_guard_and_explicit_ssh_terminal` clicks the native
  trust-gated load/preview/confirm controls, verifies refusal after an external edit, confirms a
  real upload, and types a command into the explicitly opened native SSH terminal to write a
  server-side verification marker.

This does not claim RustRover parity, remote development, containers, or general SFTP-server
compatibility beyond the tested OpenSSH server/client.

## Public references

- [OpenSSH sftp manual](https://man.openbsd.org/sftp.1): batch mode, quoted paths, get/put/rename.
- [OpenSSH SSH configuration](https://man.openbsd.org/ssh_config.5) and [ssh manual](https://man.openbsd.org/ssh.1): host-key checks, explicit identities, configuration isolation and terminal startup.
- [OpenSSH 9.6 client source](https://github.com/openssh/openssh-portable/blob/V_9_6_P1/sftp-client.c): automatic POSIX rename extension selection in sftp_rename.

OpenSSH is an external user-installed tool. No OpenSSH source, assets or documentation prose
are distributed as part of this implementation.
