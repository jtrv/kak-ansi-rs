//! kak-ansi-filter: strip ANSI escapes from stdin to stdout, emit Kakoune
//! range-specs for the colors on stderr (which the plugin `source`s).
//!
//! stderr is a *sourced command file* — nothing but the one protocol line
//! (or a `fail` command) may ever land there, including panic messages.

use std::io::{Read, Write};

mod face;
mod filter;

use filter::{Coord, Filter};

/// Print a Kakoune `fail` command on stderr and exit 1. The message is
/// emitted as a Kakoune *single-quoted* string (embedded `'` doubled):
/// unlike double quotes, single quotes suppress %-expansion, so arbitrary
/// argv text (e.g. `%sh{...}`) stays inert in the sourced command file.
fn die(msg: &str) -> ! {
    // writeln! (not eprintln!) so a failing stderr can't panic on the way out
    let _ = writeln!(std::io::stderr(), "fail 'kak-ansi-filter: {}'", msg.replace('\'', "''"));
    std::process::exit(1);
}

/// Parse `L.C,L.C` (the reference's `%d.%d,%d.%d`).
fn parse_range(s: &str) -> Option<(Coord, Coord)> {
    let coord = |c: &str| -> Option<Coord> {
        let (line, column) = c.split_once('.')?;
        Some(Coord { line: line.parse().ok()?, column: column.parse().ok()? })
    };
    let (a, b) = s.split_once(',')?;
    Some((coord(a)?, coord(b)?))
}

fn main() {
    // A default panic message on stderr would be executed as Kakoune
    // commands; emit a well-formed `fail` instead.
    std::panic::set_hook(Box::new(|_| {
        // ignore stderr write failure: eprintln! would panic-in-panic (abort)
        let _ = writeln!(std::io::stderr(), "fail 'kak-ansi-filter: internal error'");
    }));

    let mut start = Coord { line: 1, column: 1 };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-range" {
            let Some(value) = args.next() else {
                die("-range needs an argument");
            };
            let Some((a, b)) = parse_range(&value) else {
                die("invalid value for -range");
            };
            // normalized: the start coordinate is the smaller one; the end
            // coordinate is otherwise unused (reference parity)
            start = a.min(b);
        } else {
            die(&format!("invalid argument '{arg}'"));
        }
    }

    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        die("error reading stdin");
    }

    let mut filter = Filter::new(start);
    filter.feed_bytes(&input);
    let (out, ranges) = filter.finish();

    // EPIPE must not panic (Rust ignores SIGPIPE); just stop writing.
    let _ = std::io::stdout().write_all(out.as_bytes());
    // One shot, exactly one trailing newline, printed even with zero ranges.
    let _ = std::io::stderr()
        .write_all(format!("set-option -add buffer ansi_color_ranges{ranges}\n").as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_valid() {
        assert_eq!(
            parse_range("8.3,10.2"),
            Some((Coord { line: 8, column: 3 }, Coord { line: 10, column: 2 }))
        );
        assert_eq!(
            parse_range("1.1,1.1"),
            Some((Coord { line: 1, column: 1 }, Coord { line: 1, column: 1 }))
        );
    }

    #[test]
    fn parse_range_invalid() {
        assert_eq!(parse_range(""), None);
        assert_eq!(parse_range("1.1"), None);
        assert_eq!(parse_range("1,1"), None);
        assert_eq!(parse_range("a.b,c.d"), None);
        assert_eq!(parse_range("1.1,2.x"), None);
    }

    #[test]
    fn coord_ordering_is_line_then_column() {
        let a = Coord { line: 10, column: 2 };
        let b = Coord { line: 8, column: 3 };
        assert_eq!(a.min(b), b);
        let c = Coord { line: 8, column: 9 };
        assert_eq!(b.min(c), b);
    }
}
