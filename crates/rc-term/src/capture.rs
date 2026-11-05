//! Opt-in bounded raw output journal for protocol consumers; never retained for shells.
use std::collections::VecDeque;

pub const OUTPUT_LIMIT: usize = 128 * 1024;

#[derive(Default)]
pub(crate) struct Capture { end: u64, bytes: VecDeque<u8> }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputGap;
impl std::fmt::Display for OutputGap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("terminal output was discarded") }
}
impl std::error::Error for OutputGap {}

impl Capture {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.end = self.end.saturating_add(bytes.len() as u64);
        let discard = (self.bytes.len() + bytes.len()).saturating_sub(OUTPUT_LIMIT);
        self.bytes.drain(..discard.min(self.bytes.len()));
        self.bytes.extend(bytes.iter().rev().take(OUTPUT_LIMIT).rev());
    }
    pub(crate) fn end(&self) -> u64 { self.end }
    pub(crate) fn since(&self, cursor: u64) -> Result<Vec<u8>, OutputGap> {
        let start = self.end.saturating_sub(self.bytes.len() as u64);
        if cursor < start || cursor > self.end { return Err(OutputGap); }
        Ok(self.bytes.iter().skip((cursor - start) as usize).copied().collect())
    }
    pub(crate) fn tail(&self) -> Vec<u8> { self.bytes.iter().copied().collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_reports_gaps_instead_of_returning_a_partial_reply() {
        let mut capture = Capture::default(); capture.push(b"old"); let at = capture.end();
        capture.push(b"reply"); assert_eq!(capture.since(at).unwrap(), b"reply");
        capture.push(&vec![b'x'; OUTPUT_LIMIT + 1]);
        assert_eq!(capture.tail().len(), OUTPUT_LIMIT);
        assert_eq!(capture.since(at), Err(OutputGap));
    }
}
