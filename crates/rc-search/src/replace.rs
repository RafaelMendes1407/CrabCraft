//! Replace in Files: replacements for every match of a [`Query`] (with `$1`-style groups in
//! regex mode), either written to disk atomically or returned as [`TextEdit`]s for open
//! buffers so the editor applies them as one undoable change. Line breaks inserted by the
//! replacement follow the text's line endings; a UTF-8 BOM is kept.

use crate::query::{CharOffsets, Query};
use std::fmt;
use std::io::{Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Replace the chars in `range` with `text`. Edits come sorted and never overlap; apply them
/// from last to first (or shift offsets) so earlier ranges stay valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub text: String,
}

/// Where a file's replacements go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaceTarget {
    /// Rewrite the file on disk.
    Disk,
    /// The file is open: compute edits on this buffer text instead.
    Buffer(String),
}

/// What happened to one file.
#[derive(Debug)]
pub enum ReplaceOutcome {
    /// The file was rewritten with this many replacements (0: untouched).
    Written(usize),
    /// Edits for the open buffer.
    Edits(Vec<TextEdit>),
    Failed(ReplaceError),
}

#[derive(Debug)]
pub enum ReplaceError {
    Io(std::io::Error),
    /// The file is not valid UTF-8 (it is left untouched).
    NotUtf8,
    Changed,
    NotRegular,
    ReadOnly,
    Limit,
    Cancelled,
}

impl fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplaceError::Io(e) => e.fmt(f),
            ReplaceError::NotUtf8 => f.write_str("file is not valid UTF-8"),
            ReplaceError::Changed => f.write_str("file changed since the search"),
            ReplaceError::NotRegular => f.write_str("file is not a regular file"),
            ReplaceError::ReadOnly => f.write_str("file is read-only"),
            ReplaceError::Limit => f.write_str("replacement exceeded its size or edit limit"),
            ReplaceError::Cancelled => f.write_str("replacement cancelled"),
        }
    }
}

impl std::error::Error for ReplaceError {}

impl From<std::io::Error> for ReplaceError {
    fn from(e: std::io::Error) -> Self {
        ReplaceError::Io(e)
    }
}

/// Metadata captured by the search worker. It is checked again before replacing a closed
/// file; Unix identity and change times also reject swaps or restored modification times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    readonly: bool,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl FileStamp {
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino(), metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReplaceLimits {
    pub bytes: usize,
    pub edits: usize,
}
impl Default for ReplaceLimits {
    fn default() -> Self {
        Self { bytes: 8 * 1024 * 1024, edits: 100_000 }
    }
}

pub struct ReplacePlan {
    pub edits: Vec<TextEdit>,
    pub output: String,
}

fn cancelled(cancel: &AtomicBool) -> Result<(), ReplaceError> {
    if cancel.load(Ordering::Relaxed) { Err(ReplaceError::Cancelled) } else { Ok(()) }
}

