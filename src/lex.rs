//! The lexical analyzer (replaces jslex.c).
//!
//! Operates on the UTF-8 source as a char stream; newline handling and
//! token boundaries mirror the C implementation exactly.

use crate::number;
use crate::state::{State, R};
use crate::utf;

pub const TK_IDENTIFIER: i32 = 256;
pub const TK_NUMBER: i32 = 257;
pub const TK_STRING: i32 = 258;
pub const TK_REGEXP: i32 = 259;

pub const TK_LE: i32 = 260;
pub const TK_GE: i32 = 261;
pub const TK_EQ: i32 = 262;
pub const TK_NE: i32 = 263;
pub const TK_STRICTEQ: i32 = 264;
pub const TK_STRICTNE: i32 = 265;
pub const TK_SHL: i32 = 266;
pub const TK_SHR: i32 = 267;
pub const TK_USHR: i32 = 268;
pub const TK_AND: i32 = 269;
pub const TK_OR: i32 = 270;
pub const TK_ADD_ASS: i32 = 271;
pub const TK_SUB_ASS: i32 = 272;
pub const TK_MUL_ASS: i32 = 273;
pub const TK_DIV_ASS: i32 = 274;
pub const TK_MOD_ASS: i32 = 275;
pub const TK_SHL_ASS: i32 = 276;
pub const TK_SHR_ASS: i32 = 277;
pub const TK_USHR_ASS: i32 = 278;
pub const TK_AND_ASS: i32 = 279;
pub const TK_OR_ASS: i32 = 280;
pub const TK_XOR_ASS: i32 = 281;
pub const TK_INC: i32 = 282;
pub const TK_DEC: i32 = 283;

pub const TK_BREAK: i32 = 284;
pub const TK_CASE: i32 = 285;
pub const TK_CATCH: i32 = 286;
pub const TK_CONTINUE: i32 = 287;
pub const TK_DEBUGGER: i32 = 288;
pub const TK_DEFAULT: i32 = 289;
pub const TK_DELETE: i32 = 290;
pub const TK_DO: i32 = 291;
pub const TK_ELSE: i32 = 292;
pub const TK_FALSE: i32 = 293;
pub const TK_FINALLY: i32 = 294;
pub const TK_FOR: i32 = 295;
pub const TK_FUNCTION: i32 = 296;
pub const TK_IF: i32 = 297;
pub const TK_IN: i32 = 298;
pub const TK_INSTANCEOF: i32 = 299;
pub const TK_NEW: i32 = 300;
pub const TK_NULL: i32 = 301;
pub const TK_RETURN: i32 = 302;
pub const TK_SWITCH: i32 = 303;
pub const TK_THIS: i32 = 304;
pub const TK_THROW: i32 = 305;
pub const TK_TRUE: i32 = 306;
pub const TK_TRY: i32 = 307;
pub const TK_TYPEOF: i32 = 308;
pub const TK_VAR: i32 = 309;
pub const TK_VOID: i32 = 310;
pub const TK_WHILE: i32 = 311;
pub const TK_WITH: i32 = 312;

const EOF: char = '\0';

/// jsY_findword: binary search in a sorted word list.
pub fn findword(s: &str, list: &[&str]) -> Option<usize> {
    list.binary_search_by(|w| w.as_bytes().cmp(s.as_bytes())).ok()
}

const KEYWORDS: [&str; 29] = [
    "break", "case", "catch", "continue", "debugger", "default", "delete", "do", "else",
    "false", "finally", "for", "function", "if", "in", "instanceof", "new", "null", "return",
    "switch", "this", "throw", "true", "try", "typeof", "var", "void", "while", "with",
];

