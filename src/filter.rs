//! The stateful core: overstrike delay, escape state machine, DEC
//! line-drawing, coordinate tracking and range emission.
//!
//! Per-char pipeline (same order and one-char delay as the reference):
//! `overstrike -> escape -> translate -> display`. The EOF flush pushes the
//! delayed char through escape/translate/display, skipping overstrike, then
//! emits the final face run.

use crate::face::{parse_codes, Face, BOLD, UNDERLINE};
use std::fmt::Write;

/// 1-based Kakoune buffer coordinate; column is a *byte* offset.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Coord {
    pub line: i32,
    pub column: i32,
}

/// Cap on buffered CSI parameters, like the reference's 1024-char sequence
/// buffer; excess chars are dropped, not printed, and an SGR that hits the
/// cap is dropped entirely (the reference's buffer cannot hold the final
/// `m`, so it never applies truncated params — neither do we). OSC contents
/// are never buffered at all, so ST termination works past any length
/// (fixes the reference bug where an over-long OSC could only end via BEL).
const CSI_PARAM_CAP: usize = 1018;

#[allow(clippy::enum_variant_names)] // `Esc::Esc` is the clearest name here
enum Esc {
    Ground,
    /// Saw ESC, waiting for the intro char.
    Esc,
    /// `ESC [`: `params` holds digits/`;`/`:`/`?`; `len` counts consumed
    /// param chars (also past the cap) so `?` is only accepted first.
    Csi { params: String, len: usize },
    /// `ESC ]`: contents ignored; `prev` detects the `ESC \` terminator.
    Osc { prev: char },
    /// `ESC (`: exactly one more char selects the G1 charset.
    Charset,
}

pub struct Filter {
    current_face: Face,
    previous_face: Face,
    cur: Coord,
    prev_end: Coord,
    face_start: Coord,
    g1: bool,
    esc: Esc,
    // overstrike (nroff/man bold & underline) one-char delay
    os_backspace_pending: bool,
    os_char_pending: bool,
    os_want_attr_reset: bool,
    os_saved_attrs: u16,
    os_last: Option<char>,
    out: String,
    ranges: String,
}

impl Filter {
    pub fn new(start: Coord) -> Self {
        Filter {
            current_face: Face::default(),
            previous_face: Face::default(),
            cur: start,
            // start.column - 1 makes "face active but zero chars printed"
            // emit nothing (the reference's (1,0) initial previous_char_end)
            prev_end: Coord { line: start.line, column: start.column - 1 },
            face_start: start,
            g1: false,
            esc: Esc::Ground,
            os_backspace_pending: false,
            os_char_pending: false,
            os_want_attr_reset: false,
            os_saved_attrs: 0,
            os_last: None,
            out: String::new(),
            ranges: String::new(),
        }
    }

    pub fn feed(&mut self, ch: char) {
        if let Some(ch) = self.overstrike(ch) {
            self.feed_past_overstrike(ch);
        }
    }

