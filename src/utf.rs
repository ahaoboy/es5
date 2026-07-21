//! UTF helpers built on the Rust standard library, replacing utf.c and the
//! generated tables in utfdata.h. Strings are stored as UTF-8 (`str`), and
//! characters are Unicode scalar values (`char`), which play the role of
//! mujs's `Rune`.
//!
//! Note: mujs counts string length in UTF-16 code units (surrogate pairs for
//! astral characters); the helpers below reproduce that behavior on top of
//! UTF-8 storage.

/// Decode the first char of a string; mirrors chartorune.
/// Returns (rune, bytes consumed), or (U+FFFD, 1) for empty input.
pub fn chartorune(s: &str) -> (char, usize) {
    match s.chars().next() {
        Some(c) => (c, c.len_utf8()),
        None => ('\u{FFFD}', 0),
    }
}

/// Number of UTF-8 bytes needed to encode a char (runelen).
pub fn runelen(c: char) -> usize {
    c.len_utf8()
}

/// isalpharune: Unicode alphabetic property.
pub fn isalpharune(c: char) -> bool {
    c.is_alphabetic()
}

/// islowerrune / isupperrune.
pub fn islowerrune(c: char) -> bool {
    c.is_lowercase()
}

pub fn isupperrune(c: char) -> bool {
    c.is_uppercase()
}

/// Simple (1:1) case mappings; mujs uses the UnicodeData simple mappings,
/// so we take the first character of the full mapping provided by std.
pub fn tolowerrune(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

pub fn toupperrune(c: char) -> char {
    c.to_uppercase().next().unwrap_or(c)
}

/// Full case mappings (may expand to multiple chars), replacing
/// tolowerrune_full / toupperrune_full.
pub fn tolowerrune_full(c: char) -> Option<String> {
    let mut it = c.to_lowercase();
    let _ = it.next()?;
    if it.next().is_some() {
        Some(c.to_lowercase().collect())
    } else {
        // simple mapping suffices; caller uses tolowerrune
        None
    }
}

pub fn toupperrune_full(c: char) -> Option<String> {
    let mut it = c.to_uppercase();
    let first = it.next()?;
    if it.next().is_some() {
        Some(c.to_uppercase().collect())
    } else {
        let _ = first;
        None
    }
}

/// White space recognized by the lexer (jsY_iswhite).
pub fn is_white(c: char) -> bool {
    matches!(c, '\t' | '\u{B}' | '\u{C}' | ' ' | '\u{A0}' | '\u{FEFF}')
}

/// Line terminators recognized by the lexer (jsY_isnewline).
pub fn is_newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Combined test used by string<->number conversions.
pub fn is_js_white_or_newline(c: char) -> bool {
    is_white(c) || is_newline(c)
}

/// Identifier start character (jsY_isidentifierstart).
pub fn is_identifier_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '$' || c == '_' || (c as u32 > 0x7f && isalpharune(c))
}

/// Identifier part character (jsY_isidentifierpart).
pub fn is_identifier_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '$' || c == '_' || (c as u32 > 0x7f && isalpharune(c))
}

// ---------------------------------------------------------------------------
// UTF-16 code-unit indexing (mujs string semantics on UTF-8 storage)
// ---------------------------------------------------------------------------

/// Length of a string in UTF-16 code units (js_utflen).
pub fn utflen(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

/// Get the UTF-16 code unit at the given code-unit index (js_runeat).
/// Astral characters yield surrogate halves. Returns None past the end.
pub fn runeat(s: &str, i: usize) -> Option<u32> {
    let mut pos = 0usize;
    for c in s.chars() {
        let w = c.len_utf16();
        if pos == i {
            if w == 1 {
                return Some(c as u32);
            }
            let v = c as u32 - 0x10000;
            return Some(0xD800 + (v >> 10));
        }
        if w == 2 && pos + 1 == i {
            let v = c as u32 - 0x10000;
            return Some(0xDC00 + (v & 0x3FF));
        }
        pos += w;
    }
    None
}

/// Byte offset of the given UTF-16 code-unit index, or None if the index
/// splits a surrogate pair or is past the end. Returns the byte offset of
/// the character containing that unit.
pub fn utf16_idx_to_byte(s: &str, i: usize) -> Option<usize> {
    let mut pos = 0usize;
    for (b, c) in s.char_indices() {
        if pos == i {
            return Some(b);
        }
        pos += c.len_utf16();
    }
    if pos == i {
        return Some(s.len());
    }
    None
}

/// Convert a byte offset into a UTF-16 code-unit index (js_utfptrtoidx).
pub fn byte_to_utf16_idx(s: &str, byte: usize) -> usize {
    let mut i = 0;
    for (b, c) in s.char_indices() {
        if b >= byte {
            break;
        }
        i += c.len_utf16();
    }
    i
}

/// Extract a substring by UTF-16 code-unit range [a, a+n), splitting
/// surrogate pairs the way Sp_substring_imp does in jsstring.c.
pub fn substring_utf16(s: &str, a: usize, n: usize) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    let end = a + n;
    for c in s.chars() {
        let w = c.len_utf16();
        let next = pos + w;
        if next <= a {
            pos = next;
            continue;
        }
        if pos >= end {
            break;
        }
        if pos >= a && next <= end {
            out.push(c);
        } else {
            // partial overlap: emit the visible surrogate half
            let v = c as u32 - 0x10000;
            if pos < a {
                // starts with low surrogate
                out.push(char::from_u32(0xDC00 + (v & 0x3FF)).unwrap_or('\u{FFFD}'));
            } else {
                // ends with high surrogate
                out.push(char::from_u32(0xD800 + (v >> 10)).unwrap_or('\u{FFFD}'));
            }
        }
        pos = next;
    }
    out
}

/// Push a char (or raw surrogate code point) onto a string, encoding
/// unpaired surrogates as U+FFFD (mujs would emit raw WTF-8 bytes; Rust
/// strings must be valid UTF-8).
pub fn push_rune(out: &mut String, rune: u32) {
    out.push(char::from_u32(rune).unwrap_or('\u{FFFD}'));
}