/// Bounded computation for both open buffers and closed files, with cancellation between
/// matches. Byte-level regex matches may not split a UTF-8 character.
pub fn prepare_replacement(text: &str, query: &Query, replacement: &str, cancel: &AtomicBool, limits: ReplaceLimits) -> Result<ReplacePlan, ReplaceError> {
    cancelled(cancel)?;
    if text.len() > limits.bytes || replacement.len() > 64 * 1024 {
        return Err(ReplaceError::Limit);
    }
    let crlf = text.find('\n').is_some_and(|index| index > 0 && text.as_bytes()[index - 1] == b'\r');
    let mut edits = Vec::new();
    let mut convert = CharOffsets::new(text);
    let mut output = String::new();
    let (mut at, mut last) = (0, 0);
    while let Some(captures) = query.captures_at(text.as_bytes(), at) {
        cancelled(cancel)?;
        let Some(found) = captures.get(0) else { break };
        at = found.end();
        if !text.is_char_boundary(found.start()) || !text.is_char_boundary(found.end()) {
            return Err(ReplaceError::NotUtf8);
        }
        let expanded = query.expand_bounded(&captures, replacement, limits.bytes, cancel);
        cancelled(cancel)?;
        let mut expanded = expanded.ok_or(ReplaceError::Limit)?;
        if crlf && expanded.contains('\n') {
            expanded = expanded.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        if text[found.range()] == expanded {
            continue;
        }
        if edits.len() >= limits.edits {
            return Err(ReplaceError::Limit);
        }
        if output.len().saturating_add(found.start() - last).saturating_add(expanded.len()).saturating_add(text.len() - found.end()) > limits.bytes {
            return Err(ReplaceError::Limit);
        }
        output.push_str(&text[last..found.start()]);
        output.push_str(&expanded);
        last = found.end();
        edits.push(TextEdit { range: convert.at(found.start())..convert.at(found.end()), text: expanded });
    }
    cancelled(cancel)?;
    output.push_str(&text[last..]);
    Ok(ReplacePlan { edits, output })
}

fn read_guarded(path: &Path, expected: &FileStamp, cancel: &AtomicBool, limit: usize) -> Result<(Vec<u8>, std::fs::Metadata), ReplaceError> {
    cancelled(cancel)?;
    let before = std::fs::symlink_metadata(path)?;
    if !before.is_file() {
        return Err(ReplaceError::NotRegular);
    }
    if before.permissions().readonly() {
        return Err(ReplaceError::ReadOnly);
    }
    if &FileStamp::from_metadata(&before) != expected {
        return Err(ReplaceError::Changed);
    }
    if before.len() > limit as u64 {
        return Err(ReplaceError::Limit);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(ReplaceError::NotRegular);
    }
    if &FileStamp::from_metadata(&metadata) != expected {
        return Err(ReplaceError::Changed);
    }
    let mut bytes = Vec::new();
    let mut block = [0; 64 * 1024];
    loop {
        cancelled(cancel)?;
        let count = file.read(&mut block)?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > limit {
            return Err(ReplaceError::Limit);
        }
        bytes.extend_from_slice(&block[..count]);
    }
    if &FileStamp::from_metadata(&file.metadata()?) != expected {
        return Err(ReplaceError::Changed);
    }
    Ok((bytes, metadata))
}

/// Replaces a regular closed file only if its search-time stamp and its exact original
/// bytes still match, using an atomic same-folder persist. Already replaced files remain
/// replaced when a later file fails or the job is cancelled.
pub fn replace_in_file_guarded(
    path: &Path,
    expected: &FileStamp,
    query: &Query,
    replacement: &str,
    cancel: &AtomicBool,
    limits: ReplaceLimits,
) -> Result<(usize, usize), ReplaceError> {
    let (bytes, metadata) = read_guarded(path, expected, cancel, limits.bytes)?;
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(body) => (true, body),
        None => (false, bytes.as_slice()),
    };
    let text = std::str::from_utf8(body).map_err(|_| ReplaceError::NotUtf8)?;
    let body_limits = ReplaceLimits { bytes: limits.bytes.saturating_sub(if bom { BOM.len() } else { 0 }), ..limits };
    let plan = prepare_replacement(text, query, replacement, cancel, body_limits)?;
    if plan.edits.is_empty() {
        return Ok((0, 0));
    }
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new().prefix(".crabcraft-replace").tempfile_in(parent)?;
    temp.as_file().set_permissions(metadata.permissions())?;
    if bom {
        temp.write_all(BOM)?;
    }
    for block in plan.output.as_bytes().chunks(64 * 1024) {
        cancelled(cancel)?;
        temp.write_all(block)?;
    }
    temp.as_file().sync_all()?;
    let (current, _) = read_guarded(path, expected, cancel, limits.bytes)?;
    if current != bytes {
        return Err(ReplaceError::Changed);
    }
    cancelled(cancel)?;
    temp.persist(path).map_err(|error| ReplaceError::Io(error.error))?;
    Ok((plan.edits.len(), plan.output.len() + if bom { BOM.len() } else { 0 }))
}

