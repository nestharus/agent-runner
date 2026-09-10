//! Replies to terminal queries belong to the emulated child screen, not the outer TUI.

use super::PendingChildInput;

// Bound queued responses even when a child writes queries without reading stdin.
const MAX_PENDING_REPLY_BYTES: usize = 64 * 1024;

pub(super) struct TerminalParser {
    parser: vt100::Parser<QueryReplies>,
}

impl TerminalParser {
    pub(super) fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(rows, cols, scrollback, QueryReplies::new()),
        }
    }

    pub(super) fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub(super) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub(super) fn screen_mut(&mut self) -> &mut vt100::Screen {
        self.parser.screen_mut()
    }

    pub(super) fn append_replies(&mut self, pending: &mut PendingChildInput) {
        // Share the existing FIFO so partial writes cannot splice a response into
        // a keyboard escape sequence or an in-flight bracketed paste (or vice versa).
        if pending.pending_len() < MAX_PENDING_REPLY_BYTES
            && !self.parser.callbacks().pending.is_empty()
        {
            pending.enqueue(&self.parser.callbacks_mut().pending.take_pending());
        }
    }
}

struct QueryReplies {
    pending: PendingChildInput,
}

impl QueryReplies {
    fn new() -> Self {
        Self {
            pending: PendingChildInput::new(),
        }
    }

    fn enqueue(&mut self, reply: &[u8]) {
        if self.pending.pending_len() + reply.len() <= MAX_PENDING_REPLY_BYTES {
            self.pending.enqueue(reply);
        }
    }
}

impl vt100::Callbacks for QueryReplies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        params: &[&[u16]],
        final_byte: char,
    ) {
        if second.is_some() || params.len() > 1 || params.first().is_some_and(|p| p.len() != 1) {
            return;
        }
        let parameter = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (first, parameter, final_byte) {
            (None, 5, 'n') => self.enqueue(b"\x1b[0n"),
            (None, 6, 'n') => {
                let (row, col) = screen.cursor_position();
                let (rows, cols) = screen.size();
                // vt100 represents pending wrap with col == cols; the terminal
                // cursor is still on its last physical cell until the next print.
                let row = row.min(rows.saturating_sub(1));
                let col = col.min(cols.saturating_sub(1));
                self.enqueue(
                    format!("\x1b[{};{}R", u32::from(row) + 1, u32::from(col) + 1).as_bytes(),
                );
            }
            // Advertise only the basic emulated terminal, never the outer terminal's
            // capabilities (its cursor and keyboard modes belong to the runner).
            (None, 0, 'c') => self.enqueue(b"\x1b[?1;2c"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take_replies(parser: &mut TerminalParser) -> Vec<u8> {
        parser.parser.callbacks_mut().pending.take_pending()
    }

    #[test]
    fn cursor_queries_use_child_position_even_with_scrollback_selected() {
        let mut parser = TerminalParser::new(4, 20, 20);
        parser.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\x1b[3;7H");
        parser.screen_mut().set_scrollback(1);
        parser.process(b"\x1b[6n\x1b[2;9H\x1b[6n");
        assert_eq!(take_replies(&mut parser), b"\x1b[3;7R\x1b[2;9R");
    }

    #[test]
    fn query_responses_survive_every_read_boundary() {
        let bytes = b"\x1b[8;12H\x1b[6n\x1b[5n\x1b[c";
        for split in 0..=bytes.len() {
            let mut parser = TerminalParser::new(12, 80, 0);
            parser.process(&bytes[..split]);
            parser.process(&bytes[split..]);
            assert_eq!(
                take_replies(&mut parser),
                b"\x1b[8;12R\x1b[0n\x1b[?1;2c",
                "split={split}"
            );
        }
    }

    #[test]
    fn cursor_reply_stays_on_last_cell_during_pending_wrap() {
        let mut parser = TerminalParser::new(4, 5, 0);
        parser.process(b"abcde\x1b[6n");
        assert_eq!(take_replies(&mut parser), b"\x1b[1;5R");
        parser.process(b"f\x1b[6n");
        assert_eq!(take_replies(&mut parser), b"\x1b[2;2R");
    }

    #[test]
    fn responses_preserve_partial_paste_and_subsequent_input_order() {
        let mut pending = PendingChildInput::new();
        pending.enqueue(b"\x1b[200~paste body\x1b[201~");
        pending.drained = 10; // Model a short nonblocking write inside the paste.
        let mut parser = TerminalParser::new(4, 20, 0);
        parser.process(b"\x1b[6n");
        parser.append_replies(&mut pending);
        pending.enqueue(b"next key");
        assert_eq!(pending.take_pending(), b"e body\x1b[201~\x1b[1;1Rnext key");
        assert!(take_replies(&mut parser).is_empty());
    }

    #[test]
    fn blocked_child_does_not_grow_reply_queues_on_repeated_harvests() {
        let mut parser = TerminalParser::new(4, 20, 0);
        let mut pending = PendingChildInput::new();
        for _ in 0..MAX_PENDING_REPLY_BYTES {
            parser.process(b"\x1b[6n");
            parser.append_replies(&mut pending);
        }
        // One bounded response batch can cross the FIFO threshold; further
        // replies stay in the separately bounded parser until the FIFO drains.
        assert!(pending.pending_len() <= 2 * MAX_PENDING_REPLY_BYTES);
        assert!(take_replies(&mut parser).len() <= MAX_PENDING_REPLY_BYTES);
    }

    #[test]
    fn partial_reply_writes_reclaim_consumed_storage_while_queue_stays_nonempty() {
        let mut parser = TerminalParser::new(4, 20, 0);
        let mut pending = PendingChildInput::new();
        pending.enqueue(b"\x1b[1;1R");
        for _ in 0..MAX_PENDING_REPLY_BYTES {
            parser.process(b"\x1b[6n");
            parser.append_replies(&mut pending);
            pending.drained += 6; // One of the two queued reports was written.
            assert_eq!(pending.pending_len(), 6);
            assert!(pending.bytes.len() <= super::super::RELAY_BUFFER_BYTES + 12);
        }
        assert_eq!(pending.take_pending(), b"\x1b[1;1R");
    }

    #[test]
    fn reply_queue_is_bounded_and_does_not_answer_response_packets() {
        let mut parser = TerminalParser::new(12, 80, 0);
        parser.process(b"\x1b[1;1R\x1b[?1;2c\x1b[?0u\x1b[10;6n");
        assert!(take_replies(&mut parser).is_empty());
        for _ in 0..MAX_PENDING_REPLY_BYTES {
            parser.process(b"\x1b[6n");
        }
        assert!(!parser.parser.callbacks().pending.is_empty());
        assert!(take_replies(&mut parser).len() <= MAX_PENDING_REPLY_BYTES);
        assert!(parser.parser.callbacks().pending.is_empty());
    }
}
