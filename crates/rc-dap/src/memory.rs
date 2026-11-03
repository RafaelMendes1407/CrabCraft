//! Raw memory read with the DAP `readMemory` request: decoding the adapter's answer (base64,
//! partial reads) and laying it out as a hex dump (address, 16 bytes in hex, ASCII).
//!
//! The adapter's answer is untrusted: addresses, lengths and data are checked here and the
//! bytes that were not read are kept as `None` (shown as `??`).

use crate::protocol::ReadMemoryResponse;
use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

/// Bytes on one row of the dump.
pub const BYTES_PER_ROW: usize = 16;

/// The most bytes asked in one `readMemory`.
pub const MAX_READ: usize = 4096;

/// Why an answer of `readMemory` could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryError {
    /// The `address` of the answer is not a number.
    BadAddress(String),
    /// The `data` of the answer is not base64.
    BadData(String),
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryError::BadAddress(a) => write!(f, "the debugger answered an invalid address: {a}"),
            MemoryError::BadData(e) => write!(f, "the debugger answered invalid memory data: {e}"),
        }
    }
}

/// Bytes from `address` on; `None` for the ones that could not be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryBlock {
    pub address: u64,
    pub bytes: Vec<Option<u8>>,
}

/// One row of the dump: its address and up to [`BYTES_PER_ROW`] bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HexRow {
    pub address: u64,
    pub cells: Vec<Option<u8>>,
}

impl MemoryBlock {
    /// `count` bytes from `address`, none of them readable.
    pub fn unreadable(address: u64, count: usize) -> MemoryBlock {
        MemoryBlock { address, bytes: vec![None; count.min(MAX_READ)] }
    }

    /// The byte at `address`: `None` outside the block, `Some(None)` when it was not read.
    pub fn get(&self, address: u64) -> Option<Option<u8>> {
        let offset = usize::try_from(address.checked_sub(self.address)?).ok()?;
        self.bytes.get(offset).copied()
    }

    /// The block cut into rows of [`BYTES_PER_ROW`], starting at its first byte.
    pub fn rows(&self) -> Vec<HexRow> {
        self.bytes
            .chunks(BYTES_PER_ROW)
            .enumerate()
            .map(|(i, cells)| HexRow { address: self.address.wrapping_add((i * BYTES_PER_ROW) as u64), cells: cells.to_vec() })
            .collect()
    }

    /// For each byte, whether `previous` had another value at the same address (bytes that
    /// were not read on either side do not count as changed).
    pub fn changed_since(&self, previous: Option<&MemoryBlock>) -> Vec<bool> {
        (0..self.bytes.len())
            .map(|i| {
                let now = self.bytes[i];
                let Some(prev) = previous else { return false };
                let Some(address) = self.address.checked_add(i as u64) else { return false };
                matches!((prev.get(address), now), (Some(Some(old)), Some(new)) if old != new)
            })
            .collect()
    }
}

/// An address typed by the user or answered by the adapter: `0x7ffd…` in hexadecimal or a
/// decimal number.
pub fn parse_address(text: &str) -> Option<u64> {
    let t = text.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) => u64::from_str_radix(hex, 16).ok(),
        Some(_) => None,
        None if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) => t.parse().ok(),
        None => None,
    }
}

/// The first `0x…` address in a printed value (`(*mut [u8; 4]) 0x7ffd5c3a1b20`).
pub fn address_in(value: &str) -> Option<u64> {
    let start = value.find("0x")?;
    let digits: String = value[start + 2..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    if digits.is_empty() {
        return None;
    }
    u64::from_str_radix(&digits, 16).ok()
}

/// Standard base64, with or without its `=` padding.
pub fn decode_base64(text: &str) -> Result<Vec<u8>, MemoryError> {
    const ENGINE: GeneralPurpose = GeneralPurpose::new(&base64::alphabet::STANDARD, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent));
    let clean: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    ENGINE.decode(clean).map_err(|e| MemoryError::BadData(e.to_string()))
}

