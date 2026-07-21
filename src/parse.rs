//! The recursive descent parser (replaces jsparse.c).
//!
//! The AST is stored in an arena (index-based), which allows the parent
//! pointers needed by the compiler's break/continue target resolution.

use crate::lex::{Lexer, TK_IDENTIFIER, TK_NUMBER, TK_REGEXP, TK_STRING, TK_VAR, TK_WITH};
use crate::lex::{TK_ADD_ASS, TK_AND, TK_AND_ASS, TK_BREAK, TK_CASE, TK_CATCH, TK_CONTINUE, TK_DEBUGGER};
use crate::lex::{TK_DEC, TK_DEFAULT, TK_DELETE, TK_DIV_ASS, TK_DO, TK_ELSE, TK_EQ, TK_FALSE};
use crate::lex::{TK_FINALLY, TK_FOR, TK_FUNCTION, TK_GE, TK_IF, TK_IN, TK_INC, TK_INSTANCEOF};
use crate::lex::{TK_LE, TK_MOD_ASS, TK_MUL_ASS, TK_NE, TK_NEW, TK_NULL, TK_OR, TK_OR_ASS};
use crate::lex::{TK_RETURN, TK_SHL, TK_SHL_ASS, TK_SHR, TK_SHR_ASS, TK_STRICTEQ, TK_STRICTNE};
use crate::lex::{TK_SUB_ASS, TK_SWITCH, TK_THIS, TK_THROW, TK_TRUE, TK_TRY, TK_TYPEOF};
use crate::lex::{TK_USHR, TK_USHR_ASS, TK_VOID, TK_WHILE, TK_XOR_ASS};
use crate::state::{State, R};
use std::rc::Rc;

pub type AstRef = u32;
pub const AST_NONE: AstRef = u32::MAX;

const JS_ASTLIMIT: i32 = 400;

/// AST node types (enum js_AstType).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AstType {
    AstList,
    AstFundec,
    AstIdentifier,

    ExpIdentifier,
    ExpNumber,
    ExpString,
    ExpRegexp,

    // literals
    ExpElision,
    ExpNull,
    ExpTrue,
    ExpFalse,
    ExpThis,

    ExpArray,
    ExpObject,
    ExpPropVal,
    ExpPropGet,
    ExpPropSet,

    ExpFun,

    // expressions
    ExpIndex,
    ExpMember,
    ExpCall,
    ExpNew,

    ExpPostInc,
    ExpPostDec,

    ExpDelete,
    ExpVoid,
    ExpTypeof,
    ExpPreInc,
    ExpPreDec,
    ExpPos,
    ExpNeg,
    ExpBitNot,
    ExpLogNot,

    ExpMod,
    ExpDiv,
    ExpMul,
    ExpSub,
    ExpAdd,
    ExpUshr,
    ExpShr,
    ExpShl,
    ExpIn,
    ExpInstanceof,
    ExpGe,
    ExpLe,
    ExpGt,
    ExpLt,
    ExpStrictNe,
    ExpStrictEq,
    ExpNe,
    ExpEq,
    ExpBitAnd,
    ExpBitXor,
    ExpBitOr,
    ExpLogAnd,
    ExpLogOr,

    ExpCond,

    ExpAss,
    ExpAssMul,
    ExpAssDiv,
    ExpAssMod,
    ExpAssAdd,
    ExpAssSub,
    ExpAssShl,
    ExpAssShr,
    ExpAssUshr,
    ExpAssBitAnd,
    ExpAssBitXor,
    ExpAssBitOr,

    ExpComma,

    ExpVar, // var initializer

    // statements
    StmBlock,
    StmEmpty,
    StmVar,
    StmIf,
    StmDo,
    StmWhile,
    StmFor,
    StmForVar,
    StmForIn,
    StmForInVar,
    StmContinue,
    StmBreak,
    StmReturn,
    StmWith,
    StmSwitch,
    StmThrow,
    StmTry,
    StmDebugger,

    StmLabel,
    StmCase,
    StmDefault,
}

/// A break/continue jump to patch (js_JumpList).
#[derive(Clone, Copy)]
pub struct Jump {
    pub typ: AstType,
    pub inst: usize,
}

/// One AST node (js_Ast). The break/continue jump list and switch case
/// jump target are compiler temporaries and live in compile.rs.
pub struct AstNode {
    pub typ: AstType,
    pub line: u32,
    pub col: u32,
    pub a: AstRef,
    pub b: AstRef,
    pub c: AstRef,
    pub d: AstRef,
    pub number: f64,
    pub string: Option<Rc<str>>,
    pub parent: AstRef,
}

/// The complete parse tree arena.
pub struct Ast {
    pub nodes: Vec<AstNode>,
    pub root: AstRef,
    pub filename: String,
}

impl Ast {
    #[inline]
    pub fn node(&self, r: AstRef) -> &AstNode {
        &self.nodes[r as usize]
    }
}

pub struct Parser {
    pub lex: Lexer,
    pub lookahead: i32,
    pub astdepth: i32,
    pub nodes: Vec<AstNode>,
}

impl Parser {
    fn new(filename: &str, source: &str) -> Parser {
        Parser {
            lex: Lexer::new(filename, source),
            lookahead: 0,
            astdepth: 0,
            nodes: Vec::with_capacity(64),
        }
    }

    fn error<T>(&self, st: &mut State, msg: &str) -> R<T> {
        let (file, line, col) = (
            self.lex.filename.clone(),
            self.lex.lexline,
            self.lex.lexcol,
        );
        st.syntax_error_loc(msg, &file, line, col)
    }

    fn warning(&self, st: &mut State, msg: &str) {
        st.report(&format!(
            "{}:{}:{}: warning: {}",
            self.lex.filename, self.lex.lexline, self.lex.lexcol, msg
        ));
    }

