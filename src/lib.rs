//! es5: a Rust port of the MuJS ES5 JavaScript engine.
//!
//! The module layout mirrors the original C sources:
//! - `number`   replaces jsdtoa.c (uses Rust std float parsing/formatting)
//! - `utf`      replaces utf.c/utfdata.h (uses Rust std char classification)
//! - `value`    replaces jsvalue.c (js_Value and conversions)
//! - `object`   replaces jsproperty.c/jsgc.c/jsintern.c (objects, heap, GC)
//! - `state`    replaces jsstate.c/jsrun.c (interpreter state, stack, calls)
//! - `lex`      replaces jslex.c (tokenizer)
//! - `parse`    replaces jsparse.c (AST + recursive descent parser)
//! - `compile`  replaces jscompile.c (AST -> bytecode compiler)
//! - `run`      replaces the jsR_run interpreter loop
//! - `builtins` replaces jsbuiltin.c and friends
//! - `regexp`   replaces regexp.c with the `regress` crate

pub mod builtins;
pub mod compile;
pub mod diag;
pub mod lex;
pub mod number;
pub mod object;
pub mod parse;
pub mod regexp;
pub mod run;
pub mod state;
pub mod utf;
pub mod value;

pub use state::{State, R};

pub const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = git_version::git_version!();
pub const VERSION: &str = const_str::concat!(CARGO_PKG_VERSION, " ", GIT_HASH);