# Native Docker controls

The Docker toolwindow is a native egui client for a separately installed Docker CLI and daemon. It never installs or starts a daemon and never runs a Docker process merely because a project or its layout opens. The executable and optional local `unix://` or Windows `npipe://` daemon endpoint live only in memory. An empty endpoint uses Docker's configured default; project Runner environment overrides and project-local working-directory configuration are not forwarded.

After trusting the project, explicitly check client/server version, list containers or images, or inspect the last 200 timestamped log lines from a selected container. Container IDs are validated hexadecimal values from the list, not arbitrary command arguments. The installed CLI must be trusted: Docker daemon access can grant host-level privileges. The IDE does not claim to sandbox Docker or to enforce host security policy.

Build, Run, Stop and Remove each prepare a frozen, quoted argv preview. They require a separate confirmation checkbox before execution; the backend enforces project trust and confirmation too. Changing the executable, endpoint or project invalidates the prepared command. The preview is display only; the IDE starts a process with separate arguments and no host shell.

- Build accepts an absolute build-context directory and Dockerfile within the active workspace, plus an image tag. Paths are canonicalized and checked in the worker. The command requests `--network=none` for build steps and `--pull=false`. **This does not guarantee an offline build**: Docker can still fetch missing base images or frontends. Docker can send the entire context to its daemon, so review `.dockerignore` and the selected context before confirming. Dockerfile instructions can execute code.
- Run accepts an existing local image and at most 32 separate command arguments. It uses `--detach`, `--pull=never`, no host mounts and `--network=none` by default. Bridge networking requires a visible checkbox. The image's configured entrypoint still applies. Running image code is explicit.
- Stop targets the selected exact container ID with a ten-second grace period. Remove targets the selected ID without `--force`; Docker refuses removal of a running container. These affect daemon resources and are not undoable. A review is always required.

All processes run in a background worker, with process-tree cancellation, 4 MiB per output pipe, 60-second ordinary deadlines and a ten-minute build deadline. The UI displays at most 16,384 output characters and at most 1,000 container rows. Cancellation stops the local CLI request and its children. It **does not claim to roll back daemon-side work or delete a container already created by a detached run**; the visible cancellation message directs the user to explicit Stop or Remove when needed. Closing the workspace or revoking trust cancels the request.

Docker installation, Compose, registry authentication, arbitrary extra CLI flags, port forwarding, mounts/volumes and container-backed IDE language services/debugging are visibly unsupported in this initial toolwindow. No registry credentials are stored by CrabCraft; Docker may use its existing user credential configuration. Dev Containers are a separate integration rather than an implication of these controls.

## Validation

`cargo test -p rc-app containers::tests -- --nocapture` passes three default tests. They use a real Rust Cargo project and a deterministic CLI fixture to prove that opening/untrusted actions do not start a process or run `build.rs`, writes require confirmation even at the backend, typed argv preserves shell-looking text, invalid IDs/remote endpoint forms/outside-project build paths are refused, cancellation terminates a long process, and the native list/review/checkbox/execute flow works.

The ignored `real_rootless_engine_builds_and_runs_a_static_rust_project` test requires an explicitly provided disposable daemon. It compiles the project's Rust source with `crt-static`, builds a local `FROM scratch`/`COPY` image, runs it without pull/network, verifies exit and log output, then removes its container. It makes no image pulls or public writes. The actual Docker 29.9.3 daemon successfully built the `FROM scratch` image from the real static Rust executable. The subsequent Run was refused because the unprivileged daemon could not read `/sys/kernel/security/apparmor/profiles` to check `docker-default`. The real test therefore failed at Run, with evidence in `/tmp/crabcraft-docker-real.log`; there is no claim that the container executed. No AppArmor policy was relaxed and no hidden security options were added. F-09 remains partial; a successful build, mock CLI and version check do not prove container execution.

Our temporary daemon probe uses an ordinary host UID 1000 mapped to namespace UID 0, private mount/PID/network namespaces, a private `/run` bind mount, a temporary Unix socket/data/config and vfs storage. This final probe runs Docker without its `--rootless` flag or RootlessKit integration, entirely inside the ordinary user’s namespaces; it does not run as host root or fabricate a RootlessKit API. It neither uses host root/sudo nor changes global AppArmor/system settings. The daemon became ready and built the image, but its AppArmor limitation prevented container execution. This is recorded rather than bypassed.

## Public references

- [Docker CLI reference](https://docs.docker.com/reference/cli/docker/)
- [Build options and legacy backend caveats](https://docs.docker.com/reference/cli/docker/image/build/)
- [Docker rootless requirements](https://docs.docker.com/engine/security/rootless/)
- [Official static binaries, suitable for testing](https://docs.docker.com/engine/install/binaries/)

No Docker binary is distributed with CrabCraft.