/// The `count` bytes asked from `requested` (the start of the read when the memory reference
/// is an address; otherwise the answer's address is the start) out of a `readMemory` answer.
///
/// The answer's data starts at its `address`, which may be past the requested start when the
/// first bytes could not be read; bytes after the data (`unreadableBytes`) and anything the
/// adapter sent beyond `count` are left out.
pub fn decode_read(requested: Option<u64>, count: usize, reply: &ReadMemoryResponse) -> Result<MemoryBlock, MemoryError> {
    let count = count.min(MAX_READ);
    let base = parse_address(&reply.address).ok_or_else(|| MemoryError::BadAddress(reply.address.clone()))?;
    let data = match reply.data.as_deref() {
        Some(d) => decode_base64(d)?,
        None => Vec::new(),
    };
    let start = requested.unwrap_or(base);
    let bytes = (0..count)
        .map(|i| {
            let address = start.checked_add(i as u64)?;
            let offset = usize::try_from(address.checked_sub(base)?).ok()?;
            data.get(offset).copied()
        })
        .collect();
    Ok(MemoryBlock { address: start, bytes })
}

/// `0x00007ffd5c3a1b20`: the address column.
pub fn format_address(address: u64) -> String {
    format!("{address:#018x}")
}

/// A byte in hex, or `??` when it could not be read.
pub fn format_byte(byte: Option<u8>) -> String {
    match byte {
        Some(b) => format!("{b:02x}"),
        None => "??".into(),
    }
}

/// The ASCII column: printable characters as they are, other bytes as `.`, unread ones as `?`.
pub fn format_ascii(cells: &[Option<u8>]) -> String {
    cells
        .iter()
        .map(|b| match b {
            Some(c @ 0x20..=0x7e) => *c as char,
            Some(_) => '.',
            None => '?',
        })
        .collect()
}