/// Every match of `query` in `text` with its replacement (byte ranges).
fn replacements(text: &str, query: &Query, replacement: &str) -> Vec<(Range<usize>, String)> {
    let crlf = text.find('\n').is_some_and(|i| i > 0 && text.as_bytes()[i - 1] == b'\r');
    let hay = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    let mut buf = Vec::new();
    while let Some(caps) = query.captures_at(hay, at) {
        let Some(m) = caps.get(0) else { break };
        buf.clear();
        query.expand(&caps, replacement, &mut buf);
        let mut s = String::from_utf8_lossy(&buf).into_owned();
        if crlf && s.contains('\n') {
            s = s.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        out.push((m.range(), s));
        at = m.end();
    }
    out
}

/// Edits (char ranges) replacing every match in `text`.
pub fn compute_edits(text: &str, query: &Query, replacement: &str) -> Vec<TextEdit> {
    let mut conv = CharOffsets::new(text);
    replacements(text, query, replacement).into_iter().map(|(r, s)| TextEdit { range: conv.at(r.start)..conv.at(r.end), text: s }).collect()
}

/// `text` with every match replaced, and the number of replacements.
pub fn replace_text(text: &str, query: &Query, replacement: &str) -> (String, usize) {
    let reps = replacements(text, query, replacement);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (r, s) in &reps {
        out.push_str(&text[last..r.start]);
        out.push_str(s);
        last = r.end;
    }
    out.push_str(&text[last..]);
    (out, reps.len())
}

/// Replaces every match in the file and writes it back atomically (temporary file in the
/// same folder, then rename), keeping its permissions and BOM. Returns the number of
/// replacements; the file is not touched when there are none.
pub fn replace_in_file(path: &Path, query: &Query, replacement: &str) -> Result<usize, ReplaceError> {
    let bytes = std::fs::read(path)?;
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, &bytes[..]),
    };
    let text = std::str::from_utf8(body).map_err(|_| ReplaceError::NotUtf8)?;
    let (new_text, count) = replace_text(text, query, replacement);
    if count == 0 || new_text == text {
        return Ok(count);
    }
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut tmp = tempfile::Builder::new().prefix(".crabcraft-replace").tempfile_in(dir)?;
    if bom {
        tmp.write_all(BOM)?;
    }
    tmp.write_all(new_text.as_bytes())?;
    tmp.as_file().sync_all()?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = tmp.as_file().set_permissions(meta.permissions());
    }
    tmp.persist(path).map_err(|e| ReplaceError::Io(e.error))?;
    Ok(count)
}