/// Pretty-print a token for error messages (jsY_tokenstring).
pub fn tokenstring(token: i32) -> String {
    match token {
        0 => "(end-of-file)".to_string(),
        TK_IDENTIFIER => "(identifier)".to_string(),
        TK_NUMBER => "(number)".to_string(),
        TK_STRING => "(string)".to_string(),
        TK_REGEXP => "(regexp)".to_string(),
        TK_LE => "'<='".to_string(),
        TK_GE => "'>='".to_string(),
        TK_EQ => "'=='".to_string(),
        TK_NE => "'!='".to_string(),
        TK_STRICTEQ => "'==='".to_string(),
        TK_STRICTNE => "'!=='".to_string(),
        TK_SHL => "'<<'".to_string(),
        TK_SHR => "'>>'".to_string(),
        TK_USHR => "'>>>'".to_string(),
        TK_AND => "'&&'".to_string(),
        TK_OR => "'||'".to_string(),
        TK_ADD_ASS => "'+='".to_string(),
        TK_SUB_ASS => "'-='".to_string(),
        TK_MUL_ASS => "'*='".to_string(),
        TK_DIV_ASS => "'/='".to_string(),
        TK_MOD_ASS => "'%='".to_string(),
        TK_SHL_ASS => "'<<='".to_string(),
        TK_SHR_ASS => "'>>='".to_string(),
        TK_USHR_ASS => "'>>>='".to_string(),
        TK_AND_ASS => "'&='".to_string(),
        TK_OR_ASS => "'|='".to_string(),
        TK_XOR_ASS => "'^='".to_string(),
        TK_INC => "'++'".to_string(),
        TK_DEC => "'--'".to_string(),
        TK_BREAK..=TK_WITH => format!("'{}'", KEYWORDS[(token - TK_BREAK) as usize]),
        c if (32..=126).contains(&c) => format!("'{}'", (c as u8) as char),
        c if c < 128 => format!("'\\x{:02X}'", c),
        _ => "<unknown>".to_string(),
    }
}

/// Lexer state (the lexer-related fields of js_State).
pub struct Lexer {
    pub filename: String,
    source: Vec<char>, // source as chars (newlines normalized on the fly)
    pos: usize,        // index into source (char position of NEXT char)
    pub line: u32,
    pub col: u32,      // column of lexchar (1-based, in runes)
    pub lexline: u32,
    pub lexcol: u32,
    pub lexchar: char, // current lookahead character; EOF == '\0' sentinel
    pub lasttoken: i32,
    pub newline: bool,
    pub text: String,
    pub number: f64,
    lexbuf: String,
    has_eof: bool,
}

impl Lexer {
    /// jsY_initlex
    pub fn new(filename: &str, source: &str) -> Lexer {
        let mut lex = Lexer {
            filename: filename.to_string(),
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            lexline: 1,
            lexcol: 1,
            lexchar: EOF,
            lasttoken: 0,
            newline: false,
            text: String::new(),
            number: 0.0,
            lexbuf: String::new(),
            has_eof: false,
        };
        lex.next();
        lex
    }

    fn syntax_error<T>(&self, st: &mut State, msg: &str) -> R<T> {
        st.syntax_error_loc(msg, &self.filename.clone(), self.lexline, self.lexcol)
    }

    /// jsY_next: consume one rune; normalize newlines.
    fn next(&mut self) {
        if self.has_eof || self.pos >= self.source.len() {
            self.lexchar = EOF;
            self.has_eof = true;
            return;
        }
        let mut c = self.source[self.pos];
        self.pos += 1;
        // consume CR LF as one unit
        if c == '\r' && self.pos < self.source.len() && self.source[self.pos] == '\n' {
            self.pos += 1;
        }
        if utf::is_newline(c) {
            self.line += 1;
            self.col = 1;
            c = '\n';
        } else {
            self.col += 1;
        }
        self.lexchar = c;
    }