    /// Decode `bytes` as UTF-8 and feed; an invalid byte or truncated
    /// sequence becomes one U+FFFD and decoding continues (the reference's
    /// `fgetwc` silently truncates the rest of the input instead).
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    s.chars().for_each(|ch| self.feed(ch));
                    return;
                }
                Err(e) => {
                    let (valid, bad) = rest.split_at(e.valid_up_to());
                    // valid_up_to guarantees this is UTF-8
                    std::str::from_utf8(valid)
                        .unwrap()
                        .chars()
                        .for_each(|ch| self.feed(ch));
                    self.feed('\u{FFFD}');
                    match e.error_len() {
                        Some(n) => rest = &bad[n..],
                        None => return, // truncated sequence at EOF
                    }
                }
            }
        }
    }

    pub fn finish(mut self) -> (String, String) {
        if let Some(ch) = self.os_last.take() {
            self.feed_past_overstrike(ch);
        }
        // Improvement over the reference, which emits `current_face` here:
        // a trailing SGR after the last printed char then mislabels the
        // final run (`\e[31mx\e[32m` -> green), silently loses it
        // (`\e[31mx\e[0m` -> nothing) or fabricates a phantom range
        // (`x\e[31m` -> red). Emitting the face of the run actually printed
        // is equivalent whenever no trailing SGR exists and correct when
        // one does.
        let face = self.previous_face;
        self.emit_face(face);
        (self.out, self.ranges)
    }

    fn feed_past_overstrike(&mut self, ch: char) {
        if let Some(ch) = self.escape(ch) {
            let ch = self.translate(ch);
            self.display(ch);
        }
    }

    fn overstrike(&mut self, ch: char) -> Option<char> {
        if self.os_backspace_pending {
            if !self.os_want_attr_reset {
                self.os_want_attr_reset = true;
                self.os_saved_attrs = self.current_face.attrs;
            }
            if self.os_last == Some(ch) {
                self.current_face.attrs |= BOLD;
            }
            if ch == '_' || self.os_last == Some('_') {
                self.current_face.attrs |= UNDERLINE;
            }
            self.os_char_pending = true;
            if ch != '_' {
                self.os_last = Some(ch);
            }
            self.os_backspace_pending = false;
            None
        } else if ch == '\u{8}' {
            self.os_backspace_pending = true;
            None
        } else {
            if self.os_char_pending {
                self.os_char_pending = false;
            } else if self.os_want_attr_reset {
                self.os_want_attr_reset = false;
                self.current_face.attrs = self.os_saved_attrs;
            }
            let released = self.os_last;
            self.os_last = Some(ch);
            released
        }
    }

    fn escape(&mut self, ch: char) -> Option<char> {
        loop {
            match &mut self.esc {
                Esc::Ground => {
                    return match ch {
                        // CR would overdraw in a real terminal; we are a
                        // pager filter, so just delete it
                        '\r' => None,
                        '\u{0E}' => {
                            self.g1 = true;
                            None
                        }
                        '\u{0F}' => {
                            self.g1 = false;
                            None
                        }
                        '\u{1B}' => {
                            self.esc = Esc::Esc;
                            None
                        }
                        _ => Some(ch),
                    };
                }
                Esc::Esc => match ch {
                    '[' => {
                        self.esc = Esc::Csi { params: String::new(), len: 0 };
                        return None;
                    }
                    ']' => {
                        self.esc = Esc::Osc { prev: ']' };
                        return None;
                    }
                    '(' => {
                        self.esc = Esc::Charset;
                        return None;
                    }
                    // Improvement over the reference (which left its state
                    // machine armed, corrupting later plain text): drop the
                    // lone ESC and re-dispatch the follower through ground
                    // state, so `\e\e[31m` still parses SGR, `\e\r` still
                    // deletes the CR, `\e\x0E` still toggles G1.
                    _ => {
                        self.esc = Esc::Ground;
                        continue;
                    }
                },
                Esc::Csi { params, len } => {
                    if (*len == 0 && ch == '?') || ch == ';' || ch == ':' || ch.is_ascii_digit() {
                        if params.len() < CSI_PARAM_CAP {
                            params.push(ch);
                        }
                        *len += 1;
                        return None;
                    }
                    // any other char is the final byte; only `m` (SGR) is
                    // processed, every other final is silently swallowed.
                    // Parity note: the `?` marker does NOT exempt an
                    // m-final sequence from SGR processing.
                    // Parity note: at the cap the reference's buffer cannot
                    // hold the final `m` either, so an oversized SGR is
                    // dropped whole — never applied with truncated params
                    // (which could split a 38;2;R;G;B mid-payload).
                    if ch == 'm' && *len < CSI_PARAM_CAP {
                        let codes = parse_codes(params);
                        self.current_face.apply_sgr(&codes);
                    }
                    self.esc = Esc::Ground;
                    return None;
                }
                Esc::Osc { prev } => {
                    if ch == '\u{7}' || (ch == '\\' && *prev == '\u{1B}') {
                        self.esc = Esc::Ground;
                    } else {
                        *prev = ch;
                    }
                    return None;
                }
                Esc::Charset => {
                    match ch {
                        '0' => self.g1 = true,
                        'B' => self.g1 = false,
                        _ => {}
                    }
                    self.esc = Esc::Ground;
                    return None;
                }
            }
        }
    }

    fn translate(&self, ch: char) -> char {
        if !self.g1 {
            return ch;
        }
        match ch {
            'j' => '┘',
            'k' => '┐',
            'l' => '┌',
            'm' => '└',
            'n' => '┼',
            'q' => '─',
            't' => '├',
            'u' => '┤',
            'v' => '┴',
            'w' => '┬',
            'x' => '│',
            _ => ch,
        }
    }

    fn display(&mut self, ch: char) {
        self.out.push(ch);
        if self.previous_face != self.current_face {
            let face = self.previous_face;
            self.emit_face(face);
            self.face_start = self.cur;
            self.previous_face = self.current_face;
        }
        let bytes = ch.len_utf8() as i32;
        self.prev_end = Coord { line: self.cur.line, column: self.cur.column + bytes - 1 };
        if ch == '\n' {
            self.cur.line += 1;
            self.cur.column = 1;
        } else {
            self.cur.column += bytes;
        }
    }

    /// Append ` L.C,L.C|face` for the run `face_start..prev_end`, unless the
    /// face is all-default or the run is empty.
    fn emit_face(&mut self, face: Face) {
        if face == Face::default() || self.face_start == self.cur {
            return;
        }
        let _ = write!(
            self.ranges,
            " {}.{},{}.{}|{}",
            self.face_start.line,
            self.face_start.column,
            self.prev_end.line,
            self.prev_end.column,
            face
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_at(input: &str, line: i32, column: i32) -> (String, Vec<String>) {
        let mut f = Filter::new(Coord { line, column });
        input.chars().for_each(|ch| f.feed(ch));
        let (out, ranges) = f.finish();
        (out, ranges.split_whitespace().map(String::from).collect())
    }

    fn run(input: &str) -> (String, Vec<String>) {
        run_at(input, 1, 1)
    }

    fn ranges(input: &str) -> Vec<String> {
        run(input).1
    }

    macro_rules! v {
        ($($s:literal),*) => {{
            let r: Vec<String> = vec![$($s.to_string()),*];
            r
        }};
    }

    // ---- ports of ../kak-ansi/tests/tests.bash, one for one ----

    #[test]
    fn removes_ansi_escapes() {
        let (out, r) = run(" \x1b[32m 1.\x1b[39m hello");
        assert_eq!(out, "  1. hello");
        assert_eq!(r, v!["1.2,1.4|green"]);
    }

    #[test]
    fn charset_escape_selects_line_drawing() {
        assert_eq!(run("\x1b(0jklmnqtuvwx").0, "┘┐┌└┼─├┤┴┬│");
    }

    #[test]
    fn charset_escape_resets_line_drawing() {
        assert_eq!(run("\x1b(0\x1b(Bjklmnqtuvwx").0, "jklmnqtuvwx");
    }

    #[test]
    fn ascii_so_selects_line_drawing() {
        assert_eq!(run("\u{0E}jklmnqtuvwx").0, "┘┐┌└┼─├┤┴┬│");
    }

    #[test]
    fn ascii_si_resets_line_drawing() {
        assert_eq!(run("\u{0E}\u{0F}jklmnqtuvwx").0, "jklmnqtuvwx");
    }

    #[test]
    fn emits_face_at_eof() {
        assert_eq!(ranges("\x1b[32mxxx"), v!["1.1,1.3|green"]);
    }

    #[test]
    fn does_not_emit_default_face() {
        assert_eq!(ranges("\x1b[39mxxx"), v![]);
    }

    #[test]
    fn new_face_for_fg_change() {
        assert_eq!(
            ranges("\x1b[32mxxx\x1b[31myyy"),
            v!["1.1,1.3|green", "1.4,1.6|red"]
        );
    }

    #[test]
    fn new_face_for_bg_change() {
        assert_eq!(
            ranges("\x1b[45mxxx\x1b[41myyy"),
            v!["1.1,1.3|default,magenta", "1.4,1.6|default,red"]
        );
    }

    #[test]
    fn merges_ranges_at_bof() {
        assert_eq!(ranges("\x1b[32m\x1b[1mxxx"), v!["1.1,1.3|green+b"]);
    }

    #[test]
    fn merges_ranges() {
        assert_eq!(ranges("y\x1b[32m\x1b[1mxxx"), v!["1.2,1.4|green+b"]);
    }

    #[test]
    fn no_new_face_if_no_change() {
        assert_eq!(ranges("\x1b[31mxxx\x1b[31myyy"), v!["1.1,1.6|red"]);
    }

    #[test]
    fn handles_change_at_2_1() {
        assert_eq!(ranges("xy\n\x1b[31mxxx"), v!["2.1,2.3|red"]);
    }

    #[test]
    fn handles_change_at_eol() {
        assert_eq!(ranges("xy\x1b[31m\nxxx"), v!["1.3,2.3|red"]);
    }

    #[test]
    fn can_specify_range() {
        assert_eq!(run_at("\x1b[32mxxx", 8, 3).1, v!["8.3,8.5|green"]);
    }

    #[test]
    fn advances_using_byte_offsets() {
        assert_eq!(ranges("┘\x1b[32mx"), v!["1.4,1.4|green"]);
    }

    #[test]
    fn covers_ending_char_bytes() {
        assert_eq!(
            ranges("\x1b[31m┘\x1b[32mx"),
            v!["1.1,1.3|red", "1.4,1.4|green"]
        );
    }

    #[test]
    fn adds_ranges_for_fg_colors() {
        assert_eq!(ranges(" \x1b[32m 1."), v!["1.2,1.4|green"]);
    }

    #[test]
    fn sgr_39_resets_fg() {
        assert_eq!(ranges(" \x1b[32m 1.\x1b[39mxx"), v!["1.2,1.4|green"]);
    }

    #[test]
    fn sgr_0_resets_fg() {
        assert_eq!(ranges(" \x1b[32m 1.\x1b[0m hello"), v!["1.2,1.4|green"]);
    }

    #[test]
    fn sgr_empty_resets_fg() {
        assert_eq!(ranges(" \x1b[32m 1.\x1b[m hello"), v!["1.2,1.4|green"]);
    }

    #[test]
    fn truecolor_fg() {
        assert_eq!(ranges("\x1b[38;2;253;17;129mxxx"), v!["1.1,1.3|rgb:FD1181"]);
    }

    #[test]
    fn palette_ansi_fg() {
        assert_eq!(ranges("\x1b[38;5;2mxxx"), v!["1.1,1.3|green"]);
    }

    #[test]
    fn palette_bright_ansi_fg() {
        assert_eq!(ranges("\x1b[38;5;10mxxx"), v!["1.1,1.3|bright-green"]);
    }

    #[test]
    fn palette_cube_fg() {
        assert_eq!(ranges("\x1b[38;5;121mxxx"), v!["1.1,1.3|rgb:87FFAF"]);
    }

    #[test]
    fn palette_greyscale_fg() {
        assert_eq!(ranges("\x1b[38;5;239mxxx"), v!["1.1,1.3|rgb:4E4E4E"]);
    }

    #[test]
    fn adds_ranges_for_bg_colors() {
        assert_eq!(ranges(" \x1b[41m 1."), v!["1.2,1.4|default,red"]);
    }

    #[test]
    fn sgr_49_resets_bg() {
        assert_eq!(ranges(" \x1b[41m 1.\x1b[49mx"), v!["1.2,1.4|default,red"]);
    }

    #[test]
    fn sgr_0_resets_bg() {
        assert_eq!(
            ranges(" \x1b[42m 1.\x1b[0m hello"),
            v!["1.2,1.4|default,green"]
        );
    }

    #[test]
    fn sgr_empty_resets_bg() {
        assert_eq!(
            ranges(" \x1b[42m 1.\x1b[m hello"),
            v!["1.2,1.4|default,green"]
        );
    }

    #[test]
    fn truecolor_bg() {
        assert_eq!(
            ranges("\x1b[48;2;17;129;253mxxx"),
            v!["1.1,1.3|default,rgb:1181FD"]
        );
    }

    #[test]
    fn bold_set() {
        assert_eq!(ranges("x\x1b[1mxx"), v!["1.2,1.3|default+b"]);
    }

    #[test]
    fn bold_reset_21() {
        assert_eq!(ranges("x\x1b[1mx\x1b[21mx"), v!["1.2,1.2|default+b"]);
    }

    #[test]
    fn bold_reset_0() {
        assert_eq!(ranges("x\x1b[1mx\x1b[0mx"), v!["1.2,1.2|default+b"]);
    }

    #[test]
    fn bold_reset_22() {
        assert_eq!(ranges("x\x1b[1mx\x1b[22mx"), v!["1.2,1.2|default+b"]);
    }

    #[test]
    fn dim_set() {
        assert_eq!(ranges("\x1b[2mxxx"), v!["1.1,1.3|default+d"]);
    }

    #[test]
    fn dim_reset_0() {
        assert_eq!(ranges("\x1b[2mx\x1b[0mx"), v!["1.1,1.1|default+d"]);
    }

    #[test]
    fn dim_reset_22() {
        assert_eq!(ranges("\x1b[2mxx\x1b[22mx"), v!["1.1,1.2|default+d"]);
    }

    #[test]
    fn italic_set() {
        assert_eq!(ranges("\x1b[3mxxx"), v!["1.1,1.3|default+i"]);
    }

    #[test]
    fn italic_reset_0() {
        assert_eq!(ranges("\x1b[3mx\x1b[0mxx"), v!["1.1,1.1|default+i"]);
    }

    #[test]
    fn italic_reset_23() {
        assert_eq!(ranges("\x1b[3mx\x1b[23mxx"), v!["1.1,1.1|default+i"]);
    }

    #[test]
    fn underline_set() {
        assert_eq!(ranges("\x1b[4mxxx"), v!["1.1,1.3|default+u"]);
    }

    #[test]
    fn underline_reset_0() {
        assert_eq!(ranges("\x1b[4mx\x1b[0mx"), v!["1.1,1.1|default+u"]);
    }

    #[test]
    fn underline_reset_24() {
        assert_eq!(ranges("\x1b[4mx\x1b[24mx"), v!["1.1,1.1|default+u"]);
    }

    #[test]
    fn blink_set() {
        assert_eq!(ranges("\x1b[5mxxx"), v!["1.1,1.3|default+B"]);
    }

    #[test]
    fn blink_reset_0() {
        assert_eq!(ranges("\x1b[5mx\x1b[0mx"), v!["1.1,1.1|default+B"]);
    }

    #[test]
    fn blink_reset_25() {
        assert_eq!(ranges("\x1b[5mx\x1b[25mx"), v!["1.1,1.1|default+B"]);
    }

    #[test]
    fn reverse_set() {
        assert_eq!(ranges("\x1b[7mxxx"), v!["1.1,1.3|default+r"]);
    }

    #[test]
    fn reverse_reset_0() {
        assert_eq!(ranges("\x1b[7mx\x1b[0mx"), v!["1.1,1.1|default+r"]);
    }

    #[test]
    fn reverse_reset_27() {
        assert_eq!(ranges("\x1b[7mx\x1b[27mx"), v!["1.1,1.1|default+r"]);
    }

    #[test]
    fn overstrike_removes_backspaces() {
        let (out, r) = run(" H\u{8}He\u{8}el\u{8}ll\u{8}lo\u{8}o ");
        assert_eq!(out, " Hello ");
        // bold sticks through the EOF-flushed trailing space, as in the
        // reference (the attribute restore needs one more plain char)
        assert_eq!(r, v!["1.2,1.7|default+b"]);
    }

    #[test]
    fn overstrike_sets_and_resets_boldface() {
        let (out, r) = run(" H\u{8}He\u{8}ello");
        assert_eq!(out, " Hello");
        assert_eq!(r, v!["1.2,1.3|default+b"]);
    }

    #[test]
    fn overstrike_removes_underscore_backspace() {
        assert_eq!(run(" _\u{8}H_\u{8}e_\u{8}l_\u{8}l_\u{8}o ").0, " Hello ");
    }

    #[test]
    fn overstrike_removes_backspace_underscore() {
        assert_eq!(run(" H\u{8}_e\u{8}_l\u{8}_l\u{8}_o\u{8}_ ").0, " Hello ");
    }

    #[test]
    fn overstrike_sets_and_resets_underline() {
        let (out, r) = run(" H\u{8}__\u{8}el\u{8}_lo ");
        assert_eq!(out, " Hello ");
        assert_eq!(r, v!["1.2,1.4|default+u"]);
    }

    #[test]
    fn combined_overstrike() {
        let (out, r) = run(" H\u{8}H\u{8}__\u{8}e\u{8}el\u{8}_\u{8}llo ");
        assert_eq!(out, " Hello ");
        assert_eq!(r, v!["1.2,1.4|default+ub"]);
    }

    #[test]
    fn other_overstrikes_are_discarded() {
        let (out, r) = run("X\u{8}Y");
        assert_eq!(out, "Y");
        assert_eq!(r, v![]);
    }

    #[test]
    fn ignores_hyperlinks() {
        assert_eq!(
            run("editor: \x1b]8;;https://kakoune.org/\x1b\\kakoune\x1b]8;;\x1b\\").0,
            "editor: kakoune"
        );
    }

    #[test]
    fn ignores_hyperlinks_terminated_by_bel() {
        assert_eq!(
            run("editor: \x1b]8;;https://kakoune.org/\u{7}kakoune\x1b]8;;\u{7}").0,
            "editor: kakoune"
        );
    }

    #[test]
    fn ignores_shell_integration_escapes() {
        assert_eq!(run("hello\x1b]133;A\x1b\\world").0, "helloworld");
    }

    #[test]
    fn ignores_common_private_modes() {
        assert_eq!(run("hello\x1b[?47hworld").0, "helloworld");
    }

    // ---- parity quirks the reference suite does not cover ----

    #[test]
    fn question_marked_sgr_is_processed() {
        // the ? marker does not exempt an m-final CSI from SGR handling
        let (out, r) = run("\x1b[?4mx");
        assert_eq!(out, "x");
        assert_eq!(r, v!["1.1,1.1|default+u"]);
    }

    #[test]
    fn question_marked_empty_sgr_resets() {
        let (out, r) = run("\x1b[31mx\x1b[?my");
        assert_eq!(out, "xy");
        assert_eq!(r, v!["1.1,1.1|red"]);
    }

    #[test]
    fn question_mark_only_accepted_first() {
        // `?` in the middle of params is a final byte (not `m`): swallowed
        assert_eq!(run("\x1b[4;?mx").0, "mx");
        assert_eq!(ranges("\x1b[4;?mx"), v![]);
    }

    #[test]
    fn literal_backspace_can_survive() {
        // a\b\bcd: the second \b becomes the overstruck buffered char and
        // is later emitted — reference parity
        let (out, r) = run("a\u{8}\u{8}cd");
        assert_eq!(out, "\u{8}cd");
        assert_eq!(r, v![]);
    }

    #[test]
    fn empty_input() {
        assert_eq!(run(""), (String::new(), v![]));
    }

    #[test]
    fn cr_is_deleted() {
        assert_eq!(run("abc\rdef\r\n").0, "abcdef\n");
    }

    #[test]
    fn cr_inside_csi_acts_as_final_byte() {
        assert_eq!(run("\x1b[31\rx").0, "x");
        assert_eq!(ranges("\x1b[31\rx"), v![]);
    }

    #[test]
    fn non_m_finals_are_swallowed() {
        assert_eq!(run("a\x1b[2J\x1b[1;1Hb\x1b[0Kc").0, "abc");
    }

    #[test]
    fn colon_separated_truecolor() {
        assert_eq!(ranges("\x1b[38:2:253:17:129mx"), v!["1.1,1.1|rgb:FD1181"]);
    }

    #[test]
    fn unterminated_osc_at_eof_prints_nothing() {
        assert_eq!(run("ok\x1b]0;title never ends").0, "ok");
    }

    // ---- EOF trailing-SGR fixes (deliberate divergence from the C, which
    // emits current_face at EOF; see finish()) ----

    #[test]
    fn trailing_sgr_keeps_final_run_face() {
        // C relabels the run with the never-printed face: 1.1,1.3|green
        assert_eq!(ranges("\x1b[32mxxx\x1b[31m"), v!["1.1,1.3|green"]);
        assert_eq!(ranges("\x1b[31mx\x1b[32m"), v!["1.1,1.1|red"]);
    }

    #[test]
    fn trailing_reset_does_not_lose_final_run() {
        // C loses the run entirely (default-face guard on current_face);
        // this is the common fifo-chunk-ends-in-\e[0m case
        assert_eq!(ranges("\x1b[31mx\x1b[0m"), v!["1.1,1.1|red"]);
    }

    #[test]
    fn trailing_sgr_after_unstyled_text_emits_nothing() {
        // C emits a phantom 1.1,1.1|red over never-styled text
        assert_eq!(ranges("x\x1b[31m"), v![]);
    }

    // ---- deliberate improvements over the reference ----

    #[test]
    fn lone_esc_does_not_arm_state_machine() {
        // reference bug: \eA[hello -> "Aello"
        assert_eq!(run("\x1bA[hello").0, "A[hello");
    }

    #[test]
    fn esc_esc_still_parses_sgr() {
        let (out, r) = run("\x1b\x1b[31mx");
        assert_eq!(out, "x");
        assert_eq!(r, v!["1.1,1.1|red"]);
    }

    #[test]
    fn esc_cr_still_deletes_cr() {
        assert_eq!(run("\x1b\rx").0, "x");
    }

    #[test]
    fn esc_so_still_toggles_g1() {
        assert_eq!(run("\x1b\u{0E}j").0, "┘");
    }

    #[test]
    fn rgb_components_clamped() {
        // reference emits invalid rgb:12C0102 here
        assert_eq!(ranges("\x1b[38;2;300;1;2mx"), v!["1.1,1.1|rgb:FF0102"]);
    }

    #[test]
    fn extended_color_leftovers_reprocessed() {
        assert_eq!(ranges("\x1b[38;2;1;31mx"), v!["1.1,1.1|red+b"]);
        assert_eq!(ranges("\x1b[38;6;31mx"), v!["1.1,1.1|red"]);
        assert_eq!(ranges("\x1b[38;5mx"), v![]);
        assert_eq!(ranges("\x1b[38;5;300mx"), v![]);
    }

    #[test]
    fn huge_parameters_do_not_panic() {
        assert_eq!(ranges("\x1b[99999999999mx"), v![]);
        assert_eq!(run("\x1b[99999999999mx").0, "x");
    }

    #[test]
    fn strikethrough() {
        assert_eq!(ranges("\x1b[9mx\x1b[29my"), v!["1.1,1.1|default+s"]);
    }

    #[test]
    fn long_osc_terminated_by_st_past_cap() {
        // reference bug: OSC > 1020 chars can only terminate via BEL
        let input = format!("\x1b]0;{}\x1b\\tail", "a".repeat(2000));
        assert_eq!(run(&input).0, "tail");
    }

    #[test]
    fn oversized_csi_is_capped_not_printed() {
        let input = format!("\x1b[{}31mx", "1;".repeat(2000));
        let (out, r) = run(&input);
        assert_eq!(out, "x"); // dropped, never printed
        assert_eq!(r, v![]); // SGR dropped whole, not applied truncated
    }

    #[test]
    fn csi_at_cap_boundary() {
        // 1015 param chars + "31" = 1017 < cap: still applied
        let ok = format!("\x1b[{}31mx", ";".repeat(1015));
        assert_eq!(ranges(&ok), v!["1.1,1.1|red"]);
        // 1018 param chars fill the cap exactly: the reference's buffer
        // cannot store the final `m`, so the SGR is dropped
        let full = format!("\x1b[{}31mx", ";".repeat(1016));
        assert_eq!(ranges(&full), v![]);
    }

    #[test]
    fn trailing_sgr_with_range_stays_inside_selection() {
        // C bug: face_start/prev_end were never rebased by -range, so this
        // emitted "1.1,8.3|green" spanning the buffer before the selection.
        // The x was printed unstyled, so no range at all (matches
        // trailing_sgr_after_unstyled_text_emits_nothing at origin 1.1).
        assert_eq!(run_at("x\x1b[32m", 8, 3).1, v![]);
    }

    #[test]
    fn lone_sgr_with_range_emits_nothing() {
        // C emitted the invalid range "1.1,1.0|green" here
        assert_eq!(run_at("\x1b[32m", 8, 3).1, v![]);
    }

    #[test]
    fn invalid_utf8_becomes_replacement_char() {
        let mut f = Filter::new(Coord { line: 1, column: 1 });
        f.feed_bytes(b"\xffabc");
        let (out, _) = f.finish();
        assert_eq!(out, "\u{FFFD}abc");
    }

    #[test]
    fn invalid_utf8_does_not_truncate_and_keeps_coords() {
        // U+FFFD is 3 bytes in the output buffer, so red starts at col 4
        let mut f = Filter::new(Coord { line: 1, column: 1 });
        f.feed_bytes(b"\xff\x1b[31mx");
        let (out, ranges) = f.finish();
        assert_eq!(out, "\u{FFFD}x");
        assert_eq!(ranges, " 1.4,1.4|red");
    }

    #[test]
    fn truncated_utf8_at_eof() {
        let mut f = Filter::new(Coord { line: 1, column: 1 });
        f.feed_bytes(b"ab\xe2\x94"); // truncated '┘'
        let (out, _) = f.finish();
        assert_eq!(out, "ab\u{FFFD}");
    }
}
