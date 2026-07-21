//! The bytecode compiler (replaces jscompile.c).
//!
//! MuJS packs bytecode into a u16 stream; the Rust port uses a typed
//! instruction enum instead, which is simpler and faster to execute while
//! preserving the exact semantics (including jump layout and line info).

use crate::lex::findword;
use crate::number;
use crate::object::FunRef;
use crate::parse::{Ast, AstNode, AstRef, AstType, Jump, AST_NONE};
use crate::state::{State, R};
use std::collections::HashMap;
use std::rc::Rc;

/// VM instructions (enum js_OpCode). Operands are stored inline.
#[derive(Clone)]
pub enum Op {
    Pop,
    Dup,
    Dup2,
    Rot2,
    Rot3,
    Rot4,

    Integer(i32),
    Number(f64),
    /// push a literal string constant (index into the permanent lit table)
    String(u32),
    Closure(u32),

    NewArray,
    NewObject,
    NewRegexp(Rc<str>, u32),

    Undef,
    Null,
    True,
    False,

    This,
    Current,

    GetLocal(u32),
    SetLocal(u32),
    DelLocal(u32),

    HasVar(Rc<str>),
    GetVar(Rc<str>),
    SetVar(Rc<str>),
    DelVar(Rc<str>),

    In,

    SkipArray,
    InitArray,
    InitProp,
    InitGetter,
    InitSetter,

    GetProp,
    GetPropS(Rc<str>),
    SetProp,
    SetPropS(Rc<str>),
    DelProp,
    DelPropS(Rc<str>),

    Iterator,
    NextIter,

    Eval,
    Call(u32),
    New(u32),

    Typeof,
    Pos,
    Neg,
    BitNot,
    LogNot,
    Inc,
    Dec,
    PostInc,
    PostDec,

    Mul,
    Div,
    Mod,
    Add,
    Sub,
    Shl,
    Shr,
    Ushr,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    JCase(usize),
    BitAnd,
    BitXor,
    BitOr,

    Instanceof,

    Throw,

    Try(usize),
    EndTry,
    Catch(Rc<str>),
    EndCatch,

    With,
    EndWith,

    Debugger,
    Jump(usize),
    JTrue(usize),
    JFalse(usize),
    Return,
}

/// One instruction with its source location (the interpreter reads the
/// location before each op, like MuJS's interleaved line numbers).
#[derive(Clone)]
pub struct Inst {
    pub line: u32,
    pub col: u32,
    pub op: Op,
}

/// A compiled function (js_Function).
pub struct Function {
    pub name: Rc<str>,
    pub script: bool,
    pub lightweight: bool,
    pub strict: bool,
    pub arguments: bool,
    pub numparams: usize,

    pub code: Rc<Vec<Inst>>,
    pub funtab: Rc<Vec<FunRef>>,
    pub vartab: Rc<Vec<Rc<str>>>,

    pub filename: Rc<str>,
    pub line: u32,
    pub col: u32,
}

/// Function under construction.
struct FunBuild {
    name: Rc<str>,
    script: bool,
    lightweight: bool,
    strict: bool,
    arguments: bool,
    numparams: usize,
    code: Vec<Inst>,
    funtab: Vec<FunRef>,
    vartab: Vec<Rc<str>>,
    filename: Rc<str>,
    line: u32,
    col: u32,
    lastline: u32,
    lastcol: u32,
}

const FUTUREWORDS: [&str; 7] = ["class", "const", "enum", "export", "extends", "import", "super"];
const STRICTFUTUREWORDS: [&str; 9] = [
    "implements",
    "interface",
    "let",
    "package",
    "private",
    "protected",
    "public",
    "static",
    "yield",
];

struct Compiler<'a> {
    st: &'a mut State,
    ast: &'a Ast,
    fun: FunBuild,
    /// break/continue jump lists per target node (js_Ast.jumps)
    jumps: HashMap<AstRef, Vec<Jump>>,
    /// switch case clause jump targets (js_Ast.casejump)
    casejumps: HashMap<AstRef, usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocalOp {
    Get,
    Set,
    Del,
}

