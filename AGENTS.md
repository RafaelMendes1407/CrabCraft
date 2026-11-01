# CrabCraft working rules

- Keep the desktop product in native Rust (egui). Do not add a webview shell.
- `FEATURES.md` is the status of record. Update it in the commit that changes a feature, then run
  `python3 scripts/features_summary.py`.
- A feature is ✅ only with evidence that it works on a real Rust project. Inherited DotCraft code
  stays 🟢 or 🔁 until then. Never claim parity with RustRover without comparing against it.
- Follow `docs/architecture.md`: the UI thread never blocks, long work is cancellable, and all
  user-facing text goes through `rc_kit::i18n::tr` with translations in `locales/`.
- Make small, reviewable commits in English. Unfinished controls must be visibly unsupported.
- Treat files, process output and settings as untrusted. Return typed errors; no input-reachable
  `unwrap`, `expect` or panics.
- Opening a project must not run its code. `build.rs` and proc macros run only after the user
  trusts the project.
- Use public documentation as a reference. Do not copy JetBrains text, proprietary assets or marks.
  Record the source and license of any distributed asset.
- Keep the DotCraft copyright and license notices. Keep the `rc-*` crate names so upstream changes
  can still be merged.
- Do not create releases or change the repository's visibility without the user's request.
