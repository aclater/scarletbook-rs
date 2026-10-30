// SPDX-License-Identifier: MIT OR Apache-2.0

//! Text decoding (specification 2, 2.6).
//!
//! The mapping tables of character sets 3 to 6 are the normative tables of
//! `sacd-spec/spec/charsets` (CC0-1.0), embedded verbatim and parsed on first use.

use std::sync::OnceLock;

/// Upper bound of a text string in bytes (specification 2, 2.6.4).
pub const MAX_STRING_BYTES: usize = 4096;

/// A character set code's decoding (specification 2, 2.6.2), from its low 3 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Charset {
    /// Codes 0, 1, 2 and 7: every byte maps to U+0000 to U+00FF.
    Latin1,
    /// Code 3: Shift_JIS-X0213.
    ShiftJisX0213,
    /// Code 4: EUC-KR (KS X 1001).
    EucKr,
    /// Code 5: EUC-CN (GB 2312).
    EucCn,
    /// Code 6: Big5.
    Big5,
}

impl Charset {
    /// The decoding selected by a stored character set code; only the low 3 bits count.
    pub fn from_code(code: u8) -> Charset {
        match code & 7 {
            3 => Charset::ShiftJisX0213,
            4 => Charset::EucKr,
            5 => Charset::EucCn,
            6 => Charset::Big5,
            _ => Charset::Latin1,
        }
    }
}

const NONE: u32 = u32::MAX;
const PAIR_FLAG: u32 = 0x8000_0000;

/// A parsed mapping table: dense lookups for single bytes and byte pairs.
struct Table {
    single: Vec<u32>,
    pairs: Vec<u32>,
    /// Two-code-point entries; a `pairs` value with `PAIR_FLAG` set indexes this.
    double: Vec<(u32, u32)>,
}

impl Table {
    fn parse(src: &str) -> Table {
        let mut t = Table { single: vec![NONE; 256], pairs: vec![NONE; 65536], double: Vec::new() };
        for line in src.lines() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((seq, cps)) = line.split_once('\t') else { continue };
            let mut points = cps
                .split(' ')
                .filter_map(|c| c.strip_prefix("U+").and_then(|h| u32::from_str_radix(h, 16).ok()));
            let (Some(first), second) = (points.next(), points.next()) else { continue };
            let value = match second {
                None => first,
                Some(second) => {
                    let Ok(idx) = u32::try_from(t.double.len()) else { continue };
                    t.double.push((first, second));
                    PAIR_FLAG | idx
                }
            };
            let Ok(key) = u16::from_str_radix(seq, 16) else { continue };
            match seq.len() {
                2 => {
                    if let Some(slot) = t.single.get_mut(usize::from(key)) {
                        *slot = value;
                    }
                }
                4 => {
                    if let Some(slot) = t.pairs.get_mut(usize::from(key)) {
                        *slot = value;
                    }
                }
                _ => {}
            }
        }
        t
    }

    fn push(&self, value: u32, out: &mut String) {
        if value & PAIR_FLAG != 0 {
            let idx = (value & !PAIR_FLAG) as usize;
            if let Some(&(a, b)) = self.double.get(idx) {
                push_cp(a, out);
                push_cp(b, out);
            } else {
                out.push('\u{FFFD}');
            }
        } else {
            push_cp(value, out);
        }
    }
}

fn push_cp(cp: u32, out: &mut String) {
    out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
}

fn table(cs: Charset) -> Option<&'static Table> {
    static SJIS: OnceLock<Table> = OnceLock::new();
    static EUCKR: OnceLock<Table> = OnceLock::new();
    static EUCCN: OnceLock<Table> = OnceLock::new();
    static BIG5: OnceLock<Table> = OnceLock::new();
    Some(match cs {
        Charset::Latin1 => return None,
        Charset::ShiftJisX0213 => {
            SJIS.get_or_init(|| Table::parse(include_str!("../data/charsets/set3-shift-jisx0213.txt")))
        }
        Charset::EucKr => {
            EUCKR.get_or_init(|| Table::parse(include_str!("../data/charsets/set4-euc-kr.txt")))
        }
        Charset::EucCn => {
            EUCCN.get_or_init(|| Table::parse(include_str!("../data/charsets/set5-euc-cn.txt")))
        }
        Charset::Big5 => BIG5.get_or_init(|| Table::parse(include_str!("../data/charsets/set6-big5.txt"))),
    })
}

/// The reserved pairs of set 3 that decode to a single U+FFFD (specification 2, 2.6.3).
const SJIS_RESERVED: [u16; 10] =
    [0x879F, 0x889E, 0x9873, 0x989E, 0xEAA5, 0xEFF8, 0xEFF9, 0xEFFA, 0xEFFB, 0xEFFC];

/// Decodes bytes with a character set (specification 2, 2.6.2 and 2.6.3). Never fails.
pub fn decode(bytes: &[u8], cs: Charset) -> String {
    let Some(t) = table(cs) else {
        return bytes.iter().map(|&b| char::from(b)).collect();
    };
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0usize;
    while let Some(&b0) = bytes.get(i) {
        if let Some(&b1) = bytes.get(i + 1) {
            let pair = u16::from_be_bytes([b0, b1]);
            let v = t.pairs.get(usize::from(pair)).copied().unwrap_or(NONE);
            if v != NONE {
                t.push(v, &mut out);
                i += 2;
                continue;
            }
            if cs == Charset::ShiftJisX0213 && SJIS_RESERVED.contains(&pair) {
                out.push('\u{FFFD}');
                i += 2;
                continue;
            }
        }
        let v = t.single.get(usize::from(b0)).copied().unwrap_or(NONE);
        if v == NONE {
            out.push('\u{FFFD}');
        } else {
            t.push(v, &mut out);
        }
        i += 1;
    }
    out
}