    #[inline]
    pub fn node(&self, r: AstRef) -> &AstNode {
        &self.nodes[r as usize]
    }

    #[inline]
    pub fn node_mut(&mut self, r: AstRef) -> &mut AstNode {
        &mut self.nodes[r as usize]
    }

    fn newnode(&mut self, typ: AstType, line: u32, a: AstRef, b: AstRef, c: AstRef, d: AstRef) -> AstRef {
        let idx = self.nodes.len() as AstRef;
        let col = self.lex.lexcol;
        self.nodes.push(AstNode {
            typ,
            line,
            col,
            a,
            b,
            c,
            d,
            number: 0.0,
            string: None,
            parent: AST_NONE,
        });
        for child in [a, b, c, d] {
            if child != AST_NONE {
                self.nodes[child as usize].parent = idx;
            }
        }
        idx
    }

    fn list(&mut self, head: AstRef) -> AstRef {
        // set parent pointers in list nodes
        let mut prev = head;
        let mut node = self.nodes[head as usize].b;
        while node != AST_NONE {
            self.nodes[node as usize].parent = prev;
            prev = node;
            node = self.nodes[node as usize].b;
        }
        head
    }

    fn newstrnode(&mut self, st: &mut State, typ: AstType, s: &str) -> AstRef {
        let line = self.lex.lexline;
        let n = self.newnode(typ, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE);
        let rc = st.heap.intern(s);
        self.nodes[n as usize].string = Some(rc);
        n
    }

    fn newnumnode(&mut self, typ: AstType, x: f64) -> AstRef {
        let line = self.lex.lexline;
        let n = self.newnode(typ, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE);
        self.nodes[n as usize].number = x;
        n
    }

    // -- lookahead ---------------------------------------------------------

    fn next(&mut self, st: &mut State) -> R<()> {
        self.lookahead = self.lex.lex(st)?;
        Ok(())
    }