impl<'a> Compiler<'a> {
    fn node(&self, r: AstRef) -> &'a AstNode {
        self.ast.node(r)
    }

    fn cerror<T>(&mut self, node: AstRef, msg: &str) -> R<T> {
        let (file, line, col) = (
            self.fun.filename.clone(),
            self.node(node).line,
            self.node(node).col,
        );
        self.st.syntax_error_loc(msg, &file, line, col)
    }

    fn checkfutureword(&mut self, exp: AstRef) -> R<()> {
        let name = self.node(exp).string.as_deref().unwrap_or("");
        if findword(name, &FUTUREWORDS).is_some() {
            return self.cerror(exp, &format!("'{}' is a future reserved word", name));
        }
        if self.fun.strict && findword(name, &STRICTFUTUREWORDS).is_some() {
            return self.cerror(
                exp,
                &format!("'{}' is a strict mode future reserved word", name),
            );
        }
        Ok(())
    }

    // -- emit helpers -----------------------------------------------------

    fn emit(&mut self, op: Op) {
        let line = self.fun.lastline;
        let col = self.fun.lastcol;
        self.fun.code.push(Inst { line, col, op });
    }

    fn emitline(&mut self, node: AstRef) {
        if node != AST_NONE {
            self.fun.lastline = self.node(node).line;
            self.fun.lastcol = self.node(node).col;
        } else {
            self.fun.lastline = 0;
            self.fun.lastcol = 0;
        }
    }

    fn addfunction(&mut self, fun: FunRef) -> u32 {
        self.fun.funtab.push(fun);
        (self.fun.funtab.len() - 1) as u32
    }

    fn addlocal(&mut self, ident: AstRef, reuse: bool) -> R<usize> {
        let name = self.node(ident).string.clone().unwrap_or_default();
        if self.fun.strict {
            if name.as_ref() == "arguments" {
                return self.cerror(
                    ident,
                    "redefining 'arguments' is not allowed in strict mode",
                );
            }
            if name.as_ref() == "eval" {
                return self.cerror(ident, "redefining 'eval' is not allowed in strict mode");
            }
        }
        if reuse || self.fun.strict {
            for i in 0..self.fun.vartab.len() {
                if self.fun.vartab[i] == name {
                    if reuse {
                        return Ok(i + 1);
                    }
                    if self.fun.strict {
                        return self.cerror(
                            ident,
                            &format!("duplicate formal parameter '{}'", name),
                        );
                    }
                }
            }
        }
        self.fun.vartab.push(name);
        Ok(self.fun.vartab.len())
    }

    fn findlocal(&self, name: &str) -> i32 {
        for i in (0..self.fun.vartab.len()).rev() {
            if self.fun.vartab[i].as_ref() == name {
                return (i + 1) as i32;
            }
        }
        -1
    }

    fn emitfunction(&mut self, fun: FunRef) {
        self.fun.lightweight = false;
        let idx = self.addfunction(fun);
        self.emit(Op::Closure(idx));
    }

    fn emitnumber(&mut self, num: f64) {
        if num == 0.0 {
            self.emit(Op::Integer(0));
            if num.is_sign_negative() {
                self.emit(Op::Neg);
            }
        } else if (-32768.0..=32767.0).contains(&num) && num == num.trunc() {
            self.emit(Op::Integer(num as i32));
        } else {
            self.emit(Op::Number(num));
        }
    }

    fn emitlocal(&mut self, kind: LocalOp, ident: AstRef) -> R<()> {
        let name = self.node(ident).string.clone().unwrap_or_default();
        let is_arguments = name.as_ref() == "arguments";
        let is_eval = name.as_ref() == "eval";

        if is_arguments {
            self.fun.lightweight = false;
            self.fun.arguments = true;
        }

        self.checkfutureword(ident)?;

        if self.fun.strict && kind == LocalOp::Set {
            if is_arguments {
                return self.cerror(ident, "'arguments' is read-only in strict mode");
            }
            if is_eval {
                return self.cerror(ident, "'eval' is read-only in strict mode");
            }
        }

        let i = self.findlocal(&name);
        if i < 0 {
            match kind {
                LocalOp::Get => self.emit(Op::GetVar(name)),
                LocalOp::Set => self.emit(Op::SetVar(name)),
                LocalOp::Del => self.emit(Op::DelVar(name)),
            }
        } else {
            match kind {
                LocalOp::Get => self.emit(Op::GetLocal(i as u32)),
                LocalOp::Set => self.emit(Op::SetLocal(i as u32)),
                LocalOp::Del => self.emit(Op::DelLocal(i as u32)),
            }
        }
        Ok(())
    }

    fn here(&self) -> usize {
        self.fun.code.len()
    }

    fn emitjump(&mut self, mk: fn(usize) -> Op) -> usize {
        self.emit(mk(0));
        self.fun.code.len() - 1
    }

    fn emitjumpto(&mut self, mk: fn(usize) -> Op, dest: usize) {
        self.emit(mk(dest));
    }

    fn patch(&mut self, inst: usize, target: usize) {
        match &mut self.fun.code[inst].op {
            Op::Jump(t) | Op::JTrue(t) | Op::JFalse(t) | Op::JCase(t) | Op::Try(t) => {
                *t = target
            }
            _ => unreachable!("patch on non-jump instruction"),
        }
    }

    fn label(&mut self, inst: usize) {
        let here = self.here();
        self.patch(inst, here);
    }

    fn labelto(&mut self, inst: usize, target: usize) {
        self.patch(inst, target);
    }

    // -- expressions ----------------------------------------------------------

    fn ctypeof(&mut self, exp: AstRef) -> R<()> {
        let a = self.node(exp).a;
        if self.node(a).typ == AstType::ExpIdentifier {
            self.emitline(a);
            self.emitlocal_special(LocalOp::Get, a, true)?;
        } else {
            self.cexp(a)?;
        }
        self.emitline(exp);
        self.emit(Op::Typeof);
        Ok(())
    }

    /// emitlocal, but typeof on identifiers uses OP_HASVAR instead of OP_GETVAR.
    fn emitlocal_special(&mut self, kind: LocalOp, ident: AstRef, typeof_: bool) -> R<()> {
        if !typeof_ {
            return self.emitlocal(kind, ident);
        }
        let name = self.node(ident).string.clone().unwrap_or_default();
        let is_arguments = name.as_ref() == "arguments";
        if is_arguments {
            self.fun.lightweight = false;
            self.fun.arguments = true;
        }
        self.checkfutureword(ident)?;
        let i = self.findlocal(&name);
        if i < 0 {
            self.emit(Op::HasVar(name));
        } else {
            self.emit(Op::GetLocal(i as u32));
        }
        Ok(())
    }

    fn cunary(&mut self, exp: AstRef, op: Op) -> R<()> {
        self.cexp(self.node(exp).a)?;
        self.emitline(exp);
        self.emit(op);
        Ok(())
    }

    fn cbinary(&mut self, exp: AstRef, op: Op) -> R<()> {
        self.cexp(self.node(exp).a)?;
        self.cexp(self.node(exp).b)?;
        self.emitline(exp);
        self.emit(op);
        Ok(())
    }

    fn carray(&mut self, mut list: AstRef) -> R<()> {
        while list != AST_NONE {
            let a = self.node(list).a;
            self.emitline(a);
            if self.node(a).typ == AstType::ExpElision {
                self.emit(Op::SkipArray);
            } else {
                self.cexp(a)?;
                self.emit(Op::InitArray);
            }
            list = self.node(list).b;
        }
        Ok(())
    }

    fn checkdup(&mut self, mut list: AstRef, end: AstRef) -> R<()> {
        let needle: String = {
            let e = self.node(end).a;
            if self.node(e).typ == AstType::ExpNumber {
                number::number_to_string(self.node(e).number)
            } else {
                self.node(e).string.clone().unwrap_or_default().to_string()
            }
        };

        while self.node(list).a != end {
            if self.node(self.node(list).a).typ == self.node(end).typ {
                let prop = self.node(self.node(list).a).a;
                let straw: String = if self.node(prop).typ == AstType::ExpNumber {
                    number::number_to_string(self.node(prop).number)
                } else {
                    self.node(prop).string.clone().unwrap_or_default().to_string()
                };
                if needle == straw {
                    return self.cerror(
                        list,
                        &format!("duplicate property '{}' in object literal", needle),
                    );
                }
            }
            list = self.node(list).b;
        }
        Ok(())
    }

    fn cobject(&mut self, mut list: AstRef) -> R<()> {
        let head = list;

        while list != AST_NONE {
            let kv = self.node(list).a;
            let prop = self.node(kv).a;

            match self.node(prop).typ {
                AstType::AstIdentifier | AstType::ExpString => {
                    self.emitline(prop);
                    let s = self.node(prop).string.clone().unwrap_or_default();
                    let idx = self.st.heap.lit(&s);
                    self.emit(Op::String(idx));
                }
                AstType::ExpNumber => {
                    self.emitline(prop);
                    let n = self.node(prop).number;
                    self.emitnumber(n);
                }
                _ => return self.cerror(prop, "invalid property name in object initializer"),
            }

            if self.fun.strict {
                self.checkdup(head, kv)?;
            }

            match self.node(kv).typ {
                AstType::ExpPropVal => {
                    self.cexp(self.node(kv).b)?;
                    self.emitline(kv);
                    self.emit(Op::InitProp);
                }
                AstType::ExpPropGet => {
                    let (line, col) = (self.node(prop).line, self.node(prop).col);
                    let body = self.node(kv).c;
                    let fun = self.newfun(line, col, AST_NONE, AST_NONE, body, false, self.fun.strict, true)?;
                    self.emitfunction(fun);
                    self.emitline(kv);
                    self.emit(Op::InitGetter);
                }
                AstType::ExpPropSet => {
                    let (line, col) = (self.node(prop).line, self.node(prop).col);
                    let (args, body) = (self.node(kv).b, self.node(kv).c);
                    let fun = self.newfun(line, col, AST_NONE, args, body, false, self.fun.strict, true)?;
                    self.emitfunction(fun);
                    self.emitline(kv);
                    self.emit(Op::InitSetter);
                }
                _ => {}
            }

            list = self.node(list).b;
        }
        Ok(())
    }

    fn cargs(&mut self, mut list: AstRef) -> R<usize> {
        let mut n = 0;
        while list != AST_NONE {
            self.cexp(self.node(list).a)?;
            list = self.node(list).b;
            n += 1;
        }
        Ok(n)
    }

    fn cassign(&mut self, exp: AstRef) -> R<()> {
        let lhs = self.node(exp).a;
        let rhs = self.node(exp).b;
        match self.node(lhs).typ {
            AstType::ExpIdentifier => {
                self.cexp(rhs)?;
                self.emitline(exp);
                self.emitlocal(LocalOp::Set, lhs)?;
            }
            AstType::ExpIndex => {
                self.cexp(self.node(lhs).a)?;
                self.cexp(self.node(lhs).b)?;
                self.cexp(rhs)?;
                self.emitline(exp);
                self.emit(Op::SetProp);
            }
            AstType::ExpMember => {
                self.cexp(self.node(lhs).a)?;
                self.cexp(rhs)?;
                self.emitline(exp);
                let name = self.node(self.node(lhs).b).string.clone().unwrap_or_default();
                self.emit(Op::SetPropS(name));
            }
            _ => return self.cerror(lhs, "invalid l-value in assignment"),
        }
        Ok(())
    }

    fn cassignforin(&mut self, stm: AstRef) -> R<()> {
        let lhs = self.node(stm).a;

        if self.node(stm).typ == AstType::StmForInVar {
            if self.node(lhs).b != AST_NONE {
                return self.cerror(
                    self.node(lhs).b,
                    "more than one loop variable in for-in statement",
                );
            }
            let target = self.node(self.node(lhs).a).a; // list(var-init(ident))
            self.emitline(self.node(lhs).a);
            self.emitlocal(LocalOp::Set, target)?;
            self.emit(Op::Pop);
            return Ok(());
        }

        match self.node(lhs).typ {
            AstType::ExpIdentifier => {
                self.emitline(lhs);
                self.emitlocal(LocalOp::Set, lhs)?;
                self.emit(Op::Pop);
            }
            AstType::ExpIndex => {
                self.cexp(self.node(lhs).a)?;
                self.cexp(self.node(lhs).b)?;
                self.emitline(lhs);
                self.emit(Op::Rot3);
                self.emit(Op::SetProp);
                self.emit(Op::Pop);
            }
            AstType::ExpMember => {
                self.cexp(self.node(lhs).a)?;
                self.emitline(lhs);
                self.emit(Op::Rot2);
                let name = self.node(self.node(lhs).b).string.clone().unwrap_or_default();
                self.emit(Op::SetPropS(name));
                self.emit(Op::Pop);
            }
            _ => return self.cerror(lhs, "invalid l-value in for-in loop assignment"),
        }
        Ok(())
    }

    fn cassignop1(&mut self, lhs: AstRef) -> R<()> {
        match self.node(lhs).typ {
            AstType::ExpIdentifier => {
                self.emitline(lhs);
                self.emitlocal(LocalOp::Get, lhs)?;
            }
            AstType::ExpIndex => {
                self.cexp(self.node(lhs).a)?;
                self.cexp(self.node(lhs).b)?;
                self.emitline(lhs);
                self.emit(Op::Dup2);
                self.emit(Op::GetProp);
            }
            AstType::ExpMember => {
                self.cexp(self.node(lhs).a)?;
                self.emitline(lhs);
                self.emit(Op::Dup);
                let name = self.node(self.node(lhs).b).string.clone().unwrap_or_default();
                self.emit(Op::GetPropS(name));
            }
            _ => return self.cerror(lhs, "invalid l-value in assignment"),
        }
        Ok(())
    }

    fn cassignop2(&mut self, lhs: AstRef, postfix: bool) -> R<()> {
        match self.node(lhs).typ {
            AstType::ExpIdentifier => {
                self.emitline(lhs);
                if postfix {
                    self.emit(Op::Rot2);
                }
                self.emitlocal(LocalOp::Set, lhs)?;
            }
            AstType::ExpIndex => {
                self.emitline(lhs);
                if postfix {
                    self.emit(Op::Rot4);
                }
                self.emit(Op::SetProp);
            }
            AstType::ExpMember => {
                self.emitline(lhs);
                if postfix {
                    self.emit(Op::Rot3);
                }
                let name = self.node(self.node(lhs).b).string.clone().unwrap_or_default();
                self.emit(Op::SetPropS(name));
            }
            _ => return self.cerror(lhs, "invalid l-value in assignment"),
        }
        Ok(())
    }

    fn cassignop(&mut self, exp: AstRef, op: Op) -> R<()> {
        let lhs = self.node(exp).a;
        let rhs = self.node(exp).b;
        self.cassignop1(lhs)?;
        self.cexp(rhs)?;
        self.emitline(exp);
        self.emit(op);
        self.cassignop2(lhs, false)?;
        Ok(())
    }

    fn cdelete(&mut self, exp: AstRef) -> R<()> {
        let arg = self.node(exp).a;
        match self.node(arg).typ {
            AstType::ExpIdentifier => {
                if self.fun.strict {
                    return self.cerror(
                        exp,
                        "delete on an unqualified name is not allowed in strict mode",
                    );
                }
                self.emitline(exp);
                self.emitlocal(LocalOp::Del, arg)?;
            }
            AstType::ExpIndex => {
                self.cexp(self.node(arg).a)?;
                self.cexp(self.node(arg).b)?;
                self.emitline(exp);
                self.emit(Op::DelProp);
            }
            AstType::ExpMember => {
                self.cexp(self.node(arg).a)?;
                self.emitline(exp);
                let name = self.node(self.node(arg).b).string.clone().unwrap_or_default();
                self.emit(Op::DelPropS(name));
            }
            _ => return self.cerror(exp, "invalid l-value in delete expression"),
        }
        Ok(())
    }

    fn ceval(&mut self, args: AstRef) -> R<()> {
        let n = self.cargs(args)?;
        self.fun.lightweight = false;
        self.fun.arguments = true;
        if n == 0 {
            self.emit(Op::Undef);
        } else {
            for _ in 1..n {
                self.emit(Op::Pop);
            }
        }
        self.emit(Op::Eval);
        Ok(())
    }

    fn ccall(&mut self, fun: AstRef, args: AstRef) -> R<()> {
        match self.node(fun).typ {
            AstType::ExpIndex => {
                self.cexp(self.node(fun).a)?;
                self.emit(Op::Dup);
                self.cexp(self.node(fun).b)?;
                self.emit(Op::GetProp);
                self.emit(Op::Rot2);
            }
            AstType::ExpMember => {
                self.cexp(self.node(fun).a)?;
                self.emit(Op::Dup);
                let name = self.node(self.node(fun).b).string.clone().unwrap_or_default();
                self.emit(Op::GetPropS(name));
                self.emit(Op::Rot2);
            }
            AstType::ExpIdentifier => {
                let name = self.node(fun).string.clone().unwrap_or_default();
                // a direct call on the global eval() compiles to OP_EVAL;
                // a locally shadowed eval is an ordinary function call
                if name.as_ref() == "eval" && self.findlocal("eval") < 0 {
                    return self.ceval(args);
                }
                self.cexp(fun)?;
                self.emit(Op::Undef);
            }
            _ => {
                self.cexp(fun)?;
                self.emit(Op::Undef);
            }
        }
        let n = self.cargs(args)?;
        self.emit(Op::Call(n as u32));
        Ok(())
    }

    fn cexp(&mut self, exp: AstRef) -> R<()> {
        match self.node(exp).typ {
            AstType::ExpString => {
                self.emitline(exp);
                let s = self.node(exp).string.clone().unwrap_or_default();
                let idx = self.st.heap.lit(&s);
                self.emit(Op::String(idx));
            }
            AstType::ExpNumber => {
                self.emitline(exp);
                let n = self.node(exp).number;
                self.emitnumber(n);
            }
            AstType::ExpElision => {}
            AstType::ExpNull => {
                self.emitline(exp);
                self.emit(Op::Null);
            }
            AstType::ExpTrue => {
                self.emitline(exp);
                self.emit(Op::True);
            }
            AstType::ExpFalse => {
                self.emitline(exp);
                self.emit(Op::False);
            }
            AstType::ExpThis => {
                self.emitline(exp);
                self.emit(Op::This);
            }
            AstType::ExpRegexp => {
                self.emitline(exp);
                let s = self.node(exp).string.clone().unwrap_or_default();
                let flags = self.node(exp).number as u32;
                self.emit(Op::NewRegexp(s, flags));
            }
            AstType::ExpObject => {
                self.emitline(exp);
                self.emit(Op::NewObject);
                self.cobject(self.node(exp).a)?;
            }
            AstType::ExpArray => {
                self.emitline(exp);
                self.emit(Op::NewArray);
                self.carray(self.node(exp).a)?;
            }
            AstType::ExpFun => {
                self.emitline(exp);
                let (a, b, c) = {
                    let n = self.node(exp);
                    (n.a, n.b, n.c)
                };
                let (line, col) = (self.node(exp).line, self.node(exp).col);
                let fun = self.newfun(line, col, a, b, c, false, self.fun.strict, true)?;
                self.emitfunction(fun);
            }
            AstType::ExpIdentifier => {
                self.emitline(exp);
                self.emitlocal(LocalOp::Get, exp)?;
            }
            AstType::ExpIndex => {
                self.cexp(self.node(exp).a)?;
                self.cexp(self.node(exp).b)?;
                self.emitline(exp);
                self.emit(Op::GetProp);
            }
            AstType::ExpMember => {
                self.cexp(self.node(exp).a)?;
                self.emitline(exp);
                let name = self.node(self.node(exp).b).string.clone().unwrap_or_default();
                self.emit(Op::GetPropS(name));
            }
            AstType::ExpCall => {
                self.ccall(self.node(exp).a, self.node(exp).b)?;
            }
            AstType::ExpNew => {
                self.cexp(self.node(exp).a)?;
                let n = self.cargs(self.node(exp).b)?;
                self.emitline(exp);
                self.emit(Op::New(n as u32));
            }
            AstType::ExpDelete => {
                self.cdelete(exp)?;
            }
            AstType::ExpPreInc => {
                self.cassignop1(self.node(exp).a)?;
                self.emitline(exp);
                self.emit(Op::Inc);
                self.cassignop2(self.node(exp).a, false)?;
            }
            AstType::ExpPreDec => {
                self.cassignop1(self.node(exp).a)?;
                self.emitline(exp);
                self.emit(Op::Dec);
                self.cassignop2(self.node(exp).a, false)?;
            }
            AstType::ExpPostInc => {
                self.cassignop1(self.node(exp).a)?;
                self.emitline(exp);
                self.emit(Op::PostInc);
                self.cassignop2(self.node(exp).a, true)?;
                self.emit(Op::Pop);
            }
            AstType::ExpPostDec => {
                self.cassignop1(self.node(exp).a)?;
                self.emitline(exp);
                self.emit(Op::PostDec);
                self.cassignop2(self.node(exp).a, true)?;
                self.emit(Op::Pop);
            }
            AstType::ExpVoid => {
                self.cexp(self.node(exp).a)?;
                self.emitline(exp);
                self.emit(Op::Pop);
                self.emit(Op::Undef);
            }
            AstType::ExpTypeof => self.ctypeof(exp)?,
            AstType::ExpPos => self.cunary(exp, Op::Pos)?,
            AstType::ExpNeg => self.cunary(exp, Op::Neg)?,
            AstType::ExpBitNot => self.cunary(exp, Op::BitNot)?,
            AstType::ExpLogNot => self.cunary(exp, Op::LogNot)?,

            AstType::ExpBitOr => self.cbinary(exp, Op::BitOr)?,
            AstType::ExpBitXor => self.cbinary(exp, Op::BitXor)?,
            AstType::ExpBitAnd => self.cbinary(exp, Op::BitAnd)?,
            AstType::ExpEq => self.cbinary(exp, Op::Eq)?,
            AstType::ExpNe => self.cbinary(exp, Op::Ne)?,
            AstType::ExpStrictEq => self.cbinary(exp, Op::StrictEq)?,
            AstType::ExpStrictNe => self.cbinary(exp, Op::StrictNe)?,
            AstType::ExpLt => self.cbinary(exp, Op::Lt)?,
            AstType::ExpGt => self.cbinary(exp, Op::Gt)?,
            AstType::ExpLe => self.cbinary(exp, Op::Le)?,
            AstType::ExpGe => self.cbinary(exp, Op::Ge)?,
            AstType::ExpInstanceof => self.cbinary(exp, Op::Instanceof)?,
            AstType::ExpIn => self.cbinary(exp, Op::In)?,
            AstType::ExpShl => self.cbinary(exp, Op::Shl)?,
            AstType::ExpShr => self.cbinary(exp, Op::Shr)?,
            AstType::ExpUshr => self.cbinary(exp, Op::Ushr)?,
            AstType::ExpAdd => self.cbinary(exp, Op::Add)?,
            AstType::ExpSub => self.cbinary(exp, Op::Sub)?,
            AstType::ExpMul => self.cbinary(exp, Op::Mul)?,
            AstType::ExpDiv => self.cbinary(exp, Op::Div)?,
            AstType::ExpMod => self.cbinary(exp, Op::Mod)?,

            AstType::ExpAss => self.cassign(exp)?,
            AstType::ExpAssMul => self.cassignop(exp, Op::Mul)?,
            AstType::ExpAssDiv => self.cassignop(exp, Op::Div)?,
            AstType::ExpAssMod => self.cassignop(exp, Op::Mod)?,
            AstType::ExpAssAdd => self.cassignop(exp, Op::Add)?,
            AstType::ExpAssSub => self.cassignop(exp, Op::Sub)?,
            AstType::ExpAssShl => self.cassignop(exp, Op::Shl)?,
            AstType::ExpAssShr => self.cassignop(exp, Op::Shr)?,
            AstType::ExpAssUshr => self.cassignop(exp, Op::Ushr)?,
            AstType::ExpAssBitAnd => self.cassignop(exp, Op::BitAnd)?,
            AstType::ExpAssBitXor => self.cassignop(exp, Op::BitXor)?,
            AstType::ExpAssBitOr => self.cassignop(exp, Op::BitOr)?,

            AstType::ExpComma => {
                self.cexp(self.node(exp).a)?;
                self.emitline(exp);
                self.emit(Op::Pop);
                self.cexp(self.node(exp).b)?;
            }

            AstType::ExpLogOr => {
                self.cexp(self.node(exp).a)?;
                self.emitline(exp);
                self.emit(Op::Dup);
                let end = self.emitjump(Op::JTrue);
                self.emit(Op::Pop);
                self.cexp(self.node(exp).b)?;
                self.label(end);
            }

            AstType::ExpLogAnd => {
                self.cexp(self.node(exp).a)?;
                self.emitline(exp);
                self.emit(Op::Dup);
                let end = self.emitjump(Op::JFalse);
                self.emit(Op::Pop);
                self.cexp(self.node(exp).b)?;
                self.label(end);
            }

            AstType::ExpCond => {
                self.cexp(self.node(exp).a)?;
                self.emitline(exp);
                let then = self.emitjump(Op::JTrue);
                self.cexp(self.node(exp).c)?;
                let end = self.emitjump(Op::Jump);
                self.label(then);
                self.cexp(self.node(exp).b)?;
                self.label(end);
            }

            _ => return self.cerror(exp, "unknown expression type"),
        }
        Ok(())
    }

    // -- break/continue patching ------------------------------------------------

    fn addjump(&mut self, typ: AstType, target: AstRef, inst: usize) {
        self.jumps.entry(target).or_default().push(Jump { typ, inst });
    }

    fn labeljumps(&mut self, stm: AstRef, baddr: usize, caddr: usize) {
        let jumps = self.jumps.remove(&stm).unwrap_or_default();
        for jump in jumps {
            if jump.typ == AstType::StmBreak {
                self.labelto(jump.inst, baddr);
            }
            if jump.typ == AstType::StmContinue {
                self.labelto(jump.inst, caddr);
            }
        }
    }

    // -- try/catch/finally --------------------------------------------------------

    /// Compile a finally block that runs while an exception is on the stack.
    /// In script mode statements normally pop the previous statement value;
    /// here that would eat the pending exception, so switch out of script
    /// mode for the handler copy (ES5 requires the exception to propagate).
    fn cfinally_handler(&mut self, finallystm: AstRef) -> R<()> {
        let saved = self.fun.script;
        self.fun.script = false;
        let r = self.cstm(finallystm);
        self.fun.script = saved;
        r
    }

    fn ctryfinally(&mut self, trystm: AstRef, finallystm: AstRef) -> R<()> {
        let l1 = self.emitjump(Op::Try);
        {
            // if we get here, we have caught an exception in the try block
            self.cfinally_handler(finallystm)?; // inline finally block
            self.emit(Op::Throw); // rethrow exception
        }
        self.label(l1);
        self.cstm(trystm)?;
        self.emit(Op::EndTry);
        self.cstm(finallystm)?;
        Ok(())
    }

    fn ctrycatch(&mut self, trystm: AstRef, catchvar: AstRef, catchstm: AstRef) -> R<()> {
        let l1 = self.emitjump(Op::Try);
        {
            // if we get here, we have caught an exception in the try block
            self.checkfutureword(catchvar)?;
            if self.fun.strict {
                let name = self.node(catchvar).string.as_deref().unwrap_or("");
                if name == "arguments" {
                    return self.cerror(
                        catchvar,
                        "redefining 'arguments' is not allowed in strict mode",
                    );
                }
                if name == "eval" {
                    return self.cerror(catchvar, "redefining 'eval' is not allowed in strict mode");
                }
            }
            self.emitline(catchvar);
            let name = self.node(catchvar).string.clone().unwrap_or_default();
            self.emit(Op::Catch(name));
            self.cstm(catchstm)?;
            self.emit(Op::EndCatch);
            let l2 = self.emitjump(Op::Jump); // skip past the try block
            self.label(l1);
            self.cstm(trystm)?;
            self.emit(Op::EndTry);
            self.label(l2);
        }
        Ok(())
    }

    fn ctrycatchfinally(
        &mut self,
        trystm: AstRef,
        catchvar: AstRef,
        catchstm: AstRef,
        finallystm: AstRef,
    ) -> R<()> {
        let l1 = self.emitjump(Op::Try);
        {
            // if we get here, we have caught an exception in the try block
            let l2 = self.emitjump(Op::Try);
            {
                // if we get here, we have caught an exception in the catch block
                self.cfinally_handler(finallystm)?; // inline finally block
                self.emit(Op::Throw); // rethrow exception
            }
            self.label(l2);
            if self.fun.strict {
                self.checkfutureword(catchvar)?;
                let name = self.node(catchvar).string.as_deref().unwrap_or("");
                if name == "arguments" {
                    return self.cerror(
                        catchvar,
                        "redefining 'arguments' is not allowed in strict mode",
                    );
                }
                if name == "eval" {
                    return self.cerror(catchvar, "redefining 'eval' is not allowed in strict mode");
                }
            }
            self.emitline(catchvar);
            let name = self.node(catchvar).string.clone().unwrap_or_default();
            self.emit(Op::Catch(name));
            self.cstm(catchstm)?;
            self.emit(Op::EndCatch);
            self.emit(Op::EndTry);
            let l3 = self.emitjump(Op::Jump); // skip to the finally block
            self.label(l1);
            self.cstm(trystm)?;
            self.emit(Op::EndTry);
            self.label(l3);
            self.cstm(finallystm)?;
        }
        Ok(())
    }

    // -- switch -------------------------------------------------------------------

    fn cswitch(&mut self, ref_: AstRef, head: AstRef) -> R<()> {
        let mut def: AstRef = AST_NONE;
        let mut end: usize = usize::MAX;

        self.cexp(ref_)?;

        // emit an if-else chain of tests for the case clause expressions
        let mut node = head;
        while node != AST_NONE {
            let clause = self.node(node).a;
            if self.node(clause).typ == AstType::StmDefault {
                if def != AST_NONE {
                    return self.cerror(clause, "more than one default label in switch");
                }
                def = clause;
            } else {
                self.cexp(self.node(clause).a)?;
                self.emitline(clause);
                let j = self.emitjump(Op::JCase);
                self.casejumps.insert(clause, j);
            }
            node = self.node(node).b;
        }
        self.emit(Op::Pop);
        if def != AST_NONE {
            self.emitline(def);
            let j = self.emitjump(Op::Jump);
            self.casejumps.insert(def, j);
        } else {
            end = self.emitjump(Op::Jump);
        }

        // emit the case clause bodies
        let mut node = head;
        while node != AST_NONE {
            let clause = self.node(node).a;
            let casejump = *self.casejumps.get(&clause).expect("casejump");
            self.label(casejump);
            if self.node(clause).typ == AstType::StmDefault {
                self.cstmlist(self.node(clause).a)?;
            } else {
                self.cstmlist(self.node(clause).b)?;
            }
            node = self.node(node).b;
        }

        if end != usize::MAX {
            self.label(end);
        }
        Ok(())
    }

    // -- statements -----------------------------------------------------------------

    fn cvarinit(&mut self, mut list: AstRef) -> R<()> {
        while list != AST_NONE {
            let var = self.node(list).a;
            if self.node(var).b != AST_NONE {
                self.cexp(self.node(var).b)?;
                self.emitline(var);
                self.emitlocal(LocalOp::Set, self.node(var).a)?;
                self.emit(Op::Pop);
            }
            list = self.node(list).b;
        }
        Ok(())
    }

    fn cstm(&mut self, stm: AstRef) -> R<()> {
        self.emitline(stm);

        match self.node(stm).typ {
            AstType::AstFundec => {}

            AstType::StmBlock => {
                self.cstmlist(self.node(stm).a)?;
            }

            AstType::StmEmpty => {
                if self.fun.script {
                    self.emitline(stm);
                    self.emit(Op::Pop);
                    self.emit(Op::Undef);
                }
            }

            AstType::StmVar => {
                self.cvarinit(self.node(stm).a)?;
            }

            AstType::StmIf => {
                let (a, b, c) = {
                    let n = self.node(stm);
                    (n.a, n.b, n.c)
                };
                if c != AST_NONE {
                    self.cexp(a)?;
                    self.emitline(stm);
                    let then = self.emitjump(Op::JTrue);
                    self.cstm(c)?;
                    self.emitline(stm);
                    let end = self.emitjump(Op::Jump);
                    self.label(then);
                    self.cstm(b)?;
                    self.label(end);
                } else {
                    self.cexp(a)?;
                    self.emitline(stm);
                    let end = self.emitjump(Op::JFalse);
                    self.cstm(b)?;
                    self.label(end);
                }
            }

            AstType::StmDo => {
                let (a, b) = (self.node(stm).a, self.node(stm).b);
                let loop_ = self.here();
                self.cstm(a)?;
                let cont = self.here();
                self.cexp(b)?;
                self.emitline(stm);
                self.emitjumpto(Op::JTrue, loop_);
                let here = self.here();
                self.labeljumps(stm, here, cont);
            }

            AstType::StmWhile => {
                let (a, b) = (self.node(stm).a, self.node(stm).b);
                let loop_ = self.here();
                self.cexp(a)?;
                self.emitline(stm);
                let end = self.emitjump(Op::JFalse);
                self.cstm(b)?;
                self.emitline(stm);
                self.emitjumpto(Op::Jump, loop_);
                self.label(end);
                let here = self.here();
                self.labeljumps(stm, here, loop_);
            }

            AstType::StmFor | AstType::StmForVar => {
                let (a, b, c, d) = {
                    let n = self.node(stm);
                    (n.a, n.b, n.c, n.d)
                };
                if self.node(stm).typ == AstType::StmForVar {
                    self.cvarinit(a)?;
                } else if a != AST_NONE {
                    self.cexp(a)?;
                    self.emit(Op::Pop);
                }
                let loop_ = self.here();
                let mut end = usize::MAX;
                if b != AST_NONE {
                    self.cexp(b)?;
                    self.emitline(stm);
                    end = self.emitjump(Op::JFalse);
                }
                self.cstm(d)?;
                let cont = self.here();
                if c != AST_NONE {
                    self.cexp(c)?;
                    self.emit(Op::Pop);
                }
                self.emitline(stm);
                self.emitjumpto(Op::Jump, loop_);
                if end != usize::MAX {
                    self.label(end);
                }
                let here = self.here();
                self.labeljumps(stm, here, cont);
            }

            AstType::StmForIn | AstType::StmForInVar => {
                let (b, c) = (self.node(stm).b, self.node(stm).c);
                self.cexp(b)?;
                self.emitline(stm);
                self.emit(Op::Iterator);
                let loop_ = self.here();
                self.emitline(stm);
                self.emit(Op::NextIter);
                let end = self.emitjump(Op::JFalse);
                self.cassignforin(stm)?;
                if self.fun.script {
                    self.emit(Op::Rot2);
                    self.cstm(c)?;
                    self.emit(Op::Rot2);
                } else {
                    self.cstm(c)?;
                }
                self.emitline(stm);
                self.emitjumpto(Op::Jump, loop_);
                self.label(end);
                let here = self.here();
                self.labeljumps(stm, here, loop_);
            }

            AstType::StmSwitch => {
                let (a, b) = (self.node(stm).a, self.node(stm).b);
                self.cswitch(a, b)?;
                let here = self.here();
                self.labeljumps(stm, here, 0);
            }

            AstType::StmLabel => {
                let mut s = stm;
                self.cstm(self.node(s).b)?;
                // skip consecutive labels
                while self.node(s).typ == AstType::StmLabel {
                    s = self.node(s).b;
                }
                // loops and switches have already been labelled
                if !isloop(self.node(s).typ) && self.node(s).typ != AstType::StmSwitch {
                    let here = self.here();
                    self.labeljumps(s, here, 0);
                }
            }

            AstType::StmBreak => {
                let target;
                if self.node(stm).a != AST_NONE {
                    let a = self.node(stm).a;
                    self.checkfutureword(a)?;
                    let label = self.node(a).string.clone().unwrap_or_default();
                    target = self.breaktarget(self.node(stm).parent, Some(&label));
                    if target == AST_NONE {
                        return self.cerror(
                            stm,
                            &format!("break label '{}' not found", label),
                        );
                    }
                } else {
                    target = self.breaktarget(self.node(stm).parent, None);
                    if target == AST_NONE {
                        return self.cerror(stm, "unlabelled break must be inside loop or switch");
                    }
                }
                self.cexit(AstType::StmBreak, stm, target)?;
                self.emitline(stm);
                let j = self.emitjump(Op::Jump);
                self.addjump(AstType::StmBreak, target, j);
            }

            AstType::StmContinue => {
                let target;
                if self.node(stm).a != AST_NONE {
                    let a = self.node(stm).a;
                    self.checkfutureword(a)?;
                    let label = self.node(a).string.clone().unwrap_or_default();
                    target = self.continuetarget(self.node(stm).parent, Some(&label));
                    if target == AST_NONE {
                        return self.cerror(stm, &format!("continue label '{}' not found", label));
                    }
                } else {
                    target = self.continuetarget(self.node(stm).parent, None);
                    if target == AST_NONE {
                        return self.cerror(stm, "continue must be inside loop");
                    }
                }
                self.cexit(AstType::StmContinue, stm, target)?;
                self.emitline(stm);
                let j = self.emitjump(Op::Jump);
                self.addjump(AstType::StmContinue, target, j);
            }

            AstType::StmReturn => {
                if self.node(stm).a != AST_NONE {
                    self.cexp(self.node(stm).a)?;
                } else {
                    self.emit(Op::Undef);
                }
                let target = self.returntarget(self.node(stm).parent);
                if target == AST_NONE {
                    return self.cerror(stm, "return not in function");
                }
                self.cexit(AstType::StmReturn, stm, target)?;
                self.emitline(stm);
                self.emit(Op::Return);
            }

            AstType::StmThrow => {
                self.cexp(self.node(stm).a)?;
                self.emitline(stm);
                self.emit(Op::Throw);
            }

            AstType::StmWith => {
                self.fun.lightweight = false;
                if self.fun.strict {
                    return self.cerror(self.node(stm).a, "'with' statements are not allowed in strict mode");
                }
                self.cexp(self.node(stm).a)?;
                self.emitline(stm);
                self.emit(Op::With);
                self.cstm(self.node(stm).b)?;
                self.emitline(stm);
                self.emit(Op::EndWith);
            }

            AstType::StmTry => {
                self.emitline(stm);
                let (a, b, c, d) = {
                    let n = self.node(stm);
                    (n.a, n.b, n.c, n.d)
                };
                if b != AST_NONE && c != AST_NONE {
                    self.fun.lightweight = false;
                    if d != AST_NONE {
                        self.ctrycatchfinally(a, b, c, d)?;
                    } else {
                        self.ctrycatch(a, b, c)?;
                    }
                } else {
                    self.ctryfinally(a, d)?;
                }
            }

            AstType::StmDebugger => {
                self.emitline(stm);
                self.emit(Op::Debugger);
            }

            _ => {
                if self.fun.script {
                    self.emitline(stm);
                    self.emit(Op::Pop);
                    self.cexp(stm)?;
                } else {
                    self.cexp(stm)?;
                    self.emitline(stm);
                    self.emit(Op::Pop);
                }
            }
        }
        Ok(())
    }

    fn cstmlist(&mut self, mut list: AstRef) -> R<()> {
        while list != AST_NONE {
            let a = self.node(list).a;
            self.cstm(a)?;
            list = self.node(list).b;
        }
        Ok(())
    }

    // -- break/continue/return targets -------------------------------------------

    fn matchlabel(&self, mut node: AstRef, label: &str) -> bool {
        while node != AST_NONE && self.node(node).typ == AstType::StmLabel {
            let s = self.node(self.node(node).a).string.clone().unwrap_or_default();
            if s.as_ref() == label {
                return true;
            }
            node = self.node(node).parent;
        }
        false
    }

    fn breaktarget(&self, mut node: AstRef, label: Option<&str>) -> AstRef {
        while node != AST_NONE {
            if isfun(self.node(node).typ) {
                break;
            }
            match label {
                None => {
                    if isloop(self.node(node).typ) || self.node(node).typ == AstType::StmSwitch {
                        return node;
                    }
                }
                Some(l) => {
                    if self.matchlabel(self.node(node).parent, l) {
                        return node;
                    }
                }
            }
            node = self.node(node).parent;
        }
        AST_NONE
    }

    fn continuetarget(&self, mut node: AstRef, label: Option<&str>) -> AstRef {
        while node != AST_NONE {
            if isfun(self.node(node).typ) {
                break;
            }
            if isloop(self.node(node).typ) {
                match label {
                    None => return node,
                    Some(l) => {
                        if self.matchlabel(self.node(node).parent, l) {
                            return node;
                        }
                    }
                }
            }
            node = self.node(node).parent;
        }
        AST_NONE
    }

    fn returntarget(&self, mut node: AstRef) -> AstRef {
        while node != AST_NONE {
            if isfun(self.node(node).typ) {
                return node;
            }
            node = self.node(node).parent;
        }
        AST_NONE
    }

    /// Emit code to rebalance stack and scopes during an abrupt exit.
    fn cexit(&mut self, typ: AstType, node: AstRef, target: AstRef) -> R<()> {
        let mut node = node;
        loop {
            let prev = node;
            node = self.node(node).parent;
            match self.node(node).typ {
                AstType::StmWith => {
                    self.emitline(node);
                    self.emit(Op::EndWith);
                }
                AstType::StmForIn | AstType::StmForInVar => {
                    self.emitline(node);
                    // pop the iterator if leaving the loop
                    if self.fun.script {
                        if typ == AstType::StmReturn
                            || typ == AstType::StmBreak
                            || (typ == AstType::StmContinue && target != node)
                        {
                            // pop the iterator, save the return or exp value
                            self.emit(Op::Rot2);
                            self.emit(Op::Pop);
                        }
                        if typ == AstType::StmContinue {
                            self.emit(Op::Rot2); // put the iterator back on top
                        }
                    } else {
                        if typ == AstType::StmReturn {
                            // pop the iterator, save the return value
                            self.emit(Op::Rot2);
                            self.emit(Op::Pop);
                        }
                        if typ == AstType::StmBreak || (typ == AstType::StmContinue && target != node)
                        {
                            self.emit(Op::Pop); // pop the iterator
                        }
                    }
                }
                AstType::StmTry => {
                    self.emitline(node);
                    // came from try block
                    if prev == self.node(node).a {
                        self.emit(Op::EndTry);
                        if self.node(node).d != AST_NONE {
                            self.cstm(self.node(node).d)?; // finally
                        }
                    }
                    // came from catch block
                    if prev == self.node(node).c {
                        // ... with finally
                        if self.node(node).d != AST_NONE {
                            self.emit(Op::EndCatch);
                            self.emit(Op::EndTry);
                            self.cstm(self.node(node).d)?; // finally
                        } else {
                            self.emit(Op::EndCatch);
                        }
                    }
                }
                _ => {}
            }
            if node == target {
                break;
            }
        }
        Ok(())
    }

    // -- declarations and programs --------------------------------------------------

    fn cparams(&mut self, mut list: AstRef) -> R<()> {
        self.fun.numparams = listlength(self.ast, list);
        while list != AST_NONE {
            let a = self.node(list).a;
            self.checkfutureword(a)?;
            self.addlocal(a, false)?;
            list = self.node(list).b;
        }
        Ok(())
    }

    fn cvardecs(&mut self, node: AstRef) -> R<()> {
        if self.node(node).typ == AstType::AstList {
            let mut n = node;
            while n != AST_NONE {
                self.cvardecs(self.node(n).a)?;
                n = self.node(n).b;
            }
            return Ok(());
        }

        if isfun(self.node(node).typ) {
            return Ok(()); // stop at inner functions
        }

        if self.node(node).typ == AstType::ExpVar {
            let a = self.node(node).a;
            self.checkfutureword(a)?;
            self.addlocal(a, true)?;
        }

        let (a, b, c, d) = {
            let n = self.node(node);
            (n.a, n.b, n.c, n.d)
        };
        if a != AST_NONE {
            self.cvardecs(a)?;
        }
        if b != AST_NONE {
            self.cvardecs(b)?;
        }
        if c != AST_NONE {
            self.cvardecs(c)?;
        }
        if d != AST_NONE {
            self.cvardecs(d)?;
        }
        Ok(())
    }

    fn cfundecs(&mut self, mut list: AstRef) -> R<()> {
        while list != AST_NONE {
            let stm = self.node(list).a;
            if self.node(stm).typ == AstType::AstFundec {
                self.emitline(stm);
                let (a, b, c) = {
                    let n = self.node(stm);
                    (n.a, n.b, n.c)
                };
                let (line, col) = (self.node(stm).line, self.node(stm).col);
                let fun = self.newfun(line, col, a, b, c, false, self.fun.strict, false)?;
                self.emitfunction(fun);
                self.emitline(stm);
                let idx = self.addlocal(a, true)?;
                self.emit(Op::SetLocal(idx as u32));
                self.emit(Op::Pop);
            }
            list = self.node(list).b;
        }
        Ok(())
    }

    fn cfunbody(
        &mut self,
        name: AstRef,
        params: AstRef,
        body: AstRef,
        is_fun_exp: bool,
    ) -> R<()> {
        self.fun.lightweight = true;
        self.fun.arguments = false;

        if self.fun.script {
            self.fun.lightweight = false;
        }

        // Check if first statement is 'use strict':
        if body != AST_NONE && self.node(body).typ == AstType::AstList {
            let first = self.node(body).a;
            if first != AST_NONE && self.node(first).typ == AstType::ExpString
                && let Some(s) = &self.node(first).string
                    && s.as_ref() == "use strict" {
                        self.fun.strict = true;
                    }
        }

        self.fun.lastline = self.fun.line;
        self.fun.lastcol = self.fun.col;

        self.cparams(params)?;

        if body != AST_NONE {
            self.cvardecs(body)?;
            self.cfundecs(body)?;
        }

        if name != AST_NONE {
            self.checkfutureword(name)?;
            if is_fun_exp {
                let s = self.node(name).string.clone().unwrap_or_default();
                if self.findlocal(&s) < 0 {
                    // TODO: make this binding immutable!
                    self.emit(Op::Current);
                    let idx = self.addlocal(name, true)?;
                    self.emit(Op::SetLocal(idx as u32));
                    self.emit(Op::Pop);
                }
            }
        }

        if self.fun.script {
            self.emit(Op::Undef);
            self.cstmlist(body)?;
            self.emit(Op::Return);
        } else {
            self.cstmlist(body)?;
            self.emit(Op::Undef);
            self.emit(Op::Return);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn newfun(
        &mut self,
        line: u32,
        col: u32,
        name: AstRef,
        params: AstRef,
        body: AstRef,
        script: bool,
        default_strict: bool,
        is_fun_exp: bool,
    ) -> R<FunRef> {
        let fname: Rc<str> = if name != AST_NONE {
            self.node(name).string.clone().unwrap_or_default()
        } else {
            Rc::from("")
        };
        let filename = self.fun.filename.clone();
        let mut sub = Compiler {
            st: &mut *self.st,
            ast: self.ast,
            fun: FunBuild {
                name: fname,
                script,
                lightweight: true,
                strict: default_strict,
                arguments: false,
                numparams: 0,
                code: Vec::new(),
                funtab: Vec::new(),
                vartab: Vec::new(),
                filename,
                line,
                col,
                lastline: line,
                lastcol: col,
            },
            jumps: HashMap::new(),
            casejumps: HashMap::new(),
        };
        sub.cfunbody(name, params, body, is_fun_exp)?;
        let f = Function {
            name: sub.fun.name,
            script: sub.fun.script,
            lightweight: sub.fun.lightweight,
            strict: sub.fun.strict,
            arguments: sub.fun.arguments,
            numparams: sub.fun.numparams,
            code: Rc::new(sub.fun.code),
            funtab: Rc::new(sub.fun.funtab),
            vartab: Rc::new(sub.fun.vartab),
            filename: sub.fun.filename,
            line: sub.fun.line,
            col: sub.fun.col,
        };
        Ok(self.st.heap.alloc_fun(f))
    }
}

fn isloop(t: AstType) -> bool {
    matches!(
        t,
        AstType::StmDo
            | AstType::StmWhile
            | AstType::StmFor
            | AstType::StmForVar
            | AstType::StmForIn
            | AstType::StmForInVar
    )
}

fn isfun(t: AstType) -> bool {
    matches!(
        t,
        AstType::AstFundec | AstType::ExpFun | AstType::ExpPropGet | AstType::ExpPropSet
    )
}

fn listlength(ast: &Ast, mut list: AstRef) -> usize {
    let mut n = 0;
    while list != AST_NONE {
        n += 1;
        list = ast.node(list).b;
    }
    n
}

/// jsC_compilescript: compile a program (global code or eval).
pub fn compile_script(st: &mut State, ast: &Ast, default_strict: bool) -> R<FunRef> {
    let ((root_line, root_col), root) = if ast.root != AST_NONE {
        (
            (ast.node(ast.root).line, ast.node(ast.root).col),
            ast.root,
        )
    } else {
        ((0, 0), AST_NONE)
    };
    let filename = st.heap.intern(&ast.filename);
    let mut c = Compiler {
        st,
        ast,
        fun: FunBuild {
            name: Rc::from(""),
            script: true,
            lightweight: true,
            strict: default_strict,
            arguments: false,
            numparams: 0,
            code: Vec::new(),
            funtab: Vec::new(),
            vartab: Vec::new(),
            filename,
            line: root_line,
            col: root_col,
            lastline: root_line,
            lastcol: root_col,
        },
        jumps: HashMap::new(),
        casejumps: HashMap::new(),
    };
    c.cfunbody(AST_NONE, AST_NONE, root, false)?;
    let f = Function {
        name: c.fun.name,
        script: c.fun.script,
        lightweight: c.fun.lightweight,
        strict: c.fun.strict,
        arguments: c.fun.arguments,
        numparams: c.fun.numparams,
        code: Rc::new(c.fun.code),
        funtab: Rc::new(c.fun.funtab),
        vartab: Rc::new(c.fun.vartab),
        filename: c.fun.filename,
        line: c.fun.line,
        col: c.fun.col,
    };
    Ok(c.st.heap.alloc_fun(f))
}

/// jsC_compilefunction: compile a Function-constructor body.
pub fn compile_function(st: &mut State, ast: &Ast) -> R<FunRef> {
    let root = ast.root;
    let (a, b, c, line, col) = {
        let n = ast.node(root);
        (n.a, n.b, n.c, n.line, n.col)
    };
    let filename = st.heap.intern(&ast.filename);
    let default_strict = st.default_strict;
    let mut comp = Compiler {
        st,
        ast,
        fun: FunBuild {
            name: Rc::from(""),
            script: false,
            lightweight: true,
            strict: default_strict,
            arguments: false,
            numparams: 0,
            code: Vec::new(),
            funtab: Vec::new(),
            vartab: Vec::new(),
            filename,
            line,
            col,
            lastline: line,
            lastcol: col,
        },
        jumps: HashMap::new(),
        casejumps: HashMap::new(),
    };
    comp.newfun(line, col, a, b, c, false, default_strict, true)
}
