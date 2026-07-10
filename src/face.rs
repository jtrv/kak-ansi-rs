//! Colors, attributes, SGR application and Kakoune face formatting.

use std::fmt;

// Attribute bits mirror the reference C: bit = 1 << SGR-code.
pub const BOLD: u16 = 1 << 1;
pub const DIM: u16 = 1 << 2;
pub const ITALIC: u16 = 1 << 3;
pub const UNDERLINE: u16 = 1 << 4;
pub const BLINK: u16 = 1 << 5;
pub const REVERSE: u16 = 1 << 7;
pub const STRIKE: u16 = 1 << 9; // improvement over the C: SGR 9/29 -> Kakoune `s`

const NAMED: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    Named(u8), // 0..=15
    Rgb(u8, u8, u8),
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Color::Default => f.write_str("default"),
            Color::Named(n) => f.write_str(NAMED[n as usize]),
            Color::Rgb(r, g, b) => write!(f, "rgb:{r:02X}{g:02X}{b:02X}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Face {
    pub fg: Color,
    pub bg: Color,
    pub attrs: u16,
}

impl fmt::Display for Face {
    /// `fg[,bg][+attrs]`; fg always shown, bg only when non-default.
    /// Attribute letter order `urbBdi` matches the reference; `s` appended after.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.fg)?;
        if self.bg != Color::Default {
            write!(f, ",{}", self.bg)?;
        }
        if self.attrs != 0 {
            f.write_str("+")?;
            for (bit, ch) in [
                (UNDERLINE, 'u'),
                (REVERSE, 'r'),
                (BOLD, 'b'),
                (BLINK, 'B'),
                (DIM, 'd'),
                (ITALIC, 'i'),
                (STRIKE, 's'),
            ] {
                if self.attrs & bit != 0 {
                    use fmt::Write;
                    f.write_char(ch)?;
                }
            }
        }
        Ok(())
    }
}

/// Extract numeric parameters from a CSI parameter string: maximal ASCII
/// digit runs, everything else (`;`, `:`, `?`, junk) is a separator —
/// matching the reference's tolerant scanner. Overflow saturates
/// (improvement: the C hits `swscanf %d` UB on huge parameters).
pub fn parse_codes(params: &str) -> Vec<u32> {
    params
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().unwrap_or(u32::MAX))
        .collect()
}

/// SGR 38/48 extended color, with the reference's exact index-consumption
/// semantics: the selector is consumed unconditionally; the payload only if
/// fully present; on truncation/unknown selector returns Default and the
/// caller re-processes leftover params as ordinary SGR codes.
fn parse_extended_color(codes: &[u32], i: &mut usize) -> Color {
    if *i >= codes.len() {
        return Color::Default;
    }
    let selector = codes[*i];
    *i += 1;
    match selector {
        2 => {
            // truecolor
            if *i + 3 > codes.len() {
                return Color::Default;
            }
            // improvement over the C: clamp to 0-255 (the C emits invalid
            // colors like rgb:12C0102 which make Kakoune's `source` fail)
            let clamp = |v: u32| v.min(255) as u8;
            let (r, g, b) = (codes[*i], codes[*i + 1], codes[*i + 2]);
            *i += 3;
            Color::Rgb(clamp(r), clamp(g), clamp(b))
        }
        5 => {
            // 256-palette
            if *i >= codes.len() {
                return Color::Default;
            }
            let p = codes[*i];
            *i += 1;
            match p {
                0..=15 => Color::Named(p as u8),
                16..=231 => {
                    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
                    let p = (p - 16) as usize;
                    Color::Rgb(LEVELS[p / 36 % 6], LEVELS[p / 6 % 6], LEVELS[p % 6])
                }
                232..=255 => {
                    let l = (8 + (p - 232) * 10) as u8;
                    Color::Rgb(l, l, l)
                }
                _ => Color::Default,
            }
        }
        _ => Color::Default,
    }
}

