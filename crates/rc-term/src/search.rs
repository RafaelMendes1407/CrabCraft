//! Cancellable literal scrollback search. A worker locks one row at a time, never the
//! complete history; output generations prevent selecting stale coordinates.
use crate::session::{EventProxy, TermState};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line},
};
use parking_lot::Mutex;
use rc_core::{Inbox, Waker};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const MAX_CELLS: usize = 2_000_000;
const MAX_HITS: usize = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: i32,
    pub start_column: usize,
    pub end_column: usize,
}
#[derive(Debug)]
pub struct Results {
    pub generation: u64,
    pub hits: Vec<Match>,
    pub truncated: bool,
    pub stale: bool,
}

pub struct Search {
    inbox: Inbox<Results>,
    cancel: Arc<AtomicBool>,
}
impl Search {
    pub(crate) fn start(state: Arc<Mutex<TermState>>, query: String, waker: Waker) -> std::io::Result<Self> {
        let inbox = Inbox::new(waker);
        let out = inbox.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        std::thread::Builder::new().name("rc-terminal-search".into()).spawn(move || {
            let result = scan(state, &query, &token);
            if !token.load(Ordering::Relaxed) {
                out.send(result);
            }
        })?;
        Ok(Self { inbox, cancel })
    }
    pub fn poll(&self) -> Option<Results> {
        self.inbox.drain().into_iter().next()
    }
}
impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn scan(state: Arc<Mutex<TermState>>, query: &str, cancel: &AtomicBool) -> Results {
    let (generation, first, last, columns) = {
        let state = state.lock();
        let grid = state.term.grid();
        (state.generation, grid.topmost_line().0, grid.bottommost_line().0, grid.columns())
    };
    let mut result = Results { generation, hits: Vec::new(), truncated: false, stale: false };
    if query.is_empty() || query.chars().count() > 256 {
        return result;
    }
    let max_rows = MAX_CELLS / columns.max(1);
    let first_scanned = first.max(last.saturating_sub(max_rows.saturating_sub(1).min(i32::MAX as usize) as i32));
    result.truncated = first_scanned != first;
    for row in first_scanned..=last {
        if cancel.load(Ordering::Relaxed) {
            return result;
        }
        let (text, points) = {
            let state = state.lock();
            if state.generation != generation {
                result.stale = true;
                result.hits.clear();
                return result;
            }
            row_text(&state.term, row, columns)
        };
        for (byte, matched) in text.match_indices(query) {
            if result.hits.len() == MAX_HITS {
                result.truncated = true;
                return result;
            }
            let start = points.partition_point(|(offset, _)| *offset <= byte).saturating_sub(1);
            let end = points.partition_point(|(offset, _)| *offset < byte + matched.len()).saturating_sub(1);
            if let (Some((_, start_column)), Some((_, end_column))) = (points.get(start), points.get(end)) {
                result.hits.push(Match { line: row, start_column: *start_column, end_column: *end_column });
            }
        }
    }
    result
}

fn row_text(term: &alacritty_terminal::Term<EventProxy>, line: i32, columns: usize) -> (String, Vec<(usize, usize)>) {
    let mut text = String::new();
    let mut points = Vec::new();
    for column in 0..columns {
        let cell = &term.grid()[Line(line)][Column(column)];
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        points.push((text.len(), column));
        text.push(cell.c);
        if let Some(extra) = cell.zerowidth() {
            for character in extra {
                points.push((text.len(), column));
                text.push(*character);
            }
        }
    }
    (text, points)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TermSession;
    use std::time::{Duration, Instant};
    fn finish(search: &Search) -> Results {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = search.poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "terminal search timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn searches_scrollback_and_selects_unicode_cells_without_stale_coordinates() {
        let session = TermSession::output_only(40, 3, Waker::noop());
        session.feed("first marker\r\nsecond\r\nthird\r\n漢字 marker\r\nlast\r\n".as_bytes());
        let result = finish(&session.search("marker".into(), Waker::noop()).unwrap());
        assert_eq!(result.hits.len(), 2);
        assert!(result.hits[0].line < 0, "scrollback must be included");
        assert!(session.reveal_search_match(result.hits[0], result.generation));
        assert_eq!(session.selection_text().unwrap(), "marker");
        assert!(session.reveal_search_match(result.hits[1], result.generation));
        assert_eq!(session.selection_text().unwrap(), "marker");
        session.feed(b"changed");
        assert!(!session.reveal_search_match(result.hits[0], result.generation));
        let wide = finish(&session.search("漢字".into(), Waker::noop()).unwrap());
        assert_eq!(wide.hits.len(), 1);
        assert!(session.reveal_search_match(wide.hits[0], wide.generation));
        assert_eq!(session.selection_text().unwrap(), "漢字");
    }
    #[test]
    fn bounds_results_and_treats_regex_metacharacters_literally() {
        let session = TermSession::output_only(80, 4, Waker::noop());
        session.feed(b"a.b a*b a.b\r\n");
        let result = finish(&session.search("a.b".into(), Waker::noop()).unwrap());
        assert_eq!(result.hits.len(), 2);
        session.feed("x ".repeat(6_000).as_bytes());
        let result = finish(&session.search("x".into(), Waker::noop()).unwrap());
        assert_eq!(result.hits.len(), MAX_HITS);
        assert!(result.truncated);
        let search = session.search("x".into(), Waker::noop()).unwrap();
        drop(search); // Cancels without waiting or holding the terminal lock.
    }
}
