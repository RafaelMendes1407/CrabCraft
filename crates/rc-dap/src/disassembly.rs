//! Machine code around an address with the DAP `disassemble` request: decoding the adapter's
//! answer, merging the pieces loaded above and below, and laying it out as rows (a header when
//! the function or the source line changes, then one row per instruction).
//!
//! The adapter's answer is untrusted: addresses are checked, texts are cut and cleaned of
//! control characters, and instructions that cannot be placed are dropped.
//!
//! GDB 15 cannot disassemble backwards (a negative `instructionOffset` returns the wrong
//! instructions), so the instructions above an address are read from a few bytes before it and
//! kept only when the decoding lands exactly on that address ([`instructions_before`]).

use crate::memory::parse_address;
use crate::protocol::DisassembleResponse;

/// Instructions asked at a time.
pub const CHUNK: usize = 40;

/// The most instructions asked in one `disassemble`.
pub const MAX_COUNT: usize = 1000;

/// Bytes before an address read to find the instructions above it (x86-64 instructions are at
/// most 15 bytes long, so this holds at least [`CHUNK`] / 3 of them).
pub const BACK_BYTES: i64 = 160;

/// Instructions decoded first from an arbitrary byte that are dropped (the decoding may start
/// in the middle of an instruction; it falls in step after a few).
const RESYNC: usize = 3;

/// The longest text kept from the adapter for one field.
const MAX_TEXT: usize = 300;

/// One decoded instruction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instruction {
    pub address: u64,
    /// The bytes in hex, separated by spaces (`89 4c 24 0c`), when the adapter gives them.
    pub bytes: Option<String>,
    /// Mnemonic and operands.
    pub text: String,
    /// The function or label that starts here.
    pub symbol: Option<String>,
    /// The source file (as the adapter names it) and 1-based line the instruction belongs to.
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// Why an answer of `disassemble` could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisassemblyError {
    /// No instruction of the answer has a numeric address.
    NoInstructions,
}

impl std::fmt::Display for DisassemblyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisassemblyError::NoInstructions => write!(f, "the debugger answered no instructions"),
        }
    }
}

/// Text from the adapter on one line: control characters become spaces, long texts are cut.
fn clean(text: &str) -> String {
    let mut out: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(MAX_TEXT).collect();
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out
}

/// `894c240c` → `89 4c 24 0c`; anything that is not an even run of hex digits is dropped.
pub fn format_bytes(hex: &str) -> Option<String> {
    let hex: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if hex.is_empty() || !hex.len().is_multiple_of(2) || hex.len() > 2 * 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let pairs: Vec<&str> = (0..hex.len()).step_by(2).filter_map(|i| hex.get(i..i + 2)).collect();
    Some(pairs.join(" ").to_ascii_lowercase())
}

/// The instructions of a `disassemble` answer in address order, without repeated addresses.
///
/// Instructions whose address is not a number are dropped (the spec lets adapters pad the
/// answer with invalid ones past readable memory); an answer with none left is an error.
pub fn decode(reply: &DisassembleResponse) -> Result<Vec<Instruction>, DisassemblyError> {
    let mut out: Vec<Instruction> = reply
        .instructions
        .iter()
        .take(MAX_COUNT)
        .filter_map(|i| {
            let address = parse_address(&i.address)?;
            if i.presentation_hint.as_deref() == Some("invalid") {
                return None;
            }
            let file = i
                .location
                .as_ref()
                .and_then(|s| s.path.as_ref().map(|p| p.to_string_lossy().into_owned()).or_else(|| s.name.clone()))
                .map(|f| clean(&f))
                .filter(|f| !f.is_empty());
            Some(Instruction {
                address,
                bytes: i.instruction_bytes.as_deref().and_then(format_bytes),
                text: clean(&i.instruction),
                symbol: i.symbol.as_deref().map(clean).filter(|s| !s.is_empty()),
                file,
                line: i.line.and_then(|l| u32::try_from(l).ok()).filter(|l| *l > 0),
            })
        })
        .collect();
    out.sort_by_key(|i| i.address);
    out.dedup_by_key(|i| i.address);
    if out.is_empty() { Err(DisassemblyError::NoInstructions) } else { Ok(out) }
}

