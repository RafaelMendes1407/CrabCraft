# Troubleshooting

Use the visible error message and the guide for the affected feature to narrow the problem.
The [feature inventory](../FEATURES.md) distinguishes supported workflows from partial ones.

| Symptom | Check or next action |
| --- | --- |
| No completion, navigation or Cargo targets | Check for Restricted mode, then review and trust the project if appropriate. Confirm rust-analyzer and Cargo are installed and configured in Settings → Tools. |
| Tool cannot be found | Check PATH or configure an explicit executable path in Settings → Tools. A configured missing debugger path reports an error rather than selecting another backend. |
| Build or test fails | Read the Cargo output and navigate to the reported file. Check the selected package, target, toolchain, features and project environment. |
| Formatting fails | Check rustfmt availability, project rustfmt.toml, arguments, selected channel and environment. A save/format failure aborts dependent execution. |
| Debugger does not start | Install GDB 14+ or LLVM lldb-dap, select it in Tools, and inspect the adapter error. Linux ptrace restrictions can prevent attach/launch. See [debugger setup](rust-debuggers.md). |
| A shortcut runs another action | Check Settings → Keymap and your overrides. The reference shortcuts describe a particular profile and platform. |
| Replacement is unavailable | Complete the search and narrow it if results are truncated. Review [replacement limits](find-replace.md). |
| File opens read-only or is refused | Files above 8 MiB open read-only; files above 128 MiB, special files and unsupported binary/invalid UTF-8 input are refused. See [document I/O](document-io.md). |
| Terminal or Git is inactive | Project trust is required. Terminal startup is explicit; Git also needs an installed executable. |
| Optional service is unavailable | Install/configure its separately required tool or service. Opening a workspace does not start Ollama, a database or a container daemon. |
| Resize behaves differently under Wayland | Report compositor, desktop, display backend and reproduction steps. The repeated-resize fix has headless tests; real KDE/KWin verification remains pending. |

When reporting a problem, include the CrabCraft version/commit, OS, display backend for UI
problems, selected tool versions, reproduction steps, expected result and actual error.
Use a small project you can share and redact credentials, private paths and request bodies.
Report vulnerabilities privately according to [SECURITY.md](../SECURITY.md).
