//! Regular expression engine: a thin wrapper over the `regress` crate,
//! replacing the hand-written backtracking engine in regexp.c.
//!
//! Match positions are byte offsets (as in MuJS); conversion to UTF-16 code
//! unit indices happens at the call sites that need them.

use crate::value::{JS_REGEXP_I, JS_REGEXP_M};

/// A compiled regular expression (Reprog).
pub struct Regexp {
    re: regress::Regex,
}

/// One match result (Resub): ranges for group 0 (whole match) followed by
/// the capture groups; unmatched groups are None.
pub type Sub = Vec<Option<(usize, usize)>>;

impl Regexp {
    /// Compile a pattern with JS flags (js_regcomp).
    pub fn compile(pattern: &str, flags: u32) -> Result<Regexp, String> {
        let fl = regress::Flags {
            icase: flags & JS_REGEXP_I != 0,
            multiline: flags & JS_REGEXP_M != 0,
            ..Default::default()
        };
        match regress::Regex::with_flags(pattern, fl) {
            Ok(re) => Ok(Regexp { re }),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Search for the first match at or after byte offset `start`
    /// (js_regexec). `start` is rounded up to a char boundary.
    pub fn exec(&self, text: &str, start: usize) -> Option<Sub> {
        let start = ceil_char_boundary(text, start);
        let m = self.re.find_from(text, start).next()?;
        let mut sub: Sub = Vec::with_capacity(m.captures.len() + 1);
        sub.push(Some((m.start(), m.end())));
        for c in m.captures.iter() {
            sub.push(c.as_ref().map(|r| (r.start, r.end)));
        }
        Some(sub)
    }
}

fn ceil_char_boundary(text: &str, mut i: usize) -> usize {
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let re = Regexp::compile("(a+)(b)?", 0).unwrap();
        let sub = re.exec("xxaab", 0).unwrap();
        assert_eq!(sub[0], Some((2, 5)));
        assert_eq!(sub[1], Some((2, 4)));
        assert_eq!(sub[2], Some((4, 5)));
    }

    #[test]
    fn flags() {
        let re = Regexp::compile("^abc$", JS_REGEXP_I | JS_REGEXP_M).unwrap();
        assert!(re.exec("xx\nAbC\nyy", 0).is_some());
    }
}
