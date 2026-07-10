//! Integration tests exercising the real process boundary: stderr protocol
//! line shape, -range handling, and `fail` + exit-1 error paths.

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kak-ansi-filter"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kak-ansi-filter");
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

#[test]
fn empty_input_prints_bare_protocol_line() {
    let out = run(&[], b"");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(stderr(&out), "set-option -add buffer ansi_color_ranges\n");
}

#[test]
fn ranges_are_space_separated_single_line() {
    let out = run(&[], b"\x1b[32mxxx\x1b[31myyy");
    assert_eq!(out.stdout, b"xxxyyy");
    assert_eq!(
        stderr(&out),
        "set-option -add buffer ansi_color_ranges 1.1,1.3|green 1.4,1.6|red\n"
    );
}

#[test]
fn range_flag_offsets_coordinates() {
    let out = run(&["-range", "8.3,10.2"], b"\x1b[32mxxx");
    assert_eq!(
        stderr(&out),
        "set-option -add buffer ansi_color_ranges 8.3,8.5|green\n"
    );
}

#[test]
fn reversed_range_is_normalized() {
    let out = run(&["-range", "10.2,8.3"], b"\x1b[32mxxx");
    assert_eq!(
        stderr(&out),
        "set-option -add buffer ansi_color_ranges 8.3,8.5|green\n"
    );
}

#[test]
fn invalid_argument_fails() {
    let out = run(&["--bogus"], b"");
    assert_eq!(out.status.code(), Some(1));
    // single-quoted Kakoune string: %-expansion inert, embedded ' doubled
    assert_eq!(
        stderr(&out),
        "fail 'kak-ansi-filter: invalid argument ''--bogus'''\n"
    );
}

#[test]
fn invalid_argument_with_quotes_is_escaped() {
    let out = run(&["a'b\"c"], b"");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "fail 'kak-ansi-filter: invalid argument ''a''b\"c'''\n"
    );
}

#[test]
fn expansions_in_argv_stay_inert() {
    // must land inside a single-quoted string, where Kakoune expands nothing
    let out = run(&["%sh{touch /tmp/pwned}"], b"");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "fail 'kak-ansi-filter: invalid argument ''%sh{touch /tmp/pwned}'''\n"
    );
}

#[test]
fn range_needs_an_argument() {
    let out = run(&["-range"], b"");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "fail 'kak-ansi-filter: -range needs an argument'\n");
}

#[test]
fn range_rejects_garbage() {
    let out = run(&["-range", "nonsense"], b"");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "fail 'kak-ansi-filter: invalid value for -range'\n");
}

#[test]
fn range_rejects_trailing_garbage() {
    // deliberately stricter than the reference's sscanf, which ignores
    // trailing garbage and leading whitespace (documented in README)
    for bad in ["1.1,2.2x", " 1.1,2.2", "1.1,2.2,3.3"] {
        let out = run(&["-range", bad], b"");
        assert_eq!(out.status.code(), Some(1), "should reject {bad:?}");
    }
}

#[test]
fn invalid_utf8_does_not_truncate_output() {
    let out = run(&[], b"before\xffafter");
    assert!(out.status.success());
    assert_eq!(out.stdout, "before\u{FFFD}after".as_bytes());
}