impl Face {
    /// Apply a parsed SGR parameter list. Empty list = full reset (`\e[m`).
    pub fn apply_sgr(&mut self, codes: &[u32]) {
        if codes.is_empty() {
            *self = Face::default();
            return;
        }
        let mut i = 0;
        while i < codes.len() {
            let code = codes[i];
            i += 1;
            match code {
                0 => *self = Face::default(),
                1..=5 | 7 | 9 => self.attrs |= 1 << code,
                21 | 23..=25 | 27 | 29 => self.attrs &= !(1 << (code % 10)),
                22 => self.attrs &= !(BOLD | DIM),
                30..=37 => self.fg = Color::Named((code % 10) as u8),
                38 => self.fg = parse_extended_color(codes, &mut i),
                39 => self.fg = Color::Default,
                40..=47 => self.bg = Color::Named((code % 10) as u8),
                48 => self.bg = parse_extended_color(codes, &mut i),
                49 => self.bg = Color::Default,
                90..=97 => self.fg = Color::Named(8 + (code % 10) as u8),
                100..=107 => self.bg = Color::Named(8 + (code % 10) as u8),
                _ => {} // unknown codes silently ignored
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr(codes: &[u32]) -> Face {
        let mut f = Face::default();
        f.apply_sgr(codes);
        f
    }

    #[test]
    fn parse_codes_extracts_digit_runs() {
        assert_eq!(parse_codes("38;2;253;17;129"), vec![38, 2, 253, 17, 129]);
        assert_eq!(parse_codes(""), Vec::<u32>::new());
        assert_eq!(parse_codes("?4"), vec![4]);
        assert_eq!(parse_codes("38:2:1:2:3"), vec![38, 2, 1, 2, 3]);
    }

    #[test]
    fn parse_codes_saturates_huge_params() {
        assert_eq!(parse_codes("99999999999"), vec![u32::MAX]);
    }

    #[test]
    fn basic_colors() {
        assert_eq!(sgr(&[31]).fg, Color::Named(1));
        assert_eq!(sgr(&[97]).fg, Color::Named(15));
        assert_eq!(sgr(&[41]).bg, Color::Named(1));
        assert_eq!(sgr(&[107]).bg, Color::Named(15));
        assert_eq!(sgr(&[31, 39]).fg, Color::Default);
        assert_eq!(sgr(&[41, 49]).bg, Color::Default);
    }

    #[test]
    fn attributes_set_and_clear() {
        assert_eq!(sgr(&[1]).attrs, BOLD);
        assert_eq!(sgr(&[2]).attrs, DIM);
        assert_eq!(sgr(&[3]).attrs, ITALIC);
        assert_eq!(sgr(&[4]).attrs, UNDERLINE);
        assert_eq!(sgr(&[5]).attrs, BLINK);
        assert_eq!(sgr(&[7]).attrs, REVERSE);
        assert_eq!(sgr(&[9]).attrs, STRIKE);
        assert_eq!(sgr(&[1, 21]).attrs, 0);
        assert_eq!(sgr(&[3, 23]).attrs, 0);
        assert_eq!(sgr(&[4, 24]).attrs, 0);
        assert_eq!(sgr(&[5, 25]).attrs, 0);
        assert_eq!(sgr(&[7, 27]).attrs, 0);
        assert_eq!(sgr(&[9, 29]).attrs, 0);
        // 22 clears bold AND dim
        assert_eq!(sgr(&[1, 2, 22]).attrs, 0);
        // 6 (rapid blink), 8 ignored
        assert_eq!(sgr(&[6, 8]).attrs, 0);
    }

    #[test]
    fn reset() {
        assert_eq!(sgr(&[31, 41, 1, 0]), Face::default());
        assert_eq!(sgr(&[]), Face::default()); // \e[m
        let mut f = sgr(&[31]);
        f.apply_sgr(&[]);
        assert_eq!(f, Face::default());
    }

    #[test]
    fn truecolor() {
        assert_eq!(sgr(&[38, 2, 253, 17, 129]).fg, Color::Rgb(253, 17, 129));
        assert_eq!(sgr(&[48, 2, 17, 129, 253]).bg, Color::Rgb(17, 129, 253));
    }

    #[test]
    fn truecolor_clamps_components() {
        assert_eq!(sgr(&[38, 2, 300, 1, 2]).fg, Color::Rgb(255, 1, 2));
    }

    #[test]
    fn palette_256() {
        assert_eq!(sgr(&[38, 5, 2]).fg, Color::Named(2));
        assert_eq!(sgr(&[38, 5, 10]).fg, Color::Named(10));
        assert_eq!(sgr(&[38, 5, 121]).fg, Color::Rgb(0x87, 0xFF, 0xAF));
        assert_eq!(sgr(&[38, 5, 239]).fg, Color::Rgb(0x4E, 0x4E, 0x4E));
        // out-of-range palette index: default, no panic
        assert_eq!(sgr(&[38, 5, 300]).fg, Color::Default);
    }

    #[test]
    fn extended_color_leftover_params_reprocessed() {
        // \e[38;2;1;31m: truncated truecolor -> fg default, then 1 -> bold, 31 -> red
        let f = sgr(&[38, 2, 1, 31]);
        assert_eq!(f.fg, Color::Named(1));
        assert_eq!(f.attrs, BOLD);
        // \e[38;6;31m: unknown selector 6 consumed, 31 continues -> red
        assert_eq!(sgr(&[38, 6, 31]).fg, Color::Named(1));
        // \e[38;5m: truncated palette -> default, nothing left
        assert_eq!(sgr(&[38, 5]), Face::default());
        // bare \e[38m
        assert_eq!(sgr(&[38]), Face::default());
    }

    #[test]
    fn face_formatting() {
        assert_eq!(Face::default().to_string(), "default");
        assert_eq!(sgr(&[31]).to_string(), "red");
        assert_eq!(sgr(&[92]).to_string(), "bright-green");
        assert_eq!(sgr(&[41]).to_string(), "default,red");
        assert_eq!(sgr(&[31, 42]).to_string(), "red,green");
        assert_eq!(sgr(&[38, 2, 253, 17, 129]).to_string(), "rgb:FD1181");
        // attribute order: u r b B d i s
        assert_eq!(
            sgr(&[1, 2, 3, 4, 5, 7, 9]).to_string(),
            "default+urbBdis"
        );
        assert_eq!(sgr(&[31, 1]).to_string(), "red+b");
        assert_eq!(sgr(&[41, 4]).to_string(), "default,red+u");
    }
}
