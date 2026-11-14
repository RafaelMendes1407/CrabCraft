//! The debugger's Memory view: raw memory of the paused program as a hex dump, read one page
//! at a time with `readMemory`. It opens on a variable (its memory reference, or the address
//! of its expression), an address or an expression typed in the view; every new stop reads
//! the page again and the bytes that changed since the previous stop are highlighted.
//!
//! Requests go through the session and their answers arrive as events: nothing here blocks.

use rc_dap::memory::{MemoryBlock, address_in, parse_address};
use rc_dap::{DebugEvent, DebugSession};
use rc_kit::i18n::{tr, trf};

/// Bytes read and shown at a time.
pub const PAGE: usize = 512;

#[derive(Default)]
pub struct MemoryView {
    /// The address or expression field.
    pub input: String,
    /// The memory reference shown and the offset of the page from it.
    pub reference: Option<String>,
    pub offset: i64,
    /// The bytes of the page, once read.
    pub block: Option<MemoryBlock>,
    /// The page as it was at the previous stop: changed bytes are highlighted against it.
    pub previous: Option<MemoryBlock>,
    pub error: Option<String>,
    /// The `readMemory` waiting for its answer.
    read: Option<u64>,
    /// The expression evaluated for its address: (request id, expression).
    eval: Option<(u64, String)>,
}

impl MemoryView {
    /// Forgets what a previous session showed (the field keeps its text).
    pub fn reset(&mut self) {
        let input = std::mem::take(&mut self.input);
        *self = MemoryView { input, ..MemoryView::default() };
    }

    pub fn loading(&self) -> bool {
        self.read.is_some() || self.eval.is_some()
    }

    /// Shows the memory at `reference`, from its start.
    pub fn open_reference(&mut self, session: &mut DebugSession, reference: String) {
        self.input = reference.clone();
        self.reference = Some(reference);
        self.offset = 0;
        self.block = None;
        self.previous = None;
        self.eval = None;
        self.read_page(session);
    }

    /// Shows the memory at an address (`0x7ffd…`) or at the value of an expression (`&x`).
    pub fn go(&mut self, session: &mut DebugSession, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(address) = parse_address(text) {
            self.open_reference(session, format!("{address:#x}"));
            return;
        }
        self.input = text.to_string();
        self.error = None;
        let context = session.expression_context();
        let id = session.evaluate(text, context);
        self.eval = Some((id, text.to_string()));
        self.read = None;
    }

    /// Shows the memory of a variable: its memory reference (GDB gives one for pointers),
    /// otherwise the address of its expression.
    pub fn open_variable(&mut self, session: &mut DebugSession, memory_reference: Option<&str>, expression: &str) {
        match memory_reference.filter(|r| !r.trim().is_empty()) {
            Some(r) => self.open_reference(session, r.to_string()),
            None => self.go(session, &format!("&{expression}")),
        }
    }

    /// Moves `pages` pages forward (or back when negative) and reads them.
    pub fn page(&mut self, session: &mut DebugSession, pages: i64) {
        if self.reference.is_none() {
            return;
        }
        self.offset = self.offset.saturating_add(pages.saturating_mul(PAGE as i64));
        self.read_page(session);
    }

    /// The program stopped again: keeps the bytes shown to compare and reads the page again.
    pub fn on_stopped(&mut self, session: &mut DebugSession) {
        if self.reference.is_none() {
            return;
        }
        if let Some(b) = self.block.take() {
            self.previous = Some(b);
        }
        self.read_page(session);
    }

    /// The address of the first byte of the page, when the reference is an address.
    fn page_address(&self) -> Option<u64> {
        parse_address(self.reference.as_deref()?)?.checked_add_signed(self.offset)
    }

    fn read_page(&mut self, session: &mut DebugSession) {
        let Some(reference) = self.reference.clone() else { return };
        self.error = None;
        if !session.state().is_stopped() {
            self.error = Some(tr("Memory can be read when the program is paused").into());
            return;
        }
        if !session.capabilities().supports_read_memory_request {
            self.error = Some(tr("This debugger cannot read memory").into());
            return;
        }
        self.read = Some(session.read_memory(&reference, self.offset, PAGE));
    }

    /// Takes the answers meant for the view.
    pub fn on_event(&mut self, event: &DebugEvent, session: &mut DebugSession) {
        match event {
            DebugEvent::MemoryRead { id, result } if self.read == Some(*id) => {
                self.read = None;
                match result {
                    Ok(block) => self.block = Some(block.clone()),
                    Err(e) => {
                        self.block = self.page_address().map(|a| MemoryBlock::unreadable(a, PAGE));
                        self.error = Some(trf("Cannot read memory: {error}", &[("error", e)]));
                    }
                }
            }
            DebugEvent::Evaluated { id, result, .. } if self.eval.as_ref().is_some_and(|(e, _)| e == id) => {
                let expression = self.eval.take().map(|(_, e)| e).unwrap_or_default();
                match result {
                    Ok(v) => {
                        let reference =
                            v.memory_reference.clone().filter(|r| !r.trim().is_empty()).or_else(|| address_in(&v.result).map(|a| format!("{a:#x}")));
                        match reference {
                            Some(r) => {
                                self.open_reference(session, r);
                                self.input = expression;
                            }
                            None => self.error = Some(trf("{expression} has no address. Try &{expression}.", &[("expression", &expression)])),
                        }
                    }
                    Err(e) => self.error = Some(trf("Cannot evaluate {expression}: {error}", &[("expression", &expression), ("error", e)])),
                }
            }
            _ => {}
        }
    }
}
