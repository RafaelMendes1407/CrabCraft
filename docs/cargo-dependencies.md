# Cargo dependency manager

Open the Cargo tool window and choose **Manage Cargo Dependencies…**. Select a package from
its workspace (attached Cargo projects are included). Opening the manager reads only that
package's manifest on a worker thread. It does not search the network or run project code.

The window has a toolbar (package, Refresh, Offline), banners for missing Cargo, untrusted
projects and unsaved editors, and two panes. The left pane lists the installed dependencies
grouped by table, or the crates.io search with its results; **New dependency** clears the form.
The right pane shows the selected or new dependency with its form, **Add or update dependency**
and, for an installed one, **Remove…** with an inline confirmation. Cargo output appears below,
with terminal colour codes removed, and the window keeps its size so its buttons do not move.

Use **Search** to query crates.io for the typed crate name. Select a result to fill the name
and stable version, or enter a name and version requirement directly. Add or update dependencies
in `dependencies`, `dev-dependencies` or `build-dependencies`, with features, default features,
optional dependencies (except dev dependencies) and an optional local path. Paths are relative
to the selected package's folder. **Offline** applies to Cargo operations; the crates.io search
is a separate explicit network action.

Changes require a trusted project and saved Cargo.toml editors. Cargo edits the manifest and
lock file in the background using `cargo add` or `cargo remove`, with the project's environment.
Removal asks for confirmation. The modal displays bounded command output and supports cancellation;
closing it also cancels. Cargo changes made before cancellation or an error may remain on disk.
The project model is refreshed after an operation.

Cargo adds to existing enabled features rather than replacing them. Remove individual features
in Cargo.toml. Inherited dependencies, Git sources, alternate registries, target-specific tables
and workspace-level dependency tables must be edited in Cargo.toml; direct inherited, Git and
alternate-registry entries can still be removed through this manager. There are no automatic
update checks, registry-account settings or publish actions in this window.

Validation uses temporary Cargo workspaces with local path dependencies: add, update and remove
across all three scopes, feature flags, optional/default-feature changes, member isolation,
trust rejection and native UI add/remove interactions. Build scripts are instrumented to prove
these operations do not compile the test projects. Command syntax follows the installed Cargo's
public `cargo add --help` and `cargo remove --help`.
