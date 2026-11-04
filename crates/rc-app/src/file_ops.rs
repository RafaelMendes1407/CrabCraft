//! Workspace edits that create or rename files, such as rust-analyzer's "Rename to `snake_case`"
//! fix for a module file. The operations run in the order the server gave them, mixed with
//! the text edits. The edit comes from the language server, so every path must be inside the
//! workspace; deleting files is refused (not supported yet).

use crate::app::CrabCraftApp;
use rc_lsp::uri_to_path;
use rc_kit::i18n::{tr, trf};
use rc_lsp::lsp_types as lsp;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Whether the edit creates, renames or deletes a file.
pub fn has_file_ops(edit: &lsp::WorkspaceEdit) -> bool {
    matches!(&edit.document_changes, Some(lsp::DocumentChanges::Operations(ops)) if ops.iter().any(|o| matches!(o, lsp::DocumentChangeOperation::Op(_))))
}

/// A path from the server, if it is inside `root` and names no `..`.
fn inside(root: &Path, uri: &lsp::Uri) -> Result<PathBuf, String> {
    let path = uri_to_path(uri).ok_or_else(|| tr("The language server sent a path that is not a file").to_string())?;
    if !path.starts_with(root) || path.components().any(|c| c == Component::ParentDir) {
        return Err(trf("The language server asked to change a file outside the project: {file}", &[("file", &path.display().to_string())]));
    }
    Ok(path)
}

/// Applies `edit` in order. Returns the number of files changed or created; on the first
/// failure stops and says why (what was done before stays done).
pub fn apply(app: &mut CrabCraftApp, edit: &lsp::WorkspaceEdit) -> Result<usize, String> {
    let Some(lsp::DocumentChanges::Operations(ops)) = &edit.document_changes else {
        return crate::insight::apply_workspace_edit(&mut app.docs, edit);
    };
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone()).ok_or_else(|| tr("Open a project to change its files").to_string())?;
    // Checked first, so a refused operation changes nothing.
    for op in ops {
        match op {
            lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Delete(_)) => return Err(tr("This action deletes files, which is not supported yet").to_string()),
            lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(c)) => drop(inside(&root, &c.uri)?),
            lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(r)) => {
                inside(&root, &r.old_uri)?;
                inside(&root, &r.new_uri)?;
            }
            lsp::DocumentChangeOperation::Edit(e) => drop(inside(&root, &e.text_document.uri)?),
        }
    }
    let mut count = 0;
    for op in ops {
        match op {
            lsp::DocumentChangeOperation::Edit(e) => {
                let path = inside(&root, &e.text_document.uri)?;
                let edits = e
                    .edits
                    .iter()
                    .map(|x| match x {
                        lsp::OneOf::Left(t) => t.clone(),
                        lsp::OneOf::Right(a) => a.text_edit.clone(),
                    })
                    .collect();
                let closed = app.docs.index_of(&path).is_none();
                count += crate::insight::apply_file_edits(&mut app.docs, BTreeMap::from([(path.clone(), edits)]))?;
                if closed {
                    app.insight.files_changed(&[(path, 2)]);
                }
            }
            lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(c)) => {
                let path = inside(&root, &c.uri)?;
                let options = c.options.as_ref();
                if path.exists() {
                    if options.and_then(|o| o.overwrite) != Some(true) {
                        if options.and_then(|o| o.ignore_if_exists) == Some(true) {
                            continue;
                        }
                        return Err(trf("{file} exists already", &[("file", &path.display().to_string())]));
                    }
                    if !path.is_file() {
                        return Err(trf("{file} exists already", &[("file", &path.display().to_string())]));
                    }
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, "").map_err(|e| e.to_string())?;
                app.explorer.invalidate(&path);
                app.insight.files_changed(&[(path, 1)]);
                count += 1;
            }
            lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(r)) => {
                let (from, to) = (inside(&root, &r.old_uri)?, inside(&root, &r.new_uri)?);
                if to.exists() {
                    if r.options.as_ref().and_then(|o| o.ignore_if_exists) == Some(true) && r.options.as_ref().and_then(|o| o.overwrite) != Some(true) {
                        continue;
                    }
                    // Overwriting is refused: it could lose the other file.
                    return Err(trf("{file} exists already", &[("file", &to.display().to_string())]));
                }
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                crate::file_rename::apply(app, &from, &to, &[])?;
                count += 1;
            }
            lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Delete(_)) => {}
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(p: &Path) -> lsp::Uri {
        rc_lsp::path_to_uri(p)
    }

    #[test]
    fn only_edits_with_operations_have_file_ops() {
        let root = Path::new("/w");
        let plain = lsp::WorkspaceEdit::default();
        assert!(!has_file_ops(&plain));
        let rename = lsp::WorkspaceEdit {
            document_changes: Some(lsp::DocumentChanges::Operations(vec![lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(lsp::RenameFile {
                old_uri: uri(&root.join("src/MyMod.rs")),
                new_uri: uri(&root.join("src/my_mod.rs")),
                options: None,
                annotation_id: None,
            }))])),
            ..Default::default()
        };
        assert!(has_file_ops(&rename));
    }

    #[test]
    fn paths_must_stay_in_the_project() {
        let root = Path::new("/w/demo");
        assert!(inside(root, &uri(Path::new("/w/demo/src/a.rs"))).is_ok());
        assert!(inside(root, &uri(Path::new("/w/other/a.rs"))).is_err());
        assert!(inside(root, &uri(Path::new("/etc/passwd"))).is_err());
    }
}