/// The instructions decoded from some bytes before `anchor` that come before it, when the
/// decoding lands exactly on `anchor` (otherwise it is out of step and `None`). The first few
/// are dropped: they may be decoded from the middle of an instruction.
pub fn instructions_before(decoded: &[Instruction], anchor: u64) -> Option<Vec<Instruction>> {
    let at = decoded.iter().position(|i| i.address == anchor)?;
    let before = &decoded[..at];
    Some(before.iter().skip(RESYNC.min(before.len().saturating_sub(1))).cloned().collect())
}

/// `existing` with `more` added, in address order; an address already shown keeps its
/// instruction from `existing` unless `more` replaces it.
pub fn merge(existing: &[Instruction], more: Vec<Instruction>) -> Vec<Instruction> {
    let mut all: Vec<Instruction> = more;
    all.extend(existing.iter().cloned());
    // A stable sort keeps `more` first among equal addresses, and dedup keeps the first.
    all.sort_by_key(|i| i.address);
    all.dedup_by_key(|i| i.address);
    all
}

/// One row of the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisasmRow {
    /// The function and/or source location of the instructions that follow.
    Header { symbol: Option<String>, location: Option<String> },
    /// An instruction; `index` is its place in the instruction list.
    Instruction { index: usize, current: bool },
}

/// `src/main.rs:3`, or the file alone.
pub fn location_text(file: Option<&str>, line: Option<u32>) -> Option<String> {
    match (file, line) {
        (Some(f), Some(l)) => Some(format!("{f}:{l}")),
        (Some(f), None) => Some(f.to_string()),
        (None, Some(l)) => Some(format!(":{l}")),
        (None, None) => None,
    }
}

/// The rows for `instructions`: a header before the first instruction and whenever the symbol
/// or the source location changes; the instruction at `current` is marked.
pub fn rows(instructions: &[Instruction], current: Option<u64>) -> Vec<DisasmRow> {
    let mut out = Vec::with_capacity(instructions.len() + 8);
    let mut last: Option<(Option<&str>, Option<u32>)> = None;
    for (index, i) in instructions.iter().enumerate() {
        let here = (i.file.as_deref(), i.line);
        let moved = last != Some(here);
        if i.symbol.is_some() || moved {
            let location = if moved || last.is_none() { location_text(i.file.as_deref(), i.line) } else { None };
            if i.symbol.is_some() || location.is_some() {
                out.push(DisasmRow::Header { symbol: i.symbol.clone(), location });
            }
        }
        last = Some(here);
        out.push(DisasmRow::Instruction { index, current: current == Some(i.address) });
    }
    out
}

/// `0x0000555555568bcb`: the address column.
pub fn format_address(address: u64) -> String {
    crate::memory::format_address(address)
}

