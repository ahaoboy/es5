//! Fancy terminal diagnostics for uncaught JavaScript errors, built on
//! `miette` (rendering) and `thiserror` (error type derives).
//!
//! Only compiled with the `cli` cargo feature; without it the engine falls
//! back to plain text error reporting.

#![cfg(feature = "cli")]

use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

/// A related note shown below the main diagnostic (one stack frame).
#[derive(Error, Diagnostic, Debug)]
#[error("{0}")]
pub struct TraceNote(pub String);

/// A JavaScript error rendered with a source snippet and highlight.
#[derive(Error, Diagnostic, Debug)]
#[error("{kind}: {message}")]
pub struct JsDiagnostic {
    kind: String,
    message: String,
    #[label("{label_text}")]
    span: SourceSpan,
    label_text: String,
    #[related]
    related: Vec<TraceNote>,
}

impl JsDiagnostic {
    pub fn new(
        kind: String,
        message: String,
        span: SourceSpan,
        label_text: String,
        related: Vec<TraceNote>,
    ) -> JsDiagnostic {
        JsDiagnostic {
            kind,
            message,
            span,
            label_text,
            related,
        }
    }
}

/// Fallback when no source location is available.
#[derive(Error, Diagnostic, Debug)]
#[error("{kind}: {message}")]
pub struct PlainDiagnostic {
    kind: String,
    message: String,
    #[related]
    related: Vec<TraceNote>,
}

impl PlainDiagnostic {
    pub fn new(kind: String, message: String, related: Vec<TraceNote>) -> PlainDiagnostic {
        PlainDiagnostic {
            kind,
            message,
            related,
        }
    }
}

/// Format one captured trace frame for the related-notes section.
pub fn format_trace_frame(f: &crate::object::TraceFrame) -> String {
    if f.line > 0 {
        if !f.name.is_empty() {
            format!("at {} ({}:{}:{})", f.name, f.file, f.line, f.col)
        } else {
            format!("at {}:{}:{}", f.file, f.line, f.col)
        }
    } else {
        format!("at {} ({})", f.name, f.file)
    }
}

/// Convert a 1-based (line, col) pair (col counted in runes) into a byte
/// offset plus the byte length of the remainder of that line.
pub fn line_col_to_span(src: &str, line: u32, col: u32) -> (usize, usize) {
    let mut cur_line = 1u32;
    let mut line_start = 0usize;
    for (i, b) in src.char_indices() {
        if cur_line == line {
            break;
        }
        if b == '\n' {
            cur_line += 1;
            line_start = i + 1;
        }
    }
    // advance (col - 1) runes into the line
    let mut offset = line_start;
    let mut rest = &src[line_start..];
    for _ in 1..col {
        match rest.chars().next() {
            Some(c) if c != '\n' => {
                offset += c.len_utf8();
                rest = &rest[c.len_utf8()..];
            }
            _ => break,
        }
    }
    let line_remain = rest.find('\n').unwrap_or(rest.len());
    let underline = line_remain.max(1);
    (offset, underline)
}