/// A whole row as text: address, the bytes in hex (an extra space after the eighth, padding
/// for a short row) and the ASCII column.
pub fn row_text(row: &HexRow) -> String {
    let mut hex = String::new();
    for i in 0..BYTES_PER_ROW {
        if i == BYTES_PER_ROW / 2 {
            hex.push(' ');
        }
        match row.cells.get(i) {
            Some(b) => hex.push_str(&format_byte(*b)),
            None => hex.push_str("  "),
        }
        if i + 1 < BYTES_PER_ROW {
            hex.push(' ');
        }
    }
    format!("{}  {hex}  {}", format_address(row.address), format_ascii(&row.cells))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(address: &str, data: Option<&str>, unreadable: Option<i64>) -> ReadMemoryResponse {
        ReadMemoryResponse { address: address.into(), data: data.map(Into::into), unreadable_bytes: unreadable }
    }

    #[test]
    fn base64_decodes_with_and_without_padding() {
        assert_eq!(decode_base64("3q2+7w==").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(decode_base64("3q2+7w").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());
        assert_eq!(decode_base64("aGVs\nbG8=").unwrap(), b"hello");
        assert!(matches!(decode_base64("not base64!"), Err(MemoryError::BadData(_))));
    }

    #[test]
    fn a_full_read_fills_the_block() {
        let b = decode_read(Some(0x1000), 4, &reply("0x1000", Some("3q2+7w=="), None)).unwrap();
        assert_eq!(b, MemoryBlock { address: 0x1000, bytes: vec![Some(0xde), Some(0xad), Some(0xbe), Some(0xef)] });
        // Without a numeric reference the answer's address is the start.
        let b = decode_read(None, 2, &reply("4096", Some("3q2+7w=="), None)).unwrap();
        assert_eq!(b, MemoryBlock { address: 0x1000, bytes: vec![Some(0xde), Some(0xad)] });
    }

    #[test]
    fn partial_reads_leave_unread_bytes_empty() {
        // The last bytes are unreadable.
        let b = decode_read(Some(0x1000), 6, &reply("0x1000", Some("3q0="), Some(4))).unwrap();
        assert_eq!(b.bytes, vec![Some(0xde), Some(0xad), None, None, None, None]);
        // The first bytes are unreadable: the data starts later.
        let b = decode_read(Some(0x1000), 4, &reply("0x1002", Some("vu8="), None)).unwrap();
        assert_eq!(b.bytes, vec![None, None, Some(0xbe), Some(0xef)]);
        // Nothing readable at all.
        let b = decode_read(Some(0x1000), 3, &reply("0x1000", None, Some(3))).unwrap();
        assert_eq!(b.bytes, vec![None, None, None]);
        // Data before the requested start or beyond the count is ignored.
        let b = decode_read(Some(0x1001), 2, &reply("0x1000", Some("3q2+7w=="), None)).unwrap();
        assert_eq!(b.bytes, vec![Some(0xad), Some(0xbe)]);
    }

    #[test]
    fn hostile_answers_do_not_panic() {
        assert!(matches!(decode_read(Some(0), 4, &reply("garbage", Some("AA=="), None)), Err(MemoryError::BadAddress(_))));
        assert!(matches!(decode_read(Some(0), 4, &reply("0x0", Some("@@@"), None)), Err(MemoryError::BadData(_))));
        let b = decode_read(Some(u64::MAX - 1), 4, &reply("0xffffffffffffffff", Some("AQID"), Some(-5))).unwrap();
        assert_eq!(b.bytes, vec![None, Some(1), None, None]);
        let b = decode_read(Some(0), usize::MAX, &reply("0x0", Some("AQ=="), None)).unwrap();
        assert_eq!(b.bytes.len(), MAX_READ);
    }

    #[test]
    fn addresses_are_parsed() {
        assert_eq!(parse_address("0x7ffd5c3a1b20"), Some(0x7ffd5c3a1b20));
        assert_eq!(parse_address(" 0XFF "), Some(255));
        assert_eq!(parse_address("4096"), Some(4096));
        for bad in ["", "0x", "0xzz", "&x", "-1", "0x10000000000000000"] {
            assert_eq!(parse_address(bad), None, "{bad}");
        }
        assert_eq!(address_in("(*mut [u8; 4]) 0x7ffd5c3a1b20"), Some(0x7ffd5c3a1b20));
        assert_eq!(address_in("0x10 <main>"), Some(16));
        assert_eq!(address_in("42"), None);
        assert_eq!(address_in("0x"), None);
    }

    #[test]
    fn rows_have_sixteen_bytes_hex_and_ascii() {
        let mut bytes: Vec<Option<u8>> = b"Hello, world!\n\0\x7f".iter().map(|b| Some(*b)).collect();
        bytes.extend([Some(0xde), None, Some(b'A')]);
        let block = MemoryBlock { address: 0x7ffd_0000_0010, bytes };
        let rows = block.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].address, 0x7ffd_0000_0020);
        assert_eq!(row_text(&rows[0]), "0x00007ffd00000010  48 65 6c 6c 6f 2c 20 77  6f 72 6c 64 21 0a 00 7f  Hello, world!...");
        assert_eq!(row_text(&rows[1]), "0x00007ffd00000020  de ?? 41                                          .?A");
        assert_eq!(format_byte(None), "??");
        assert!(MemoryBlock::default().rows().is_empty());
    }

    #[test]
    fn changes_are_found_by_address() {
        let before = MemoryBlock { address: 0x100, bytes: vec![Some(1), Some(2), None, Some(4)] };
        let now = MemoryBlock { address: 0x102, bytes: vec![Some(9), Some(5), Some(6)] };
        // 0x102 was unread, 0x103 changed 4 → 5, 0x104 was not in the old block.
        assert_eq!(now.changed_since(Some(&before)), vec![false, true, false]);
        assert_eq!(now.changed_since(None), vec![false, false, false]);
        assert_eq!(before.get(0x103), Some(Some(4)));
        assert_eq!(before.get(0x102), Some(None));
        assert_eq!(before.get(0xff), None);
        assert_eq!(before.get(0x104), None);
        assert_eq!(MemoryBlock::unreadable(0x10, 3).bytes, vec![None, None, None]);
    }
}
