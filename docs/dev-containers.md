# Native Dev Container CLI controls

The Dev Containers toolwindow invokes the separately installed reference `devcontainer` CLI through native egui controls. This is an explicit CLI integration, **not a container-backed IDE session**: the editor, Rust language server, build/run configurations and debugger do not move into the container. That limitation is visible in the UI. Installation, automatic attach and remote IDE services are unsupported.

No CLI or lifecycle action starts when opening a project or restoring a layout. Program paths, optional local Docker endpoint, command text and output stay in memory. After project trust, Check reads the CLI version and Read Configuration requests the CLI's normal configuration output for the active workspace, including its JSONC support and default `.devcontainer/devcontainer.json` or `.devcontainer.json` lookup. Neither action claims to validate a container runtime. Read Configuration may query Docker to find an existing container; it is an explicit trusted action, not a passive parser on project opening.

Build, Up and Exec each prepare an exact argv preview and require a separate confirmation. The backend independently checks trust and confirmation. The workspace and executable/endpoint configuration must still match when Execute is clicked. The IDE does not parse host shell command text: Exec has a separate program and at most 32 individual argument lines, passed after `--`. The reference CLI itself applies the selected devcontainer configuration and can run its configured lifecycle commands.

**Trust the whole configuration before using these controls.** The reference CLI can fetch images/features, mount host directories, run `initializeCommand` on the host, run container lifecycle hooks and execute project code. Its process has your OS permissions and Docker daemon access can grant host-level privileges. The CLI can also generate lockfiles or other configured files. These controls do not sandbox that behavior or suppress configuration hooks. Read the full configuration in the editor before confirming, rather than relying on the abbreviated output preview.

The CLI receives an absolute canonical workspace path through `--workspace-folder`, an explicit Docker executable through `--docker-path`, and an optional local `unix://`/`npipe://` endpoint through the worker's `DOCKER_HOST`. Project Runner environment overrides are not forwarded, and the CLI starts in a neutral temporary directory. Ordinary IDE environment and existing user Docker credential configuration remain available. The IDE does not store registry credentials.

Workers bound each output pipe to 4 MiB, use a 60-second read deadline or ten-minute mutating-operation deadline, and show at most 16,384 output characters. Cancel, closing the workspace, dropping the model or revoking trust stop the local CLI process tree. This does not promise rollback of daemon-side work or termination of every already-detached lifecycle process/container. A visible cancellation message explains that explicit management may still be necessary. Use the Docker toolwindow to stop/remove containers after reviewing the chosen resource. The reference CLI itself currently does not implement `stop` or `down`; the IDE does not invent those commands.

## Validation and limits

`cargo test -p rc-app dev_containers::tests -- --nocapture` passes three default tests using a real Rust Cargo workspace and deterministic CLI. They cover typed argument handling, invalid executable/remote endpoint forms, backend trust/confirmation, cancellation and the native Up review/checkbox/execute flow. Before an explicit reviewed invocation, neither the fixture CLI, project build script nor configuration host hook runs.

The ignored `official_cli_reads_real_rust_jsonc_without_running_lifecycle_hooks` test passed with the separately installed official CLI 0.90.0. It uses a deterministic empty Docker catalog to check real JSONC reading without running `initializeCommand` or `build.rs`. The result is recorded in `/tmp/crabcraft-devcontainers-real.log` for this implementation session. It proves CLI/configuration compatibility, not actual container creation. F-09 has a separate real-engine fixture. Until real Up and Exec are exercised with an available daemon, and IDE service integration is implemented, Y-09 remains partial. No authenticated registry, remote service or Docker Hub image pull is used in the default tests.

## Public references

- [Reference CLI commands, lifecycle behavior and current stop/down limits](https://github.com/devcontainers/cli)
- [Development Container supporting tools and services](https://containers.dev/supporting)
- [Development Container specification](https://containers.dev/implementors/spec/)

No third-party CLI, container image or proprietary asset is distributed with CrabCraft.
