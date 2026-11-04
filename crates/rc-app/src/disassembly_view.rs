//! The debugger's Disassembly view: the machine code around the selected frame's current
//! instruction, read with `disassemble`. It follows the selected frame, reads again at every
//! stop and loads more instructions above or below on demand.
//!
//! Requests go through the session and their answers arrive as events: nothing here blocks.

use rc_dap::disassembly::{BACK_BYTES, CHUNK, Instruction, instructions_before, merge};
use rc_dap::memory::parse_address;
use rc_dap::{DebugEvent, DebugSession};
use rc_kit::i18n::{tr, trf};

/// Tries, one byte apart, to find the instructions above an address before giving up.
const ABOVE_ATTEMPTS: i64 = 4;

/// A read of the instructions before `anchor`, from `BACK_BYTES - attempt` bytes before it.
struct Above {
    id: u64,
    anchor: u64,
    attempt: i64,
}

#[derive(Default)]
pub struct DisassemblyView {
    /// The instructions shown, in address order.
    pub instructions: Vec<Instruction>,
    /// The selected frame's current instruction.
    pub current: Option<u64>,
    pub error: Option<String>,
    /// Earlier instructions could not be found (the decoding never landed on the first one).
    pub top_unknown: bool,
    /// Scroll so the current instruction is visible (set when the rows around it arrive).
    pub scroll_to_current: bool,
    /// The (stop, frame) shown: another one is read again.
    shown: Option<(u64, i64)>,
    /// The first answer of a new read replaces the instructions shown until then.
    fresh: bool,
    /// The rows around the current instruction are being read (each piece scrolls to it).
    around: bool,
    /// The read from the current instruction (or from the last one, for "load later").
    below: Option<u64>,
    above: Option<Above>,
}

impl DisassemblyView {
    /// Forgets what a previous session showed.
    pub fn reset(&mut self) {
        *self = DisassemblyView::default();
    }

    pub fn loading(&self) -> bool {
        self.below.is_some() || self.above.is_some()
    }

    /// Reads the code around the selected frame when it is not the one shown (another frame
    /// was selected, or the program stopped again). Called while the view is visible.
    pub fn sync(&mut self, session: &mut DebugSession) {
        if !session.state().is_stopped() {
            return;
        }
        let Some(frame) = session.selected_frame() else { return };
        let key = (session.generation(), frame.id);
        if self.shown == Some(key) {
            return;
        }
        self.shown = Some(key);
        let ip = frame.instruction_pointer_reference.clone();
        self.below = None;
        self.above = None;
        self.top_unknown = false;
        self.error = None;
        if !session.capabilities().supports_disassemble_request {
            self.instructions.clear();
            self.error = Some(tr("This debugger cannot disassemble").into());
            return;
        }
        let Some(address) = ip.as_deref().and_then(parse_address) else {
            self.instructions.clear();
            self.current = None;
            self.error = Some(tr("This frame has no instruction address").into());
            return;
        };
        self.current = Some(address);
        self.fresh = true;
        self.around = true;
        self.below = Some(session.disassemble(&format!("{address:#x}"), 0, 0, CHUNK));
        self.read_above(session, address, 0);
    }

    /// Loads the instructions before the first one shown.
    pub fn load_earlier(&mut self, session: &mut DebugSession) {
        if self.above.is_some() {
            return;
        }
        if let Some(first) = self.instructions.first().map(|i| i.address) {
            self.top_unknown = false;
            self.read_above(session, first, 0);
        }
    }

    /// Loads the instructions after the last one shown.
    pub fn load_later(&mut self, session: &mut DebugSession) {
        if self.below.is_some() {
            return;
        }
        if let Some(last) = self.instructions.last().map(|i| i.address) {
            self.below = Some(session.disassemble(&format!("{last:#x}"), 0, 1, CHUNK));
        }
    }

    fn read_above(&mut self, session: &mut DebugSession, anchor: u64, attempt: i64) {
        let id = session.disassemble(&format!("{anchor:#x}"), -(BACK_BYTES - attempt), 0, CHUNK * 2);
        self.above = Some(Above { id, anchor, attempt });
    }

    fn add(&mut self, more: Vec<Instruction>) {
        if std::mem::take(&mut self.fresh) {
            self.instructions.clear();
        }
        self.instructions = merge(&self.instructions, more);
        self.scroll_to_current |= self.around;
    }

    /// Takes the answers meant for the view.
    pub fn on_event(&mut self, event: &DebugEvent, session: &mut DebugSession) {
        let DebugEvent::Disassembled { id, result } = event else { return };
        self.on_answer(*id, result, session);
        if !self.loading() {
            self.around = false;
        }
    }

    fn on_answer(&mut self, id: u64, result: &Result<Vec<Instruction>, String>, session: &mut DebugSession) {
        if self.below == Some(id) {
            self.below = None;
            match result {
                Ok(list) => self.add(list.clone()),
                Err(e) => self.error = Some(trf("Cannot disassemble: {error}", &[("error", e)])),
            }
            return;
        }
        let Some(above) = self.above.take_if(|a| a.id == id) else { return };
        match result.as_ref().ok().and_then(|list| instructions_before(list, above.anchor)) {
            Some(list) => self.add(list),
            None if above.attempt + 1 < ABOVE_ATTEMPTS && session.state().is_stopped() => self.read_above(session, above.anchor, above.attempt + 1),
            None => self.top_unknown = true,
        }
    }
}