/// A whole instruction as text: address, bytes (padded) and the instruction; `=>` marks the
/// current one.
pub fn instruction_text(i: &Instruction, current: bool) -> String {
    let mark = if current { "=>" } else { "  " };
    format!("{mark} {}  {:<24} {}", format_address(i.address), i.bytes.as_deref().unwrap_or(""), i.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DisassembledInstruction, Source};

    fn insn(address: &str, text: &str, bytes: Option<&str>, line: Option<i64>) -> DisassembledInstruction {
        DisassembledInstruction {
            address: address.into(),
            instruction: text.into(),
            instruction_bytes: bytes.map(Into::into),
            line,
            location: Some(Source { name: Some("main.rs".into()), path: Some("src/main.rs".into()), source_reference: None }),
            ..Default::default()
        }
    }

    fn at(address: u64, line: Option<u32>) -> Instruction {
        Instruction { address, text: format!("nop {address:#x}"), file: line.map(|_| "src/main.rs".into()), line, ..Default::default() }
    }

    #[test]
    fn a_gdb_answer_is_decoded() {
        // As GDB 15.1 answers: bytes without spaces, the source path as the symtab names it.
        let mut first = insn("0x555555568bcb", "mov    %ecx,0xc(%rsp)", Some("894c240c"), Some(2));
        first.symbol = Some("m::add".into());
        let reply = DisassembleResponse { instructions: vec![insn("0x555555568bcf", "mov    %ecx,%eax", Some("89c8"), Some(3)), first] };
        let got = decode(&reply).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0],
            Instruction {
                address: 0x5555_5556_8bcb,
                bytes: Some("89 4c 24 0c".into()),
                text: "mov    %ecx,0xc(%rsp)".into(),
                symbol: Some("m::add".into()),
                file: Some("src/main.rs".into()),
                line: Some(2)
            }
        );
        assert_eq!(got[1].address, 0x5555_5556_8bcf);
        assert_eq!(instruction_text(&got[1], true), "=> 0x0000555555568bcf  89 c8                    mov    %ecx,%eax");
    }

    #[test]
    fn hostile_answers_do_not_panic() {
        assert_eq!(decode(&DisassembleResponse::default()), Err(DisassemblyError::NoInstructions));
        let mut invalid = insn("0x30", "??", None, None);
        invalid.presentation_hint = Some("invalid".into());
        let long = "x".repeat(10_000);
        let reply = DisassembleResponse {
            instructions: vec![
                insn("garbage", "nop", None, None),
                insn("0x20", "jmp\n\u{1b}[31mevil", Some("zz"), Some(-4)),
                insn("0x10", &long, Some("abc"), Some(1 << 40)),
                insn("0x20", "dup", None, None),
                invalid,
            ],
        };
        let got = decode(&reply).unwrap();
        assert_eq!(got.iter().map(|i| i.address).collect::<Vec<_>>(), [0x10, 0x20]);
        assert_eq!(got[0].text.len(), MAX_TEXT);
        assert_eq!((got[0].bytes.clone(), got[0].line), (None, None));
        assert_eq!(got[1].text, "jmp  [31mevil");
        assert_eq!(got[1].line, None);
        assert_eq!(format_bytes(""), None);
        assert_eq!(format_bytes("0F 1F"), Some("0f 1f".into()));
        assert_eq!(format_bytes(&"00".repeat(40)), None);
    }

    #[test]
    fn earlier_instructions_are_kept_only_in_step() {
        let decoded: Vec<Instruction> = [0x100, 0x101, 0x104, 0x108, 0x10a, 0x10c, 0x110].iter().map(|a| at(*a, None)).collect();
        // Lands on 0x10c: the first three of the six before it are dropped.
        let before = instructions_before(&decoded, 0x10c).unwrap();
        assert_eq!(before.iter().map(|i| i.address).collect::<Vec<_>>(), [0x108, 0x10a]);
        // Out of step: 0x10b is in the middle of an instruction.
        assert_eq!(instructions_before(&decoded, 0x10b), None);
        // Nothing before: an empty list; one before: it is kept.
        assert_eq!(instructions_before(&decoded, 0x100), Some(vec![]));
        assert_eq!(instructions_before(&decoded, 0x101).map(|v| v.len()), Some(1));
    }

    #[test]
    fn pieces_merge_in_address_order() {
        let shown = vec![at(0x10, None), at(0x14, None)];
        let mut newer = at(0x14, None);
        newer.text = "newer".into();
        let merged = merge(&shown, vec![at(0x8, None), newer, at(0x18, None)]);
        assert_eq!(merged.iter().map(|i| i.address).collect::<Vec<_>>(), [0x8, 0x10, 0x14, 0x18]);
        assert_eq!(merged[2].text, "newer");
    }

    #[test]
    fn headers_appear_when_the_function_or_line_changes() {
        let mut a = at(0x10, Some(2));
        a.symbol = Some("m::add".into());
        let b = at(0x14, Some(3));
        let c = at(0x16, Some(3));
        let mut d = at(0x20, None);
        d.symbol = Some("m::main".into());
        let e = at(0x24, None);
        let list = vec![a, b, c, d, e];
        let got = rows(&list, Some(0x16));
        let header = |s: Option<&str>, l: Option<&str>| DisasmRow::Header { symbol: s.map(Into::into), location: l.map(Into::into) };
        let row = |index, current| DisasmRow::Instruction { index, current };
        assert_eq!(
            got,
            [
                header(Some("m::add"), Some("src/main.rs:2")),
                row(0, false),
                header(None, Some("src/main.rs:3")),
                row(1, false),
                row(2, true),
                header(Some("m::main"), None),
                row(3, false),
                row(4, false)
            ]
        );
        // No location and no symbol: no header at all.
        assert_eq!(rows(&[at(0x1, None)], None), [row(0, false)]);
        assert!(rows(&[], Some(1)).is_empty());
    }
}