/// Replaces in many files, choosing per file between disk and an open buffer.
pub fn replace_in_files(items: impl IntoIterator<Item = (PathBuf, ReplaceTarget)>, query: &Query, replacement: &str) -> Vec<(PathBuf, ReplaceOutcome)> {
    items
        .into_iter()
        .map(|(path, target)| {
            let outcome = match target {
                ReplaceTarget::Buffer(text) => ReplaceOutcome::Edits(compute_edits(&text, query, replacement)),
                ReplaceTarget::Disk => match replace_in_file(&path, query, replacement) {
                    Ok(n) => ReplaceOutcome::Written(n),
                    Err(e) => ReplaceOutcome::Failed(e),
                },
            };
            (path, outcome)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::QueryOptions;

    fn regex(p: &str) -> Query {
        Query::new(QueryOptions { pattern: p.into(), regex: true, case_sensitive: true, whole_word: false }).unwrap()
    }

    #[test]
    fn groups_expand_in_regex_mode() {
        let q = regex(r"(\w+)\.Get(\w+)\(\)");
        let (out, n) = replace_text("a.GetName(); b.GetAge();", &q, "$1.$2");
        assert_eq!((out.as_str(), n), ("a.Name; b.Age;", 2));
        let q = regex(r"(?P<x>\d+)");
        assert_eq!(replace_text("v1 v22", &q, "<${x}>$$").0, "v<1>$ v<22>$");
    }

    #[test]
    fn plain_mode_inserts_replacement_literally() {
        let q = Query::new(QueryOptions::text("cost")).unwrap();
        assert_eq!(replace_text("Cost cost", &q, "$1").0, "$1 $1");
    }

    #[test]
    fn edits_use_char_offsets() {
        let q = Query::new(QueryOptions::text("é")).unwrap();
        let edits = compute_edits("ñé ñé", &q, "e");
        assert_eq!(edits, [TextEdit { range: 1..2, text: "e".into() }, TextEdit { range: 4..5, text: "e".into() }]);
    }

    #[test]
    fn inserted_line_breaks_follow_crlf() {
        let q = regex(r";");
        assert_eq!(replace_text("a;b\r\nc", &q, r";\n").0, "a;\r\nb\r\nc");
        assert_eq!(replace_text("a;b\nc", &q, r";\n").0, "a;\nb\nc");
    }
}

#[cfg(test)]
mod guarded_tests {
    use super::*;
    use crate::QueryOptions;

    #[test]
    fn bounded_groups_match_existing_expansion_and_refuse_large_captures() {
        let query = Query::new(QueryOptions { pattern: r"(?P<name>é\w+)=(\d+)".into(), regex: true, case_sensitive: true, whole_word: false }).unwrap();
        let source = "// Unicode\r\névalue=12; éother=34\r\n";
        let replacement = "${name}:$2\\n$$";
        let plan = prepare_replacement(source, &query, replacement, &AtomicBool::new(false), ReplaceLimits::default()).unwrap();
        assert_eq!(plan.output, replace_text(source, &query, replacement).0);
        assert_eq!(plan.edits.len(), 2);
        let query = Query::new(QueryOptions { pattern: "(a+)".into(), regex: true, ..Default::default() }).unwrap();
        let input = "a".repeat(1000);
        assert!(matches!(
            prepare_replacement(&input, &query, &"$1".repeat(1000), &AtomicBool::new(false), ReplaceLimits { bytes: 1024, edits: 100 }),
            Err(ReplaceError::Limit)
        ));
        assert!(matches!(prepare_replacement(&input, &query, "b", &AtomicBool::new(true), ReplaceLimits::default()), Err(ReplaceError::Cancelled)));
    }

    #[test]
    fn guarded_disk_replace_preserves_encoding_permissions_and_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, b"\xEF\xBB\xBFfn main() { let old = 1; }\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        let stamp = FileStamp::from_metadata(&std::fs::metadata(&file).unwrap());
        let query = Query::new(QueryOptions::text("old")).unwrap();
        let (count, bytes) = replace_in_file_guarded(&file, &stamp, &query, "new", &AtomicBool::new(false), ReplaceLimits::default()).unwrap();
        assert_eq!(count, 1);
        let current = std::fs::read(&file).unwrap();
        assert_eq!(bytes, current.len());
        assert_eq!(current, b"\xEF\xBB\xBFfn main() { let new = 1; }\r\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
        }
        assert!(matches!(replace_in_file_guarded(&file, &stamp, &query, "bad", &AtomicBool::new(false), ReplaceLimits::default()), Err(ReplaceError::Changed)));
        assert_eq!(std::fs::read(&file).unwrap(), current);
        let permissions = std::fs::metadata(&file).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&file, readonly).unwrap();
        let readonly_stamp = FileStamp::from_metadata(&std::fs::metadata(&file).unwrap());
        assert!(matches!(
            replace_in_file_guarded(&file, &readonly_stamp, &query, "bad", &AtomicBool::new(false), ReplaceLimits::default()),
            Err(ReplaceError::ReadOnly)
        ));
        assert_eq!(std::fs::read(&file).unwrap(), current);
        std::fs::set_permissions(&file, permissions).unwrap();
    }

    #[test]
    fn edit_limit_and_byte_regex_splitting_utf8_are_refused() {
        let query = Query::new(QueryOptions::text("a")).unwrap();
        assert!(matches!(prepare_replacement("aaa", &query, "b", &AtomicBool::new(false), ReplaceLimits { bytes: 100, edits: 2 }), Err(ReplaceError::Limit)));
        let query = Query::new(QueryOptions { pattern: r"(?-u:\xA9)".into(), regex: true, ..Default::default() }).unwrap();
        assert!(matches!(prepare_replacement("é", &query, "x", &AtomicBool::new(false), ReplaceLimits::default()), Err(ReplaceError::NotUtf8)));
    }

    #[test]
    #[cfg(unix)]
    fn fifo_and_symlink_targets_are_refused_without_opening_them() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("pipe.rs");
        let cpath = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        let stamp = FileStamp::from_metadata(&std::fs::symlink_metadata(&pipe).unwrap());
        let query = Query::new(QueryOptions::text("old")).unwrap();
        assert!(matches!(
            replace_in_file_guarded(&pipe, &stamp, &query, "new", &AtomicBool::new(false), ReplaceLimits::default()),
            Err(ReplaceError::NotRegular)
        ));
        let source = dir.path().join("real.rs");
        std::fs::write(&source, "old").unwrap();
        let link = dir.path().join("link.rs");
        symlink(&source, &link).unwrap();
        let stamp = FileStamp::from_metadata(&std::fs::metadata(&source).unwrap());
        assert!(matches!(
            replace_in_file_guarded(&link, &stamp, &query, "new", &AtomicBool::new(false), ReplaceLimits::default()),
            Err(ReplaceError::NotRegular)
        ));
        assert_eq!(std::fs::read_to_string(source).unwrap(), "old");
    }
}