    fn accept(&mut self, st: &mut State, t: i32) -> R<bool> {
        if self.lookahead == t {
            self.next(st)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn expect(&mut self, st: &mut State, t: i32) -> R<()> {
        if !self.accept(st, t)? {
            let msg = format!(
                "unexpected token: {} (expected {})",
                crate::lex::tokenstring(self.lookahead),
                crate::lex::tokenstring(t)
            );
            return self.error(st, &msg);
        }
        Ok(())
    }

    fn semicolon(&mut self, st: &mut State) -> R<()> {
        if self.lookahead == ';' as i32 {
            self.next(st)?;
            return Ok(());
        }
        if self.lex.newline || self.lookahead == '}' as i32 || self.lookahead == 0 {
            return Ok(());
        }
        let msg = format!(
            "unexpected token: {} (expected ';')",
            crate::lex::tokenstring(self.lookahead)
        );
        self.error(st, &msg)
    }

    // -- literals -----------------------------------------------------------

    fn identifier(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == TK_IDENTIFIER {
            let text = self.lex.text.clone();
            let a = self.newstrnode(st, AstType::AstIdentifier, &text);
            self.next(st)?;
            return Ok(a);
        }
        let msg = format!(
            "unexpected token: {} (expected identifier)",
            crate::lex::tokenstring(self.lookahead)
        );
        self.error(st, &msg)
    }

    fn identifieropt(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == TK_IDENTIFIER {
            return self.identifier(st);
        }
        Ok(AST_NONE)
    }

    fn identifiername(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == TK_IDENTIFIER || self.lookahead >= crate::lex::TK_BREAK {
            let text = self.lex.text.clone();
            let a = self.newstrnode(st, AstType::AstIdentifier, &text);
            self.next(st)?;
            return Ok(a);
        }
        let msg = format!(
            "unexpected token: {} (expected identifier or keyword)",
            crate::lex::tokenstring(self.lookahead)
        );
        self.error(st, &msg)
    }

    fn arrayelement(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;
        if self.lookahead == ',' as i32 {
            return Ok(self.newnode(AstType::ExpElision, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE));
        }
        self.assignment(st, false)
    }

    fn arrayliteral(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == ']' as i32 {
            return Ok(AST_NONE);
        }
        let e = self.arrayelement(st)?;
        let head = self.newnode(AstType::AstList, 0, e, AST_NONE, AST_NONE, AST_NONE);
        let mut tail = head;
        while self.accept(st, ',' as i32)? {
            if self.lookahead != ']' as i32 {
                let e = self.arrayelement(st)?;
                let cell = self.newnode(AstType::AstList, 0, e, AST_NONE, AST_NONE, AST_NONE);
                self.node_mut(tail).b = cell;
                tail = cell;
            }
        }
        Ok(self.list(head))
    }

    fn propname(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == TK_NUMBER {
            let n = self.lex.number;
            let name = self.newnumnode(AstType::ExpNumber, n);
            self.next(st)?;
            Ok(name)
        } else if self.lookahead == TK_STRING {
            let text = self.lex.text.clone();
            let name = self.newstrnode(st, AstType::ExpString, &text);
            self.next(st)?;
            Ok(name)
        } else {
            self.identifiername(st)
        }
    }

    fn propassign(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;
        let mut name = self.propname(st)?;

        if self.lookahead != ':' as i32 && self.node(name).typ == AstType::AstIdentifier {
            let s = self.node(name).string.clone().unwrap();
            if s.as_ref() == "get" {
                name = self.propname(st)?;
                self.expect(st, '(' as i32)?;
                self.expect(st, ')' as i32)?;
                let body = self.funbody(st)?;
                return Ok(self.newnode(AstType::ExpPropGet, line, name, AST_NONE, body, AST_NONE));
            }
            if s.as_ref() == "set" {
                name = self.propname(st)?;
                self.expect(st, '(' as i32)?;
                let arg = self.identifier(st)?;
                self.expect(st, ')' as i32)?;
                let body = self.funbody(st)?;
                let args = self.newnode(AstType::AstList, 0, arg, AST_NONE, AST_NONE, AST_NONE);
                return Ok(self.newnode(AstType::ExpPropSet, line, name, args, body, AST_NONE));
            }
        }

        self.expect(st, ':' as i32)?;
        let value = self.assignment(st, false)?;
        Ok(self.newnode(AstType::ExpPropVal, line, name, value, AST_NONE, AST_NONE))
    }

    fn objectliteral(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == '}' as i32 {
            return Ok(AST_NONE);
        }
        let p = self.propassign(st)?;
        let head = self.newnode(AstType::AstList, 0, p, AST_NONE, AST_NONE, AST_NONE);
        let mut tail = head;
        while self.accept(st, ',' as i32)? {
            if self.lookahead == '}' as i32 {
                break;
            }
            let p = self.propassign(st)?;
            let cell = self.newnode(AstType::AstList, 0, p, AST_NONE, AST_NONE, AST_NONE);
            self.node_mut(tail).b = cell;
            tail = cell;
        }
        Ok(self.list(head))
    }

    // -- functions -----------------------------------------------------------

    fn parameters(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == ')' as i32 {
            return Ok(AST_NONE);
        }
        let i = self.identifier(st)?;
        let head = self.newnode(AstType::AstList, 0, i, AST_NONE, AST_NONE, AST_NONE);
        let mut tail = head;
        while self.accept(st, ',' as i32)? {
            let i = self.identifier(st)?;
            let cell = self.newnode(AstType::AstList, 0, i, AST_NONE, AST_NONE, AST_NONE);
            self.node_mut(tail).b = cell;
            tail = cell;
        }
        Ok(self.list(head))
    }

    fn fundec(&mut self, st: &mut State, line: u32) -> R<AstRef> {
        let a = self.identifier(st)?;
        self.expect(st, '(' as i32)?;
        let b = self.parameters(st)?;
        self.expect(st, ')' as i32)?;
        let c = self.funbody(st)?;
        Ok(self.newnode(AstType::AstFundec, line, a, b, c, AST_NONE))
    }

    fn funstm(&mut self, st: &mut State, line: u32) -> R<AstRef> {
        let a = self.identifier(st)?;
        self.expect(st, '(' as i32)?;
        let b = self.parameters(st)?;
        self.expect(st, ')' as i32)?;
        let c = self.funbody(st)?;
        // rewrite function statement as "var X = function X() {}"
        let fun = self.newnode(AstType::ExpFun, line, a, b, c, AST_NONE);
        let var = self.newnode(AstType::ExpVar, line, a, fun, AST_NONE, AST_NONE);
        let list = self.newnode(AstType::AstList, 0, var, AST_NONE, AST_NONE, AST_NONE);
        Ok(self.newnode(AstType::StmVar, line, list, AST_NONE, AST_NONE, AST_NONE))
    }

    fn funexp(&mut self, st: &mut State, line: u32) -> R<AstRef> {
        let a = self.identifieropt(st)?;
        self.expect(st, '(' as i32)?;
        let b = self.parameters(st)?;
        self.expect(st, ')' as i32)?;
        let c = self.funbody(st)?;
        Ok(self.newnode(AstType::ExpFun, line, a, b, c, AST_NONE))
    }

    // -- expressions ----------------------------------------------------------

    fn primary(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;

        if self.lookahead == TK_IDENTIFIER {
            let text = self.lex.text.clone();
            let a = self.newstrnode(st, AstType::ExpIdentifier, &text);
            self.next(st)?;
            return Ok(a);
        }
        if self.lookahead == TK_STRING {
            let text = self.lex.text.clone();
            let a = self.newstrnode(st, AstType::ExpString, &text);
            self.next(st)?;
            return Ok(a);
        }
        if self.lookahead == TK_REGEXP {
            let text = self.lex.text.clone();
            let a = self.newstrnode(st, AstType::ExpRegexp, &text);
            self.node_mut(a).number = self.lex.number;
            self.next(st)?;
            return Ok(a);
        }
        if self.lookahead == TK_NUMBER {
            let a = self.newnumnode(AstType::ExpNumber, self.lex.number);
            self.next(st)?;
            return Ok(a);
        }

        if self.accept(st, TK_THIS)? {
            return Ok(self.newnode(AstType::ExpThis, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE));
        }
        if self.accept(st, TK_NULL)? {
            return Ok(self.newnode(AstType::ExpNull, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE));
        }
        if self.accept(st, TK_TRUE)? {
            return Ok(self.newnode(AstType::ExpTrue, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE));
        }
        if self.accept(st, TK_FALSE)? {
            return Ok(self.newnode(AstType::ExpFalse, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE));
        }
        if self.accept(st, '{' as i32)? {
            let o = self.objectliteral(st)?;
            let a = self.newnode(AstType::ExpObject, line, o, AST_NONE, AST_NONE, AST_NONE);
            self.expect(st, '}' as i32)?;
            return Ok(a);
        }
        if self.accept(st, '[' as i32)? {
            let o = self.arrayliteral(st)?;
            let a = self.newnode(AstType::ExpArray, line, o, AST_NONE, AST_NONE, AST_NONE);
            self.expect(st, ']' as i32)?;
            return Ok(a);
        }
        if self.accept(st, '(' as i32)? {
            let a = self.expression(st, false)?;
            self.expect(st, ')' as i32)?;
            return Ok(a);
        }

        let msg = format!(
            "unexpected token in expression: {}",
            crate::lex::tokenstring(self.lookahead)
        );
        self.error(st, &msg)
    }

    fn arguments(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == ')' as i32 {
            return Ok(AST_NONE);
        }
        let e = self.assignment(st, false)?;
        let head = self.newnode(AstType::AstList, 0, e, AST_NONE, AST_NONE, AST_NONE);
        let mut tail = head;
        while self.accept(st, ',' as i32)? {
            let e = self.assignment(st, false)?;
            let cell = self.newnode(AstType::AstList, 0, e, AST_NONE, AST_NONE, AST_NONE);
            self.node_mut(tail).b = cell;
            tail = cell;
        }
        Ok(self.list(head))
    }

    fn newexp(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;

        if self.accept(st, TK_NEW)? {
            let a = self.memberexp(st)?;
            if self.accept(st, '(' as i32)? {
                let b = self.arguments(st)?;
                self.expect(st, ')' as i32)?;
                return Ok(self.newnode(AstType::ExpNew, line, a, b, AST_NONE, AST_NONE));
            }
            return Ok(self.newnode(AstType::ExpNew, line, a, AST_NONE, AST_NONE, AST_NONE));
        }

        if self.accept(st, TK_FUNCTION)? {
            return self.funexp(st, line);
        }

        self.primary(st)
    }

    fn increc(&mut self, st: &mut State) -> R<()> {
        self.astdepth += 1;
        if self.astdepth > JS_ASTLIMIT {
            return self.error(st, "too much recursion");
        }
        Ok(())
    }

    fn memberexp(&mut self, st: &mut State) -> R<AstRef> {
        let mut a = self.newexp(st)?;
        let save = self.astdepth;
        loop {
            self.increc(st)?;
            let line = self.lex.lexline;
            if self.accept(st, '.' as i32)? {
                let n = self.identifiername(st)?;
                a = self.newnode(AstType::ExpMember, line, a, n, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, '[' as i32)? {
                let e = self.expression(st, false)?;
                self.expect(st, ']' as i32)?;
                a = self.newnode(AstType::ExpIndex, line, a, e, AST_NONE, AST_NONE);
                continue;
            }
            break;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn callexp(&mut self, st: &mut State) -> R<AstRef> {
        let mut a = self.newexp(st)?;
        let save = self.astdepth;
        loop {
            self.increc(st)?;
            let line = self.lex.lexline;
            if self.accept(st, '.' as i32)? {
                let n = self.identifiername(st)?;
                a = self.newnode(AstType::ExpMember, line, a, n, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, '[' as i32)? {
                let e = self.expression(st, false)?;
                self.expect(st, ']' as i32)?;
                a = self.newnode(AstType::ExpIndex, line, a, e, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, '(' as i32)? {
                let args = self.arguments(st)?;
                self.expect(st, ')' as i32)?;
                a = self.newnode(AstType::ExpCall, line, a, args, AST_NONE, AST_NONE);
                continue;
            }
            break;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn postfix(&mut self, st: &mut State) -> R<AstRef> {
        let a = self.callexp(st)?;
        let line = self.lex.lexline;
        if !self.lex.newline && self.accept(st, TK_INC)? {
            return Ok(self.newnode(AstType::ExpPostInc, line, a, AST_NONE, AST_NONE, AST_NONE));
        }
        if !self.lex.newline && self.accept(st, TK_DEC)? {
            return Ok(self.newnode(AstType::ExpPostDec, line, a, AST_NONE, AST_NONE, AST_NONE));
        }
        Ok(a)
    }

    fn unary(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;
        self.increc(st)?;
        let a = if self.accept(st, TK_DELETE)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpDelete, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else if self.accept(st, TK_VOID)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpVoid, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else if self.accept(st, TK_TYPEOF)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpTypeof, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else if self.accept(st, TK_INC)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpPreInc, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else if self.accept(st, TK_DEC)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpPreDec, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else if self.accept(st, '+' as i32)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpPos, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else if self.accept(st, '-' as i32)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpNeg, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else if self.accept(st, '~' as i32)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpBitNot, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else if self.accept(st, '!' as i32)? {
            let u = self.unary(st)?;
            self.newnode(AstType::ExpLogNot, line, u, AST_NONE, AST_NONE, AST_NONE)
        } else {
            self.postfix(st)?
        };
        self.astdepth -= 1;
        Ok(a)
    }

    fn multiplicative(&mut self, st: &mut State) -> R<AstRef> {
        let mut a = self.unary(st)?;
        let save = self.astdepth;
        loop {
            self.increc(st)?;
            let line = self.lex.lexline;
            if self.accept(st, '*' as i32)? {
                let b = self.unary(st)?;
                a = self.newnode(AstType::ExpMul, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, '/' as i32)? {
                let b = self.unary(st)?;
                a = self.newnode(AstType::ExpDiv, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, '%' as i32)? {
                let b = self.unary(st)?;
                a = self.newnode(AstType::ExpMod, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            break;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn additive(&mut self, st: &mut State) -> R<AstRef> {
        let mut a = self.multiplicative(st)?;
        let save = self.astdepth;
        loop {
            self.increc(st)?;
            let line = self.lex.lexline;
            if self.accept(st, '+' as i32)? {
                let b = self.multiplicative(st)?;
                a = self.newnode(AstType::ExpAdd, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, '-' as i32)? {
                let b = self.multiplicative(st)?;
                a = self.newnode(AstType::ExpSub, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            break;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn shift(&mut self, st: &mut State) -> R<AstRef> {
        let mut a = self.additive(st)?;
        let save = self.astdepth;
        loop {
            self.increc(st)?;
            let line = self.lex.lexline;
            if self.accept(st, TK_SHL)? {
                let b = self.additive(st)?;
                a = self.newnode(AstType::ExpShl, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, TK_SHR)? {
                let b = self.additive(st)?;
                a = self.newnode(AstType::ExpShr, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, TK_USHR)? {
                let b = self.additive(st)?;
                a = self.newnode(AstType::ExpUshr, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            break;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn relational(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let mut a = self.shift(st)?;
        let save = self.astdepth;
        loop {
            self.increc(st)?;
            let line = self.lex.lexline;
            if self.accept(st, '<' as i32)? {
                let b = self.shift(st)?;
                a = self.newnode(AstType::ExpLt, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, '>' as i32)? {
                let b = self.shift(st)?;
                a = self.newnode(AstType::ExpGt, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, TK_LE)? {
                let b = self.shift(st)?;
                a = self.newnode(AstType::ExpLe, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, TK_GE)? {
                let b = self.shift(st)?;
                a = self.newnode(AstType::ExpGe, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, TK_INSTANCEOF)? {
                let b = self.shift(st)?;
                a = self.newnode(AstType::ExpInstanceof, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if !notin && self.accept(st, TK_IN)? {
                let b = self.shift(st)?;
                a = self.newnode(AstType::ExpIn, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            break;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn equality(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let mut a = self.relational(st, notin)?;
        let save = self.astdepth;
        loop {
            self.increc(st)?;
            let line = self.lex.lexline;
            if self.accept(st, TK_EQ)? {
                let b = self.relational(st, notin)?;
                a = self.newnode(AstType::ExpEq, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, TK_NE)? {
                let b = self.relational(st, notin)?;
                a = self.newnode(AstType::ExpNe, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, TK_STRICTEQ)? {
                let b = self.relational(st, notin)?;
                a = self.newnode(AstType::ExpStrictEq, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            if self.accept(st, TK_STRICTNE)? {
                let b = self.relational(st, notin)?;
                a = self.newnode(AstType::ExpStrictNe, line, a, b, AST_NONE, AST_NONE);
                continue;
            }
            break;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn bitand(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let mut a = self.equality(st, notin)?;
        let save = self.astdepth;
        let mut line = self.lex.lexline;
        while self.accept(st, '&' as i32)? {
            self.increc(st)?;
            let b = self.equality(st, notin)?;
            a = self.newnode(AstType::ExpBitAnd, line, a, b, AST_NONE, AST_NONE);
            line = self.lex.lexline;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn bitxor(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let mut a = self.bitand(st, notin)?;
        let save = self.astdepth;
        let mut line = self.lex.lexline;
        while self.accept(st, '^' as i32)? {
            self.increc(st)?;
            let b = self.bitand(st, notin)?;
            a = self.newnode(AstType::ExpBitXor, line, a, b, AST_NONE, AST_NONE);
            line = self.lex.lexline;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn bitor(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let mut a = self.bitxor(st, notin)?;
        let save = self.astdepth;
        let mut line = self.lex.lexline;
        while self.accept(st, '|' as i32)? {
            self.increc(st)?;
            let b = self.bitxor(st, notin)?;
            a = self.newnode(AstType::ExpBitOr, line, a, b, AST_NONE, AST_NONE);
            line = self.lex.lexline;
        }
        self.astdepth = save;
        Ok(a)
    }

    fn logand(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let mut a = self.bitor(st, notin)?;
        let line = self.lex.lexline;
        if self.accept(st, TK_AND)? {
            self.increc(st)?;
            let b = self.logand(st, notin)?;
            a = self.newnode(AstType::ExpLogAnd, line, a, b, AST_NONE, AST_NONE);
            self.astdepth -= 1;
        }
        Ok(a)
    }

    fn logor(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let mut a = self.logand(st, notin)?;
        let line = self.lex.lexline;
        if self.accept(st, TK_OR)? {
            self.increc(st)?;
            let b = self.logor(st, notin)?;
            a = self.newnode(AstType::ExpLogOr, line, a, b, AST_NONE, AST_NONE);
            self.astdepth -= 1;
        }
        Ok(a)
    }

    fn conditional(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let a = self.logor(st, notin)?;
        let line = self.lex.lexline;
        if self.accept(st, '?' as i32)? {
            self.increc(st)?;
            let b = self.assignment(st, false)?;
            self.expect(st, ':' as i32)?;
            let c = self.assignment(st, notin)?;
            self.astdepth -= 1;
            return Ok(self.newnode(AstType::ExpCond, line, a, b, c, AST_NONE));
        }
        Ok(a)
    }

    fn assignment(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let a = self.conditional(st, notin)?;
        let line = self.lex.lexline;
        self.increc(st)?;
        let typ = if self.accept(st, '=' as i32)? {
            Some(AstType::ExpAss)
        } else if self.accept(st, TK_MUL_ASS)? {
            Some(AstType::ExpAssMul)
        } else if self.accept(st, TK_DIV_ASS)? {
            Some(AstType::ExpAssDiv)
        } else if self.accept(st, TK_MOD_ASS)? {
            Some(AstType::ExpAssMod)
        } else if self.accept(st, TK_ADD_ASS)? {
            Some(AstType::ExpAssAdd)
        } else if self.accept(st, TK_SUB_ASS)? {
            Some(AstType::ExpAssSub)
        } else if self.accept(st, TK_SHL_ASS)? {
            Some(AstType::ExpAssShl)
        } else if self.accept(st, TK_SHR_ASS)? {
            Some(AstType::ExpAssShr)
        } else if self.accept(st, TK_USHR_ASS)? {
            Some(AstType::ExpAssUshr)
        } else if self.accept(st, TK_AND_ASS)? {
            Some(AstType::ExpAssBitAnd)
        } else if self.accept(st, TK_XOR_ASS)? {
            Some(AstType::ExpAssBitXor)
        } else if self.accept(st, TK_OR_ASS)? {
            Some(AstType::ExpAssBitOr)
        } else {
            None
        };
        let a = match typ {
            Some(t) => {
                let b = self.assignment(st, notin)?;
                self.newnode(t, line, a, b, AST_NONE, AST_NONE)
            }
            None => a,
        };
        self.astdepth -= 1;
        Ok(a)
    }

    fn expression(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let mut a = self.assignment(st, notin)?;
        let save = self.astdepth;
        let mut line = self.lex.lexline;
        while self.accept(st, ',' as i32)? {
            self.increc(st)?;
            let b = self.assignment(st, notin)?;
            a = self.newnode(AstType::ExpComma, line, a, b, AST_NONE, AST_NONE);
            line = self.lex.lexline;
        }
        self.astdepth = save;
        Ok(a)
    }

    // -- statements -----------------------------------------------------------

    fn vardec(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let a = self.identifier(st)?;
        let line = self.lex.lexline;
        if self.accept(st, '=' as i32)? {
            let b = self.assignment(st, notin)?;
            return Ok(self.newnode(AstType::ExpVar, line, a, b, AST_NONE, AST_NONE));
        }
        Ok(self.newnode(AstType::ExpVar, line, a, AST_NONE, AST_NONE, AST_NONE))
    }

    fn vardeclist(&mut self, st: &mut State, notin: bool) -> R<AstRef> {
        let v = self.vardec(st, notin)?;
        let head = self.newnode(AstType::AstList, 0, v, AST_NONE, AST_NONE, AST_NONE);
        let mut tail = head;
        while self.accept(st, ',' as i32)? {
            let v = self.vardec(st, notin)?;
            let cell = self.newnode(AstType::AstList, 0, v, AST_NONE, AST_NONE, AST_NONE);
            self.node_mut(tail).b = cell;
            tail = cell;
        }
        Ok(self.list(head))
    }

    fn statementlist(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == '}' as i32 || self.lookahead == TK_CASE || self.lookahead == TK_DEFAULT
        {
            return Ok(AST_NONE);
        }
        let s = self.statement(st)?;
        let head = self.newnode(AstType::AstList, 0, s, AST_NONE, AST_NONE, AST_NONE);
        let mut tail = head;
        while self.lookahead != '}' as i32 && self.lookahead != TK_CASE && self.lookahead != TK_DEFAULT
        {
            let s = self.statement(st)?;
            let cell = self.newnode(AstType::AstList, 0, s, AST_NONE, AST_NONE, AST_NONE);
            self.node_mut(tail).b = cell;
            tail = cell;
        }
        Ok(self.list(head))
    }

    fn caseclause(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;

        if self.accept(st, TK_CASE)? {
            let a = self.expression(st, false)?;
            self.expect(st, ':' as i32)?;
            let b = self.statementlist(st)?;
            return Ok(self.newnode(AstType::StmCase, line, a, b, AST_NONE, AST_NONE));
        }

        if self.accept(st, TK_DEFAULT)? {
            self.expect(st, ':' as i32)?;
            let a = self.statementlist(st)?;
            return Ok(self.newnode(AstType::StmDefault, line, a, AST_NONE, AST_NONE, AST_NONE));
        }

        let msg = format!(
            "unexpected token in switch: {} (expected 'case' or 'default')",
            crate::lex::tokenstring(self.lookahead)
        );
        self.error(st, &msg)
    }

    fn caselist(&mut self, st: &mut State) -> R<AstRef> {
        if self.lookahead == '}' as i32 {
            return Ok(AST_NONE);
        }
        let c = self.caseclause(st)?;
        let head = self.newnode(AstType::AstList, 0, c, AST_NONE, AST_NONE, AST_NONE);
        let mut tail = head;
        while self.lookahead != '}' as i32 {
            let c = self.caseclause(st)?;
            let cell = self.newnode(AstType::AstList, 0, c, AST_NONE, AST_NONE, AST_NONE);
            self.node_mut(tail).b = cell;
            tail = cell;
        }
        Ok(self.list(head))
    }

    fn block(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;
        self.expect(st, '{' as i32)?;
        let a = self.statementlist(st)?;
        self.expect(st, '}' as i32)?;
        Ok(self.newnode(AstType::StmBlock, line, a, AST_NONE, AST_NONE, AST_NONE))
    }

    fn forexpression(&mut self, st: &mut State, end: i32) -> R<AstRef> {
        let mut a = AST_NONE;
        if self.lookahead != end {
            a = self.expression(st, false)?;
        }
        self.expect(st, end)?;
        Ok(a)
    }

    fn forstatement(&mut self, st: &mut State, line: u32) -> R<AstRef> {
        self.expect(st, '(' as i32)?;
        if self.accept(st, TK_VAR)? {
            let a = self.vardeclist(st, true)?;
            if self.accept(st, ';' as i32)? {
                let b = self.forexpression(st, ';' as i32)?;
                let c = self.forexpression(st, ')' as i32)?;
                let d = self.statement(st)?;
                return Ok(self.newnode(AstType::StmForVar, line, a, b, c, d));
            }
            if self.accept(st, TK_IN)? {
                let b = self.expression(st, false)?;
                self.expect(st, ')' as i32)?;
                let c = self.statement(st)?;
                return Ok(self.newnode(AstType::StmForInVar, line, a, b, c, AST_NONE));
            }
            let msg = format!(
                "unexpected token in for-var-statement: {}",
                crate::lex::tokenstring(self.lookahead)
            );
            return self.error(st, &msg);
        }

        let a = if self.lookahead != ';' as i32 {
            self.expression(st, true)?
        } else {
            AST_NONE
        };
        if self.accept(st, ';' as i32)? {
            let b = self.forexpression(st, ';' as i32)?;
            let c = self.forexpression(st, ')' as i32)?;
            let d = self.statement(st)?;
            return Ok(self.newnode(AstType::StmFor, line, a, b, c, d));
        }
        if self.accept(st, TK_IN)? {
            let b = self.expression(st, false)?;
            self.expect(st, ')' as i32)?;
            let c = self.statement(st)?;
            return Ok(self.newnode(AstType::StmForIn, line, a, b, c, AST_NONE));
        }
        let msg = format!(
            "unexpected token in for-statement: {}",
            crate::lex::tokenstring(self.lookahead)
        );
        self.error(st, &msg)
    }

    fn statement(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;

        self.increc(st)?;

        let stm: AstRef;

        if self.lookahead == '{' as i32 {
            stm = self.block(st)?;
        } else if self.accept(st, TK_VAR)? {
            let a = self.vardeclist(st, false)?;
            self.semicolon(st)?;
            stm = self.newnode(AstType::StmVar, line, a, AST_NONE, AST_NONE, AST_NONE);
        } else if self.accept(st, ';' as i32)? {
            stm = self.newnode(AstType::StmEmpty, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_IF)? {
            self.expect(st, '(' as i32)?;
            let a = self.expression(st, false)?;
            self.expect(st, ')' as i32)?;
            let b = self.statement(st)?;
            let c = if self.accept(st, TK_ELSE)? {
                self.statement(st)?
            } else {
                AST_NONE
            };
            stm = self.newnode(AstType::StmIf, line, a, b, c, AST_NONE);
        } else if self.accept(st, TK_DO)? {
            let a = self.statement(st)?;
            self.expect(st, TK_WHILE)?;
            self.expect(st, '(' as i32)?;
            let b = self.expression(st, false)?;
            self.expect(st, ')' as i32)?;
            self.semicolon(st)?;
            stm = self.newnode(AstType::StmDo, line, a, b, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_WHILE)? {
            self.expect(st, '(' as i32)?;
            let a = self.expression(st, false)?;
            self.expect(st, ')' as i32)?;
            let b = self.statement(st)?;
            stm = self.newnode(AstType::StmWhile, line, a, b, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_FOR)? {
            stm = self.forstatement(st, line)?;
        } else if self.accept(st, TK_CONTINUE)? {
            let a = self.identifieropt(st)?;
            self.semicolon(st)?;
            stm = self.newnode(AstType::StmContinue, line, a, AST_NONE, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_BREAK)? {
            let a = self.identifieropt(st)?;
            self.semicolon(st)?;
            stm = self.newnode(AstType::StmBreak, line, a, AST_NONE, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_RETURN)? {
            let a = if self.lookahead != ';' as i32
                && self.lookahead != '}' as i32
                && self.lookahead != 0
            {
                self.expression(st, false)?
            } else {
                AST_NONE
            };
            self.semicolon(st)?;
            stm = self.newnode(AstType::StmReturn, line, a, AST_NONE, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_WITH)? {
            self.expect(st, '(' as i32)?;
            let a = self.expression(st, false)?;
            self.expect(st, ')' as i32)?;
            let b = self.statement(st)?;
            stm = self.newnode(AstType::StmWith, line, a, b, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_SWITCH)? {
            self.expect(st, '(' as i32)?;
            let a = self.expression(st, false)?;
            self.expect(st, ')' as i32)?;
            self.expect(st, '{' as i32)?;
            let b = self.caselist(st)?;
            self.expect(st, '}' as i32)?;
            stm = self.newnode(AstType::StmSwitch, line, a, b, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_THROW)? {
            let a = self.expression(st, false)?;
            self.semicolon(st)?;
            stm = self.newnode(AstType::StmThrow, line, a, AST_NONE, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_TRY)? {
            let a = self.block(st)?;
            let mut b = AST_NONE;
            let mut c = AST_NONE;
            let mut d = AST_NONE;
            if self.accept(st, TK_CATCH)? {
                self.expect(st, '(' as i32)?;
                b = self.identifier(st)?;
                self.expect(st, ')' as i32)?;
                c = self.block(st)?;
            }
            if self.accept(st, TK_FINALLY)? {
                d = self.block(st)?;
            }
            if b == AST_NONE && d == AST_NONE {
                let msg = format!(
                    "unexpected token in try: {} (expected 'catch' or 'finally')",
                    crate::lex::tokenstring(self.lookahead)
                );
                return self.error(st, &msg);
            }
            stm = self.newnode(AstType::StmTry, line, a, b, c, d);
        } else if self.accept(st, TK_DEBUGGER)? {
            self.semicolon(st)?;
            stm = self.newnode(AstType::StmDebugger, line, AST_NONE, AST_NONE, AST_NONE, AST_NONE);
        } else if self.accept(st, TK_FUNCTION)? {
            self.warning(st, "function statements are not standard");
            stm = self.funstm(st, line)?;
        } else if self.lookahead == TK_IDENTIFIER {
            let a = self.expression(st, false)?;
            if self.node(a).typ == AstType::ExpIdentifier && self.accept(st, ':' as i32)? {
                self.node_mut(a).typ = AstType::AstIdentifier;
                let b = self.statement(st)?;
                stm = self.newnode(AstType::StmLabel, line, a, b, AST_NONE, AST_NONE);
            } else {
                self.semicolon(st)?;
                stm = a;
            }
        } else {
            stm = self.expression(st, false)?;
            self.semicolon(st)?;
        }

        self.astdepth -= 1;
        Ok(stm)
    }

    // -- program ----------------------------------------------------------------

    fn scriptelement(&mut self, st: &mut State) -> R<AstRef> {
        let line = self.lex.lexline;
        if self.accept(st, TK_FUNCTION)? {
            return self.fundec(st, line);
        }
        self.statement(st)
    }

    fn script(&mut self, st: &mut State, terminator: i32) -> R<AstRef> {
        if self.lookahead == terminator {
            return Ok(AST_NONE);
        }
        let e = self.scriptelement(st)?;
        let head = self.newnode(AstType::AstList, 0, e, AST_NONE, AST_NONE, AST_NONE);
        let mut tail = head;
        while self.lookahead != terminator {
            let e = self.scriptelement(st)?;
            let cell = self.newnode(AstType::AstList, 0, e, AST_NONE, AST_NONE, AST_NONE);
            self.node_mut(tail).b = cell;
            tail = cell;
        }
        Ok(self.list(head))
    }

    fn funbody(&mut self, st: &mut State) -> R<AstRef> {
        self.expect(st, '{' as i32)?;
        let a = self.script(st, '}' as i32)?;
        self.expect(st, '}' as i32)?;
        Ok(a)
    }

    // -- constant folding ---------------------------------------------------------

    fn setnumnode(&mut self, node: AstRef, x: f64) -> bool {
        let n = self.node_mut(node);
        n.typ = AstType::ExpNumber;
        n.number = x;
        n.a = AST_NONE;
        n.b = AST_NONE;
        n.c = AST_NONE;
        n.d = AST_NONE;
        true
    }

    fn foldconst(&mut self, node: AstRef) -> bool {
        if self.node(node).typ == AstType::AstList {
            let mut n = node;
            while n != AST_NONE {
                let a = self.node(n).a;
                self.foldconst(a);
                n = self.node(n).b;
            }
            return false;
        }

        if self.node(node).typ == AstType::ExpNumber {
            return true;
        }

        let (na, nb, nc, nd) = {
            let n = self.node(node);
            (n.a, n.b, n.c, n.d)
        };
        let a = if na != AST_NONE { self.foldconst(na) } else { false };
        let b = if nb != AST_NONE { self.foldconst(nb) } else { false };
        if nc != AST_NONE {
            self.foldconst(nc);
        }
        if nd != AST_NONE {
            self.foldconst(nd);
        }

        if a {
            let x = self.node(na).number;
            match self.node(node).typ {
                AstType::ExpNeg => return self.setnumnode(node, -x),
                AstType::ExpPos => return self.setnumnode(node, x),
                AstType::ExpBitNot => {
                    return self.setnumnode(node, !crate::number::number_to_int32(x) as f64)
                }
                _ => {}
            }

            if b {
                let y = self.node(nb).number;
                match self.node(node).typ {
                    AstType::ExpMul => return self.setnumnode(node, x * y),
                    AstType::ExpDiv => return self.setnumnode(node, x / y),
                    AstType::ExpMod => return self.setnumnode(node, x % y),
                    AstType::ExpAdd => return self.setnumnode(node, x + y),
                    AstType::ExpSub => return self.setnumnode(node, x - y),
                    AstType::ExpShl => {
                        return self.setnumnode(
                            node,
                            (crate::number::number_to_int32(x)
                                << (crate::number::number_to_uint32(y) & 0x1F))
                                as f64,
                        )
                    }
                    AstType::ExpShr => {
                        return self.setnumnode(
                            node,
                            (crate::number::number_to_int32(x)
                                >> (crate::number::number_to_uint32(y) & 0x1F))
                                as f64,
                        )
                    }
                    AstType::ExpUshr => {
                        return self.setnumnode(
                            node,
                            (crate::number::number_to_uint32(x)
                                >> (crate::number::number_to_uint32(y) & 0x1F))
                                as f64,
                        )
                    }
                    AstType::ExpBitAnd => {
                        return self.setnumnode(
                            node,
                            (crate::number::number_to_int32(x)
                                & crate::number::number_to_int32(y))
                                as f64,
                        )
                    }
                    AstType::ExpBitXor => {
                        return self.setnumnode(
                            node,
                            (crate::number::number_to_int32(x)
                                ^ crate::number::number_to_int32(y))
                                as f64,
                        )
                    }
                    AstType::ExpBitOr => {
                        return self.setnumnode(
                            node,
                            (crate::number::number_to_int32(x)
                                | crate::number::number_to_int32(y))
                                as f64,
                        )
                    }
                    _ => {}
                }
            }
        }

        false
    }
}

/// jsP_parse: parse a program.
pub fn parse(st: &mut State, filename: &str, source: &str) -> R<Ast> {
    let mut p = Parser::new(filename, source);
    p.next(st)?;
    p.astdepth = 0;
    let root = p.script(st, 0)?;
    if root != AST_NONE {
        p.foldconst(root);
    }
    Ok(Ast {
        nodes: p.nodes,
        root,
        filename: filename.to_string(),
    })
}

/// jsP_parsefunction: parse the Function constructor's parameter list and body.
pub fn parse_function(
    st: &mut State,
    filename: &str,
    params: Option<&str>,
    body: &str,
) -> R<Ast> {
    let mut p = Parser::new(filename, params.unwrap_or(body));
    let params_root = if params.is_some() {
        p.next(st)?;
        p.astdepth = 0;
        p.parameters(st)?
    } else {
        AST_NONE
    };

    // re-initialize the lexer for the body, keeping the same arena
    p.lex = Lexer::new(filename, body);
    p.next(st)?;
    p.astdepth = 0;
    let body_root = p.script(st, 0)?;
    if body_root != AST_NONE {
        p.foldconst(body_root);
    }

    let root = p.newnode(AstType::ExpFun, 0, AST_NONE, params_root, body_root, AST_NONE);
    Ok(Ast {
        nodes: p.nodes,
        root,
        filename: filename.to_string(),
    })
}