/// The bytes of a text string at `q` in `buf` (specification 2, 2.6.4 item 1): up to the
/// first 0x00, within at most 4096 bytes and the end of the buffer.
pub fn string_bytes(buf: &[u8], q: usize) -> &[u8] {
    let end = q.saturating_add(MAX_STRING_BYTES).min(buf.len());
    let Some(range) = buf.get(q..end) else { return &[] };
    let len = range.iter().position(|&b| b == 0).unwrap_or(range.len());
    range.get(..len).unwrap_or(range)
}

/// Decodes a text string's bytes and replaces control characters (2.6.4 items 2 to 4).
/// Returns `None` when the result is empty (the item is absent).
pub fn decode_string(bytes: &[u8], cs: Charset) -> Option<String> {
    let s: String = decode(bytes, cs).chars().map(|c| if c < '\u{20}' { ' ' } else { c }).collect();
    (!s.is_empty()).then_some(s)
}

/// The text string at `q` in `buf`, decoded (specification 2, 2.6.4).
pub fn text_string(buf: &[u8], q: usize, cs: Charset) -> Option<String> {
    decode_string(string_bytes(buf, q), cs)
}

/// The whitespace set W of specification 2, 2.6.5.
fn is_w(c: char) -> bool {
    matches!(
        c,
        '\u{09}'..='\u{0D}' | '\u{1C}'..='\u{20}' | '\u{85}' | '\u{A0}'
    )
}

/// A fixed text field (specification 2, 2.6.5).
pub fn fixed_text(field: &[u8]) -> String {
    let len = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    let s: String = field.iter().take(len).map(|&b| char::from(b)).collect();
    s.trim_matches(is_w).to_owned()
}

/// A locale's language code (specification 2, 2.5.1 item 1).
pub fn language(code: [u8; 2]) -> String {
    let s: String = code.iter().map(|&b| char::from(b)).collect();
    s.trim_end_matches(['\u{0}', ' ']).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_and_controls() {
        assert_eq!(decode_string(b"a\x01\x7f\xe9", Charset::Latin1).unwrap(), "a \u{7f}\u{e9}");
        assert_eq!(decode_string(b"", Charset::Latin1), None);
        assert_eq!(Charset::from_code(0xFF), Charset::Latin1);
        assert_eq!(Charset::from_code(0x0B), Charset::ShiftJisX0213);
    }

    #[test]
    fn string_bytes_bounds() {
        let buf = [b'a'; 5000];
        assert_eq!(string_bytes(&buf, 0).len(), 4096);
        assert_eq!(string_bytes(&buf, 4990).len(), 10);
        assert_eq!(string_bytes(&buf, 5000).len(), 0);
        assert_eq!(string_bytes(&buf, usize::MAX).len(), 0);
        assert_eq!(string_bytes(b"ab\0cd", 0), b"ab");
    }

    #[test]
    fn fixed_text_and_language() {
        assert_eq!(fixed_text(b" \x85AB C\xa0\0junk"), "AB C");
        assert_eq!(fixed_text(b"\x01X\x01"), "\u{1}X\u{1}");
        assert_eq!(language(*b"e\0"), "e");
        assert_eq!(language([0, b'e']), "\u{0}e");
        assert_eq!(language(*b"  "), "");
    }

    #[test]
    #[cfg_attr(miri, ignore = "parses large tables")]
    fn notable_table_positions() {
        let sj = Charset::ShiftJisX0213;
        assert_eq!(decode(b"\x5c\x7e", sj), "\u{a5}\u{203e}");
        assert_eq!(decode(b"\x81\x5f\x81\x60", sj), "\u{5c}\u{301c}");
        assert_eq!(decode(b"\x87\x9f", sj), "\u{fffd}");
        assert_eq!(decode(b"\x81", sj), "\u{fffd}");
        assert_eq!(decode(&[0xA1, 0xA4, 0xA1, 0xAA], Charset::EucCn), "\u{30fb}\u{2015}");
        assert_eq!(decode(&[0xA1, 0x45, 0xA2, 0x44], Charset::Big5), "\u{2022}\u{a5}");
        assert_eq!(decode(&[0xA2, 0xE6], Charset::EucKr), "\u{20ac}");
        // An unlisted pair: one U+FFFD for the lead byte, then decoding resumes.
        assert_eq!(decode(&[0xA4, 0xD4, b'a'], Charset::EucKr).chars().next(), Some('\u{fffd}'));
    }

    #[test]
    #[cfg_attr(miri, ignore = "parses large tables")]
    fn tables_have_documented_sizes() {
        // Header comments state the entry counts; check the parse saw all of them.
        for (cs, src) in [
            (Charset::ShiftJisX0213, include_str!("../data/charsets/set3-shift-jisx0213.txt")),
            (Charset::EucKr, include_str!("../data/charsets/set4-euc-kr.txt")),
            (Charset::EucCn, include_str!("../data/charsets/set5-euc-cn.txt")),
            (Charset::Big5, include_str!("../data/charsets/set6-big5.txt")),
        ] {
            let t = table(cs).unwrap();
            let singles = t.single.iter().filter(|&&v| v != NONE).count();
            let pairs = t.pairs.iter().filter(|&&v| v != NONE).count();
            let header = src.lines().find(|l| l.contains("single-byte and")).unwrap();
            let expected = format!("# {singles} single-byte and {pairs} two-byte entries.");
            assert_eq!(header, expected, "{cs:?}");
        }
    }
}