    fn accept(&mut self, x: char) -> bool {
        if self.lexchar == x {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, st: &mut State, x: char) -> R<()> {
        if !self.accept(x) {
            return self.syntax_error(st, &format!("expected '{}'", x));
        }
        Ok(())
    }

    /// Handle \uXXXX escapes in identifiers (jsY_unescape).
    fn unescape(&mut self, st: &mut State) -> R<()> {
        if self.accept('\\') {
            if self.accept('u') {
                let mut x: u32 = 0;
                for shift in [12, 8, 4, 0] {
                    if !self.lexchar.is_ascii_hexdigit() {
                        return self.syntax_error(st, "unexpected escape sequence");
                    }
                    x |= self.lexchar.to_digit(16).unwrap() << shift;
                    self.next();
                }
                self.lexchar = char::from_u32(x).unwrap_or('\u{FFFD}');
                return Ok(());
            }
            return self.syntax_error(st, "unexpected escape sequence");
        }
        Ok(())
    }

    fn textinit(&mut self) {
        self.lexbuf.clear();
    }

    fn textpush(&mut self, c: char) {
        self.lexbuf.push(c);
    }

    fn textend(&mut self) -> String {
        std::mem::take(&mut self.lexbuf)
    }

    fn lexlinecomment(&mut self) {
        while self.lexchar != EOF && self.lexchar != '\n' {
            self.next();
        }
    }

    /// lexcomment: returns Err on unterminated comment.
    fn lexcomment(&mut self) -> Result<(), ()> {
        // already consumed initial '/' '*' sequence
        while self.lexchar != EOF {
            if self.accept('*') {
                while self.lexchar == '*' {
                    self.next();
                }
                if self.accept('/') {
                    return Ok(());
                }
            } else {
                self.next();
            }
        }
        Err(())
    }

    fn lexhex(&mut self, st: &mut State) -> R<f64> {
        let mut n = 0.0;
        if !self.lexchar.is_ascii_hexdigit() {
            return self.syntax_error(st, "malformed hexadecimal number");
        }
        while self.lexchar.is_ascii_hexdigit() {
            n = n * 16.0 + self.lexchar.to_digit(16).unwrap() as f64;
            self.next();
        }
        Ok(n)
    }

    fn lexnumber(&mut self, st: &mut State) -> R<i32> {
        let start = self.pos - 1; // index of first char (already consumed)

        if self.accept('0') {
            if self.accept('x') || self.accept('X') {
                self.number = self.lexhex(st)?;
                return Ok(TK_NUMBER);
            }
            if self.lexchar.is_ascii_digit() {
                return self.syntax_error(st, "number with leading zero");
            }
            if self.accept('.') {
                while self.lexchar.is_ascii_digit() {
                    self.next();
                }
            }
        } else if self.accept('.') {
            if !self.lexchar.is_ascii_digit() {
                return Ok('.' as i32);
            }
            while self.lexchar.is_ascii_digit() {
                self.next();
            }
        } else {
            while self.lexchar.is_ascii_digit() {
                self.next();
            }
            if self.accept('.') {
                while self.lexchar.is_ascii_digit() {
                    self.next();
                }
            }
        }

        if self.accept('e') || self.accept('E') {
            if self.lexchar == '-' || self.lexchar == '+' {
                self.next();
            }
            if self.lexchar.is_ascii_digit() {
                while self.lexchar.is_ascii_digit() {
                    self.next();
                }
            } else {
                return self.syntax_error(st, "missing exponent");
            }
        }

        if utf::is_identifier_start(self.lexchar) {
            return self.syntax_error(st, "number with letter suffix");
        }

        let text: String = self.source[start..self.pos].iter().collect();
        self.number = number::strtod(&text);
        Ok(TK_NUMBER)
    }

    fn lexescape(&mut self, st: &mut State) -> R<()> {
        let mut x: u32 = 0;

        // already consumed '\'
        if self.accept('\n') {
            return Ok(());
        }

        match self.lexchar {
            c if c == EOF => return self.syntax_error(st, "unterminated escape sequence"),
            'u' => {
                self.next();
                for shift in [12, 8, 4, 0] {
                    if !self.lexchar.is_ascii_hexdigit() {
                        return self.syntax_error(st, "malformed escape sequence");
                    }
                    x |= self.lexchar.to_digit(16).unwrap() << shift;
                    self.next();
                }
                self.textpush(char::from_u32(x).unwrap_or('\u{FFFD}'));
            }
            'x' => {
                self.next();
                for shift in [4, 0] {
                    if !self.lexchar.is_ascii_hexdigit() {
                        return self.syntax_error(st, "malformed escape sequence");
                    }
                    x |= self.lexchar.to_digit(16).unwrap() << shift;
                    self.next();
                }
                self.textpush(char::from_u32(x).unwrap_or('\u{FFFD}'));
            }
            '0' => {
                self.textpush('\0');
                self.next();
            }
            '\\' => {
                self.textpush('\\');
                self.next();
            }
            '\'' => {
                self.textpush('\'');
                self.next();
            }
            '"' => {
                self.textpush('"');
                self.next();
            }
            'b' => {
                self.textpush('\u{8}');
                self.next();
            }
            'f' => {
                self.textpush('\u{C}');
                self.next();
            }
            'n' => {
                self.textpush('\n');
                self.next();
            }
            'r' => {
                self.textpush('\r');
                self.next();
            }
            't' => {
                self.textpush('\t');
                self.next();
            }
            'v' => {
                self.textpush('\u{B}');
                self.next();
            }
            c => {
                self.textpush(c);
                self.next();
            }
        }
        Ok(())
    }

    fn lexstring(&mut self, st: &mut State) -> R<i32> {
        let q = self.lexchar;
        self.next();

        self.textinit();

        while self.lexchar != q {
            if self.lexchar == EOF || self.lexchar == '\n' {
                return self.syntax_error(st, "string not terminated");
            }
            if self.accept('\\') {
                self.lexescape(st)?;
            } else {
                let c = self.lexchar;
                self.textpush(c);
                self.next();
            }
        }
        self.expect(st, q)?;

        self.text = self.textend();
        Ok(TK_STRING)
    }

    fn lexregexp(&mut self, st: &mut State) -> R<i32> {
        // already consumed initial '/'
        self.textinit();
        let mut inclass = false;

        // regexp body
        while self.lexchar != '/' || inclass {
            if self.lexchar == EOF || self.lexchar == '\n' {
                return self.syntax_error(st, "regular expression not terminated");
            } else if self.accept('\\') {
                if self.accept('/') {
                    self.textpush('/');
                } else {
                    self.textpush('\\');
                    if self.lexchar == EOF || self.lexchar == '\n' {
                        return self.syntax_error(st, "regular expression not terminated");
                    }
                    let c = self.lexchar;
                    self.textpush(c);
                    self.next();
                }
            } else {
                if self.lexchar == '[' && !inclass {
                    inclass = true;
                }
                if self.lexchar == ']' && inclass {
                    inclass = false;
                }
                let c = self.lexchar;
                self.textpush(c);
                self.next();
            }
        }
        self.expect(st, '/')?;

        let body = self.textend();

        // regexp flags
        let (mut g, mut i, mut m) = (0, 0, 0);
        while utf::is_identifier_part(self.lexchar) {
            if self.accept('g') {
                g += 1;
            } else if self.accept('i') {
                i += 1;
            } else if self.accept('m') {
                m += 1;
            } else {
                return self.syntax_error(
                    st,
                    &format!("illegal flag in regular expression: {}", self.lexchar),
                );
            }
        }

        if g > 1 || i > 1 || m > 1 {
            return self.syntax_error(st, "duplicated flag in regular expression");
        }

        self.text = body;

        let mut flags = 0;
        if g > 0 {
            flags |= crate::value::JS_REGEXP_G;
        }
        if i > 0 {
            flags |= crate::value::JS_REGEXP_I;
        }
        if m > 0 {
            flags |= crate::value::JS_REGEXP_M;
        }
        self.number = flags as f64;
        Ok(TK_REGEXP)
    }

    /// the ugliest language wart ever...
    fn isregexpcontext(last: i32) -> bool {
        !matches!(
            last,
            93 | 41 | 125 | TK_IDENTIFIER | TK_NUMBER | TK_STRING | TK_FALSE | TK_NULL | TK_THIS | TK_TRUE
        )
    }

    /// simple "return [no Line Terminator here] ..." contexts
    fn isnlthcontext(last: i32) -> bool {
        matches!(last, TK_BREAK | TK_CONTINUE | TK_RETURN | TK_THROW)
    }

    fn lexx(&mut self, st: &mut State) -> R<i32> {
        self.newline = false;

        loop {
            while utf::is_white(self.lexchar) {
                self.next();
            }

            // save location of beginning of token (whitespace contains no
            // newlines, so capturing after skipping is equivalent for the
            // line and more accurate for the column)
            self.lexline = self.line;
            self.lexcol = self.col;

            if self.accept('\n') {
                self.newline = true;
                if Self::isnlthcontext(self.lasttoken) {
                    return Ok(';' as i32);
                }
                continue;
            }

            if self.accept('/') {
                if self.accept('/') {
                    self.lexlinecomment();
                    continue;
                } else if self.accept('*') {
                    if self.lexcomment().is_err() {
                        return self.syntax_error(st, "multi-line comment not terminated");
                    }
                    continue;
                } else if Self::isregexpcontext(self.lasttoken) {
                    return self.lexregexp(st);
                } else if self.accept('=') {
                    return Ok(TK_DIV_ASS);
                } else {
                    return Ok('/' as i32);
                }
            }

            if self.lexchar.is_ascii_digit() {
                return self.lexnumber(st);
            }

            match self.lexchar {
                '(' | ')' | ',' | ':' | ';' | '?' | '[' | ']' | '{' | '}' | '~' => {
                    let t = self.lexchar as i32;
                    self.next();
                    return Ok(t);
                }
                '\'' | '"' => return self.lexstring(st),
                '.' => return self.lexnumber(st),
                '<' => {
                    self.next();
                    if self.accept('<') {
                        if self.accept('=') {
                            return Ok(TK_SHL_ASS);
                        }
                        return Ok(TK_SHL);
                    }
                    if self.accept('=') {
                        return Ok(TK_LE);
                    }
                    return Ok('<' as i32);
                }
                '>' => {
                    self.next();
                    if self.accept('>') {
                        if self.accept('>') {
                            if self.accept('=') {
                                return Ok(TK_USHR_ASS);
                            }
                            return Ok(TK_USHR);
                        }
                        if self.accept('=') {
                            return Ok(TK_SHR_ASS);
                        }
                        return Ok(TK_SHR);
                    }
                    if self.accept('=') {
                        return Ok(TK_GE);
                    }
                    return Ok('>' as i32);
                }
                '=' => {
                    self.next();
                    if self.accept('=') {
                        if self.accept('=') {
                            return Ok(TK_STRICTEQ);
                        }
                        return Ok(TK_EQ);
                    }
                    return Ok('=' as i32);
                }
                '!' => {
                    self.next();
                    if self.accept('=') {
                        if self.accept('=') {
                            return Ok(TK_STRICTNE);
                        }
                        return Ok(TK_NE);
                    }
                    return Ok('!' as i32);
                }
                '+' => {
                    self.next();
                    if self.accept('+') {
                        return Ok(TK_INC);
                    }
                    if self.accept('=') {
                        return Ok(TK_ADD_ASS);
                    }
                    return Ok('+' as i32);
                }
                '-' => {
                    self.next();
                    if self.accept('-') {
                        return Ok(TK_DEC);
                    }
                    if self.accept('=') {
                        return Ok(TK_SUB_ASS);
                    }
                    return Ok('-' as i32);
                }
                '*' => {
                    self.next();
                    if self.accept('=') {
                        return Ok(TK_MUL_ASS);
                    }
                    return Ok('*' as i32);
                }
                '%' => {
                    self.next();
                    if self.accept('=') {
                        return Ok(TK_MOD_ASS);
                    }
                    return Ok('%' as i32);
                }
                '&' => {
                    self.next();
                    if self.accept('&') {
                        return Ok(TK_AND);
                    }
                    if self.accept('=') {
                        return Ok(TK_AND_ASS);
                    }
                    return Ok('&' as i32);
                }
                '|' => {
                    self.next();
                    if self.accept('|') {
                        return Ok(TK_OR);
                    }
                    if self.accept('=') {
                        return Ok(TK_OR_ASS);
                    }
                    return Ok('|' as i32);
                }
                '^' => {
                    self.next();
                    if self.accept('=') {
                        return Ok(TK_XOR_ASS);
                    }
                    return Ok('^' as i32);
                }
                c if c == EOF => return Ok(0),
                _ => {}
            }

            // Handle \uXXXX escapes in identifiers
            self.unescape(st)?;
            if utf::is_identifier_start(self.lexchar) {
                self.textinit();
                let c = self.lexchar;
                self.textpush(c);

                self.next();
                self.unescape(st)?;
                while utf::is_identifier_part(self.lexchar) {
                    let c = self.lexchar;
                    self.textpush(c);
                    self.next();
                    self.unescape(st)?;
                }

                let word = self.textend();
                if let Some(i) = findword(&word, &KEYWORDS) {
                    self.text = KEYWORDS[i].to_string();
                    return Ok(TK_BREAK + i as i32);
                }
                self.text = word;
                return Ok(TK_IDENTIFIER);
            }

            if ('\u{20}'..='\u{7E}').contains(&self.lexchar) {
                return self.syntax_error(
                    st,
                    &format!("unexpected character: '{}'", self.lexchar),
                );
            }
            return self.syntax_error(
                st,
                &format!("unexpected character: \\u{:04X}", self.lexchar as u32),
            );
        }
    }

    /// jsY_lex
    pub fn lex(&mut self, st: &mut State) -> R<i32> {
        let t = self.lexx(st)?;
        self.lasttoken = t;
        Ok(t)
    }

    // ------------------------------------------------------------------
    // JSON lexer
    // ------------------------------------------------------------------

    fn lexjsonnumber(&mut self, st: &mut State) -> R<i32> {
        let start = self.pos - 1;

        if self.lexchar == '-' {
            self.next();
        }

        if self.lexchar == '0' {
            self.next();
        } else if self.lexchar.is_ascii_digit() {
            while self.lexchar.is_ascii_digit() {
                self.next();
            }
        } else {
            return self.syntax_error(st, "unexpected non-digit");
        }

        if self.accept('.') {
            if self.lexchar.is_ascii_digit() {
                while self.lexchar.is_ascii_digit() {
                    self.next();
                }
            } else {
                return self.syntax_error(st, "missing digits after decimal point");
            }
        }

        if self.accept('e') || self.accept('E') {
            if self.lexchar == '-' || self.lexchar == '+' {
                self.next();
            }
            if self.lexchar.is_ascii_digit() {
                while self.lexchar.is_ascii_digit() {
                    self.next();
                }
            } else {
                return self.syntax_error(st, "missing digits after exponent indicator");
            }
        }

        let text: String = self.source[start..self.pos].iter().collect();
        self.number = number::strtod(&text);
        Ok(TK_NUMBER)
    }

    fn lexjsonescape(&mut self, st: &mut State) -> R<()> {
        let mut x: u32 = 0;

        // already consumed '\'
        match self.lexchar {
            'u' => {
                self.next();
                for shift in [12, 8, 4, 0] {
                    if !self.lexchar.is_ascii_hexdigit() {
                        return self.syntax_error(st, "invalid escape sequence");
                    }
                    x |= self.lexchar.to_digit(16).unwrap() << shift;
                    self.next();
                }
                self.textpush(char::from_u32(x).unwrap_or('\u{FFFD}'));
            }
            '"' => {
                self.textpush('"');
                self.next();
            }
            '\\' => {
                self.textpush('\\');
                self.next();
            }
            '/' => {
                self.textpush('/');
                self.next();
            }
            'b' => {
                self.textpush('\u{8}');
                self.next();
            }
            'f' => {
                self.textpush('\u{C}');
                self.next();
            }
            'n' => {
                self.textpush('\n');
                self.next();
            }
            'r' => {
                self.textpush('\r');
                self.next();
            }
            't' => {
                self.textpush('\t');
                self.next();
            }
            _ => return self.syntax_error(st, "invalid escape sequence"),
        }
        Ok(())
    }

    fn lexjsonstring(&mut self, st: &mut State) -> R<i32> {
        self.textinit();

        while self.lexchar != '"' {
            if self.lexchar == EOF {
                return self.syntax_error(st, "unterminated string");
            } else if (self.lexchar as u32) < 32 {
                return self.syntax_error(st, "invalid control character in string");
            } else if self.accept('\\') {
                self.lexjsonescape(st)?;
            } else {
                let c = self.lexchar;
                self.textpush(c);
                self.next();
            }
        }
        self.expect(st, '"')?;

        self.text = self.textend();
        Ok(TK_STRING)
    }

    /// jsY_lexjson
    pub fn lexjson(&mut self, st: &mut State) -> R<i32> {
        while utf::is_white(self.lexchar) || self.lexchar == '\n' {
            self.next();
        }

        self.lexline = self.line;
        self.lexcol = self.col;

        if self.lexchar.is_ascii_digit() || self.lexchar == '-' {
            return self.lexjsonnumber(st);
        }

        match self.lexchar {
                ',' | ':' | '[' | ']' | '{' | '}' => {
                    let t = self.lexchar as i32;
                    self.next();
                    return Ok(t);
                }
                '"' => {
                    self.next();
                    return self.lexjsonstring(st);
                }
                'f' => {
                    self.next();
                    self.expect(st, 'a')?;
                    self.expect(st, 'l')?;
                    self.expect(st, 's')?;
                    self.expect(st, 'e')?;
                    return Ok(TK_FALSE);
                }
                'n' => {
                    self.next();
                    self.expect(st, 'u')?;
                    self.expect(st, 'l')?;
                    self.expect(st, 'l')?;
                    return Ok(TK_NULL);
                }
                't' => {
                    self.next();
                    self.expect(st, 'r')?;
                    self.expect(st, 'u')?;
                    self.expect(st, 'e')?;
                    return Ok(TK_TRUE);
                }
                c if c == EOF => return Ok(0),
                _ => {}
            }

            if ('\u{20}'..='\u{7E}').contains(&self.lexchar) {
                return self.syntax_error(
                    st,
                    &format!("unexpected character: '{}'", self.lexchar),
                );
            }
            self.syntax_error(
                st,
                &format!("unexpected character: \\u{:04X}", self.lexchar as u32),
            )
    }
}
