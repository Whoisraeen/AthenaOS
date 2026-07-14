//! The ECMAScript parser: a [`Token`] stream → a [`Program`] AST.
//!
//! Statements are parsed by recursive descent; expressions by a Pratt
//! (precedence-climbing) loop ([`Parser::parse_binary`]) so the operator
//! precedence/associativity table is data, not a tower of methods. Every recursive
//! entry bumps a depth counter checked against [`MAX_DEPTH`] and a node counter checked
//! against [`MAX_NODES`], so hostile input is refused with a positioned [`JsError`]
//! rather than overflowing the stack or hanging.
//!
//! **ASI** (Automatic Semicolon Insertion) is handled with three rules from the spec:
//! a statement may end at `}` or EOF; a missing `;` is inserted when the next token is
//! on a new line ([`Token::newline_before`]); and `return`/`break`/`continue`/postfix
//! `++`/`--` are "restricted productions" — a line terminator there forces termination.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lexer::{Keyword, Punct, Token, TokenKind};
use crate::{
    ArrayElement, AssignOp, BinaryOp, CatchClause, Class, ClassMember, ClassMemberKind, Expr,
    ForInit, Function, JsError, LogicalOp, MemberProp, ObjectPatternProp, ObjectProp, Param,
    Pattern, Program, PropertyKey, Stmt, SwitchCase, UnaryOp, UpdateOp, VarDeclarator, VarKind,
    MAX_DEPTH, MAX_NODES,
};

pub struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    depth: usize,
    nodes: usize,
}

impl<'a> Parser<'a> {
    pub fn new(toks: &'a [Token]) -> Self {
        Parser {
            toks,
            pos: 0,
            depth: 0,
            nodes: 0,
        }
    }

    // ─── token cursor ────────────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        // The stream always ends in Eof, so the last token is a safe fallback.
        match self.toks.get(self.pos) {
            Some(t) => t,
            None => self.eof_tok(),
        }
    }

    fn peek_at(&self, n: usize) -> &Token {
        match self.toks.get(self.pos + n) {
            Some(t) => t,
            None => self.eof_tok(),
        }
    }

    fn eof_tok(&self) -> &Token {
        // The lexer always pushes a trailing Eof, so the slice is non-empty; fall back
        // to index 0 defensively if it were ever empty (never panics).
        match self.toks.last() {
            Some(t) => t,
            None => &self.toks[0],
        }
    }

    fn kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn at_eof(&self) -> bool {
        matches!(self.kind(), TokenKind::Eof)
    }

    fn bump(&mut self) {
        if self.pos < self.toks.len().saturating_sub(1) {
            self.pos += 1;
        }
    }

    fn is_punct(&self, p: Punct) -> bool {
        matches!(self.kind(), TokenKind::Punct(x) if *x == p)
    }

    fn is_kw(&self, k: Keyword) -> bool {
        matches!(self.kind(), TokenKind::Keyword(x) if *x == k)
    }

    fn eat_punct(&mut self, p: Punct) -> bool {
        if self.is_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: Keyword) -> bool {
        if self.is_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: Punct, what: &str) -> Result<(), JsError> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(self.err_here(what))
        }
    }

    fn err_here(&self, msg: &str) -> JsError {
        let t = self.peek();
        JsError::new(msg, t.start, t.line, t.col)
    }

    fn enter(&mut self) -> Result<(), JsError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err_here("expression/statement nesting too deep"));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn node(&mut self) -> Result<(), JsError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(self.err_here("too many AST nodes"));
        }
        Ok(())
    }

    // ─── program ───────────────────────────────────────────────────────────────

    pub fn parse_program(&mut self) -> Result<Program, JsError> {
        let mut body = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            body.push(self.parse_stmt()?);
            // Defensive: every statement path must consume ≥1 token; this guarantees
            // termination even if a future edit forgets to advance.
            if self.pos == before {
                return Err(self.err_here("parser made no progress"));
            }
        }
        Ok(Program { body })
    }

    // ═══ statements ═════════════════════════════════════════════════════════

    fn parse_stmt(&mut self) -> Result<Stmt, JsError> {
        self.enter()?;
        let r = self.parse_stmt_inner();
        self.leave();
        r
    }

    fn parse_stmt_inner(&mut self) -> Result<Stmt, JsError> {
        self.node()?;
        match self.kind() {
            TokenKind::Punct(Punct::LBrace) => {
                let block = self.parse_block()?;
                Ok(Stmt::Block(block))
            }
            TokenKind::Punct(Punct::Semicolon) => {
                self.bump();
                Ok(Stmt::Empty)
            }
            TokenKind::Keyword(Keyword::Var) => self.parse_var_decl(VarKind::Var),
            TokenKind::Keyword(Keyword::Let) => self.parse_var_decl(VarKind::Let),
            TokenKind::Keyword(Keyword::Const) => self.parse_var_decl(VarKind::Const),
            TokenKind::Keyword(Keyword::If) => self.parse_if(),
            TokenKind::Keyword(Keyword::For) => self.parse_for(),
            TokenKind::Keyword(Keyword::While) => self.parse_while(),
            TokenKind::Keyword(Keyword::Do) => self.parse_do_while(),
            TokenKind::Keyword(Keyword::Switch) => self.parse_switch(),
            TokenKind::Keyword(Keyword::Break) => self.parse_break_continue(true),
            TokenKind::Keyword(Keyword::Continue) => self.parse_break_continue(false),
            TokenKind::Keyword(Keyword::Return) => self.parse_return(),
            TokenKind::Keyword(Keyword::Throw) => self.parse_throw(),
            TokenKind::Keyword(Keyword::Try) => self.parse_try(),
            TokenKind::Keyword(Keyword::Function) => {
                let f = self.parse_function(false, false)?;
                Ok(Stmt::FunctionDecl(f))
            }
            TokenKind::Keyword(Keyword::Async)
                if matches!(self.peek_at(1).kind, TokenKind::Keyword(Keyword::Function)) =>
            {
                self.bump(); // async
                let f = self.parse_function(false, true)?;
                Ok(Stmt::FunctionDecl(f))
            }
            TokenKind::Keyword(Keyword::Class) => {
                let c = self.parse_class()?;
                Ok(Stmt::ClassDecl(c))
            }
            // Labeled statement: `ident :` not followed by being part of an expression.
            TokenKind::Ident(_)
                if matches!(self.peek_at(1).kind, TokenKind::Punct(Punct::Colon)) =>
            {
                let label = self.ident_name()?;
                self.bump(); // :
                let body = self.parse_stmt()?;
                Ok(Stmt::Labeled {
                    label,
                    body: Box::new(body),
                })
            }
            _ => {
                // Expression statement.
                let expr = self.parse_expr()?;
                self.consume_semi()?;
                Ok(Stmt::Expr(expr))
            }
        }
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, JsError> {
        self.expect_punct(Punct::LBrace, "expected '{'")?;
        let mut body = Vec::new();
        while !self.is_punct(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            body.push(self.parse_stmt()?);
            if self.pos == before {
                return Err(self.err_here("parser made no progress in block"));
            }
        }
        self.expect_punct(Punct::RBrace, "expected '}'")?;
        Ok(body)
    }

    /// ASI: consume a `;`, or accept termination at `}`, EOF, or a newline boundary.
    fn consume_semi(&mut self) -> Result<(), JsError> {
        if self.eat_punct(Punct::Semicolon) {
            return Ok(());
        }
        if self.is_punct(Punct::RBrace) || self.at_eof() || self.peek().newline_before {
            return Ok(());
        }
        Err(self.err_here("expected ';' or newline"))
    }

    fn parse_var_decl(&mut self, kind: VarKind) -> Result<Stmt, JsError> {
        self.bump(); // var/let/const
        let declarations = self.parse_declarators()?;
        self.consume_semi()?;
        Ok(Stmt::VarDecl { kind, declarations })
    }

    fn parse_declarators(&mut self) -> Result<Vec<VarDeclarator>, JsError> {
        let mut out = Vec::new();
        loop {
            let target = self.parse_binding_pattern()?;
            let init = if self.eat_punct(Punct::Assign) {
                Some(self.parse_assign()?)
            } else {
                None
            };
            out.push(VarDeclarator { target, init });
            if !self.eat_punct(Punct::Comma) {
                break;
            }
        }
        Ok(out)
    }

    fn parse_if(&mut self) -> Result<Stmt, JsError> {
        self.bump(); // if
        self.expect_punct(Punct::LParen, "expected '(' after 'if'")?;
        let test = self.parse_expr()?;
        self.expect_punct(Punct::RParen, "expected ')'")?;
        let consequent = Box::new(self.parse_stmt()?);
        let alternate = if self.eat_kw(Keyword::Else) {
            Some(Box::new(self.parse_stmt()?))
        } else {
            None
        };
        Ok(Stmt::If {
            test,
            consequent,
            alternate,
        })
    }

    fn parse_for(&mut self) -> Result<Stmt, JsError> {
        self.bump(); // for
        self.expect_punct(Punct::LParen, "expected '(' after 'for'")?;

        // The init clause: empty, a declaration, or an expression. We then look for
        // `in` / `of` to choose the loop form.
        let init: Option<ForInit> = if self.is_punct(Punct::Semicolon) {
            None
        } else if self.is_kw(Keyword::Var) || self.is_kw(Keyword::Let) || self.is_kw(Keyword::Const)
        {
            let kind = match self.kind() {
                TokenKind::Keyword(Keyword::Var) => VarKind::Var,
                TokenKind::Keyword(Keyword::Let) => VarKind::Let,
                _ => VarKind::Const,
            };
            self.bump();
            // A single binding may be followed by `in`/`of`.
            let pat = self.parse_binding_pattern()?;
            if self.is_kw(Keyword::In) || self.is_kw(Keyword::Of) {
                let is_of = self.is_kw(Keyword::Of);
                self.bump();
                let right = if is_of {
                    self.parse_assign()?
                } else {
                    self.parse_expr()?
                };
                self.expect_punct(Punct::RParen, "expected ')'")?;
                let body = Box::new(self.parse_stmt()?);
                let left = Box::new(ForInit::VarDecl {
                    kind,
                    declarations: alloc::vec![VarDeclarator {
                        target: pat,
                        init: None
                    }],
                });
                return Ok(if is_of {
                    Stmt::ForOf { left, right, body }
                } else {
                    Stmt::ForIn { left, right, body }
                });
            }
            // C-style: this was the first declarator; parse its init + any more.
            let first_init = if self.eat_punct(Punct::Assign) {
                Some(self.parse_assign()?)
            } else {
                None
            };
            let mut decls = alloc::vec![VarDeclarator {
                target: pat,
                init: first_init
            }];
            while self.eat_punct(Punct::Comma) {
                let t = self.parse_binding_pattern()?;
                let i = if self.eat_punct(Punct::Assign) {
                    Some(self.parse_assign()?)
                } else {
                    None
                };
                decls.push(VarDeclarator { target: t, init: i });
            }
            Some(ForInit::VarDecl {
                kind,
                declarations: decls,
            })
        } else {
            // Expression init — may be a `for (x in/of …)` over an existing binding.
            let expr = self.parse_expr_no_in()?;
            if self.is_kw(Keyword::In) || self.is_kw(Keyword::Of) {
                let is_of = self.is_kw(Keyword::Of);
                self.bump();
                let right = if is_of {
                    self.parse_assign()?
                } else {
                    self.parse_expr()?
                };
                self.expect_punct(Punct::RParen, "expected ')'")?;
                let body = Box::new(self.parse_stmt()?);
                let left = Box::new(ForInit::Pattern(self.expr_to_pattern(expr)?));
                return Ok(if is_of {
                    Stmt::ForOf { left, right, body }
                } else {
                    Stmt::ForIn { left, right, body }
                });
            }
            Some(ForInit::Expr(expr))
        };

        // C-style for: `init ; test ; update`.
        self.expect_punct(Punct::Semicolon, "expected ';' in for-loop")?;
        let test = if self.is_punct(Punct::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect_punct(Punct::Semicolon, "expected ';' in for-loop")?;
        let update = if self.is_punct(Punct::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect_punct(Punct::RParen, "expected ')'")?;
        let body = Box::new(self.parse_stmt()?);
        Ok(Stmt::For {
            init: init.map(Box::new),
            test,
            update,
            body,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, JsError> {
        self.bump();
        self.expect_punct(Punct::LParen, "expected '(' after 'while'")?;
        let test = self.parse_expr()?;
        self.expect_punct(Punct::RParen, "expected ')'")?;
        let body = Box::new(self.parse_stmt()?);
        Ok(Stmt::While { test, body })
    }

    fn parse_do_while(&mut self) -> Result<Stmt, JsError> {
        self.bump();
        let body = Box::new(self.parse_stmt()?);
        if !self.eat_kw(Keyword::While) {
            return Err(self.err_here("expected 'while' after 'do' body"));
        }
        self.expect_punct(Punct::LParen, "expected '('")?;
        let test = self.parse_expr()?;
        self.expect_punct(Punct::RParen, "expected ')'")?;
        let _ = self.eat_punct(Punct::Semicolon); // optional
        Ok(Stmt::DoWhile { body, test })
    }

    fn parse_switch(&mut self) -> Result<Stmt, JsError> {
        self.bump();
        self.expect_punct(Punct::LParen, "expected '(' after 'switch'")?;
        let discriminant = self.parse_expr()?;
        self.expect_punct(Punct::RParen, "expected ')'")?;
        self.expect_punct(Punct::LBrace, "expected '{'")?;
        let mut cases = Vec::new();
        while !self.is_punct(Punct::RBrace) && !self.at_eof() {
            let test = if self.eat_kw(Keyword::Case) {
                let e = self.parse_expr()?;
                Some(e)
            } else if self.eat_kw(Keyword::Default) {
                None
            } else {
                return Err(self.err_here("expected 'case' or 'default'"));
            };
            self.expect_punct(Punct::Colon, "expected ':' after case label")?;
            let mut body = Vec::new();
            while !self.is_punct(Punct::RBrace)
                && !self.is_kw(Keyword::Case)
                && !self.is_kw(Keyword::Default)
                && !self.at_eof()
            {
                let before = self.pos;
                body.push(self.parse_stmt()?);
                if self.pos == before {
                    return Err(self.err_here("parser made no progress in switch"));
                }
            }
            cases.push(SwitchCase { test, body });
        }
        self.expect_punct(Punct::RBrace, "expected '}'")?;
        Ok(Stmt::Switch {
            discriminant,
            cases,
        })
    }

    fn parse_break_continue(&mut self, is_break: bool) -> Result<Stmt, JsError> {
        self.bump();
        // Restricted production: a label is only consumed if on the same line.
        let label = if !self.peek().newline_before {
            if let TokenKind::Ident(_) = self.kind() {
                Some(self.ident_name()?)
            } else {
                None
            }
        } else {
            None
        };
        self.consume_semi()?;
        Ok(if is_break {
            Stmt::Break(label)
        } else {
            Stmt::Continue(label)
        })
    }

    fn parse_return(&mut self) -> Result<Stmt, JsError> {
        self.bump();
        // Restricted production: `return` <newline> means `return;` (undefined).
        let arg = if self.peek().newline_before
            || self.is_punct(Punct::Semicolon)
            || self.is_punct(Punct::RBrace)
            || self.at_eof()
        {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.consume_semi()?;
        Ok(Stmt::Return(arg))
    }

    fn parse_throw(&mut self) -> Result<Stmt, JsError> {
        self.bump();
        if self.peek().newline_before {
            return Err(self.err_here("illegal newline after 'throw'"));
        }
        let arg = self.parse_expr()?;
        self.consume_semi()?;
        Ok(Stmt::Throw(arg))
    }

    fn parse_try(&mut self) -> Result<Stmt, JsError> {
        self.bump();
        let block = self.parse_block()?;
        let handler = if self.eat_kw(Keyword::Catch) {
            let param = if self.eat_punct(Punct::LParen) {
                let p = self.parse_binding_pattern()?;
                self.expect_punct(Punct::RParen, "expected ')'")?;
                Some(p)
            } else {
                None
            };
            let body = self.parse_block()?;
            Some(CatchClause { param, body })
        } else {
            None
        };
        let finalizer = if self.eat_kw(Keyword::Finally) {
            Some(self.parse_block()?)
        } else {
            None
        };
        if handler.is_none() && finalizer.is_none() {
            return Err(self.err_here("'try' needs 'catch' or 'finally'"));
        }
        Ok(Stmt::Try {
            block,
            handler,
            finalizer,
        })
    }

    // ═══ functions & classes ════════════════════════════════════════════════

    fn parse_function(&mut self, force_anon: bool, is_async: bool) -> Result<Function, JsError> {
        self.bump(); // function
        let is_generator = self.eat_punct(Punct::Star);
        let name = if !force_anon {
            if let TokenKind::Ident(_) = self.kind() {
                Some(self.ident_name()?)
            } else {
                None
            }
        } else {
            None
        };
        let params = self.parse_params()?;
        let body = self.parse_block()?;
        Ok(Function {
            name,
            params,
            body,
            is_arrow: false,
            is_generator,
            is_async,
            arrow_expr: None,
        })
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, JsError> {
        self.expect_punct(Punct::LParen, "expected '(' for parameters")?;
        let mut params = Vec::new();
        while !self.is_punct(Punct::RParen) && !self.at_eof() {
            let rest = self.eat_punct(Punct::DotDotDot);
            let mut pattern = self.parse_binding_pattern()?;
            if !rest && self.eat_punct(Punct::Assign) {
                let default = self.parse_assign()?;
                pattern = Pattern::Default {
                    target: Box::new(pattern),
                    default: Box::new(default),
                };
            }
            params.push(Param { pattern, rest });
            if rest {
                break;
            }
            if !self.eat_punct(Punct::Comma) {
                break;
            }
        }
        self.expect_punct(Punct::RParen, "expected ')'")?;
        Ok(params)
    }

    fn parse_class(&mut self) -> Result<Class, JsError> {
        self.bump(); // class
        let name = if let TokenKind::Ident(_) = self.kind() {
            Some(self.ident_name()?)
        } else {
            None
        };
        let super_class = if self.eat_kw(Keyword::Extends) {
            Some(Box::new(self.parse_lhs_expr()?))
        } else {
            None
        };
        self.expect_punct(Punct::LBrace, "expected '{' for class body")?;
        let mut members = Vec::new();
        while !self.is_punct(Punct::RBrace) && !self.at_eof() {
            if self.eat_punct(Punct::Semicolon) {
                continue;
            }
            let before = self.pos;
            members.push(self.parse_class_member()?);
            if self.pos == before {
                return Err(self.err_here("parser made no progress in class body"));
            }
        }
        self.expect_punct(Punct::RBrace, "expected '}'")?;
        Ok(Class {
            name,
            super_class,
            members,
        })
    }

    fn parse_class_member(&mut self) -> Result<ClassMember, JsError> {
        self.node()?;
        let is_static = self.is_kw(Keyword::Static)
            && !matches!(
                self.peek_at(1).kind,
                TokenKind::Punct(Punct::LParen | Punct::Assign)
            );
        if is_static {
            self.bump();
        }

        let is_async = self.is_kw(Keyword::Async)
            && !matches!(
                self.peek_at(1).kind,
                TokenKind::Punct(Punct::LParen | Punct::Assign | Punct::Colon)
            );
        if is_async {
            self.bump();
        }
        let is_generator = self.eat_punct(Punct::Star);

        // get/set accessor — only if followed by a key (not '(' which would make
        // `get` itself the method name).
        let mut kind = ClassMemberKind::Method;
        if self.is_kw(Keyword::Get) && !self.next_is_method_open() {
            self.bump();
            kind = ClassMemberKind::Getter;
        } else if self.is_kw(Keyword::Set) && !self.next_is_method_open() {
            self.bump();
            kind = ClassMemberKind::Setter;
        }

        let key = self.parse_property_key()?;

        if self.is_punct(Punct::LParen) {
            // Method.
            if let PropertyKey::Ident(ref n) = key {
                if n == "constructor" && kind == ClassMemberKind::Method && !is_static {
                    kind = ClassMemberKind::Constructor;
                }
            }
            let params = self.parse_params()?;
            let body = self.parse_block()?;
            Ok(ClassMember {
                key,
                kind,
                is_static,
                value: Some(Function {
                    name: None,
                    params,
                    body,
                    is_arrow: false,
                    is_generator,
                    is_async,
                    arrow_expr: None,
                }),
                field_init: None,
            })
        } else {
            // Field declaration.
            let field_init = if self.eat_punct(Punct::Assign) {
                Some(self.parse_assign()?)
            } else {
                None
            };
            self.consume_semi()?;
            Ok(ClassMember {
                key,
                kind: ClassMemberKind::Field,
                is_static,
                value: None,
                field_init,
            })
        }
    }

    /// Is the current token a method-open (`(`)? Used to decide whether `get`/`set`/
    /// `async` is a modifier or a plain member name.
    fn next_is_method_open(&self) -> bool {
        matches!(self.peek_at(1).kind, TokenKind::Punct(Punct::LParen))
    }

    fn parse_property_key(&mut self) -> Result<PropertyKey, JsError> {
        match self.kind() {
            TokenKind::Punct(Punct::LBracket) => {
                self.bump();
                let e = self.parse_assign()?;
                self.expect_punct(Punct::RBracket, "expected ']'")?;
                Ok(PropertyKey::Computed(Box::new(e)))
            }
            TokenKind::String(s) => {
                let s = s.clone();
                self.bump();
                Ok(PropertyKey::String(s))
            }
            TokenKind::Number(n) => {
                let n = *n;
                self.bump();
                Ok(PropertyKey::Number(n))
            }
            TokenKind::Ident(_) | TokenKind::Keyword(_) => {
                let name = self.ident_like_name()?;
                Ok(PropertyKey::Ident(name))
            }
            _ => Err(self.err_here("expected property key")),
        }
    }

    // ═══ patterns (declaration / destructuring) ═══════════════════════════════

    fn parse_binding_pattern(&mut self) -> Result<Pattern, JsError> {
        self.enter()?;
        let r = self.parse_binding_pattern_inner();
        self.leave();
        r
    }

    fn parse_binding_pattern_inner(&mut self) -> Result<Pattern, JsError> {
        self.node()?;
        match self.kind() {
            TokenKind::Punct(Punct::LBracket) => self.parse_array_pattern(),
            TokenKind::Punct(Punct::LBrace) => self.parse_object_pattern(),
            TokenKind::Ident(_) => Ok(Pattern::Ident(self.ident_name()?)),
            // Some contextual keywords are valid binding names.
            TokenKind::Keyword(Keyword::Of)
            | TokenKind::Keyword(Keyword::Async)
            | TokenKind::Keyword(Keyword::Await)
            | TokenKind::Keyword(Keyword::Yield)
            | TokenKind::Keyword(Keyword::Get)
            | TokenKind::Keyword(Keyword::Set)
            | TokenKind::Keyword(Keyword::Static) => Ok(Pattern::Ident(self.ident_like_name()?)),
            _ => Err(self.err_here("expected a binding name or destructuring pattern")),
        }
    }

    fn parse_array_pattern(&mut self) -> Result<Pattern, JsError> {
        self.bump(); // [
        let mut elements = Vec::new();
        let mut rest = None;
        while !self.is_punct(Punct::RBracket) && !self.at_eof() {
            if self.eat_punct(Punct::Comma) {
                elements.push(None); // elision
                continue;
            }
            if self.eat_punct(Punct::DotDotDot) {
                rest = Some(Box::new(self.parse_binding_pattern()?));
                break;
            }
            let mut p = self.parse_binding_pattern()?;
            if self.eat_punct(Punct::Assign) {
                let default = self.parse_assign()?;
                p = Pattern::Default {
                    target: Box::new(p),
                    default: Box::new(default),
                };
            }
            elements.push(Some(p));
            if !self.eat_punct(Punct::Comma) {
                break;
            }
        }
        self.expect_punct(Punct::RBracket, "expected ']' in array pattern")?;
        Ok(Pattern::Array { elements, rest })
    }

    fn parse_object_pattern(&mut self) -> Result<Pattern, JsError> {
        self.bump(); // {
        let mut properties = Vec::new();
        let mut rest = None;
        while !self.is_punct(Punct::RBrace) && !self.at_eof() {
            if self.eat_punct(Punct::DotDotDot) {
                rest = Some(Box::new(self.parse_binding_pattern()?));
                break;
            }
            let key = self.parse_property_key()?;
            let (value, shorthand) = if self.eat_punct(Punct::Colon) {
                (self.parse_binding_pattern()?, false)
            } else {
                // Shorthand: the key must be an identifier reused as the binding.
                match &key {
                    PropertyKey::Ident(n) => (Pattern::Ident(n.clone()), true),
                    _ => return Err(self.err_here("invalid shorthand in object pattern")),
                }
            };
            let value = if self.eat_punct(Punct::Assign) {
                let default = self.parse_assign()?;
                Pattern::Default {
                    target: Box::new(value),
                    default: Box::new(default),
                }
            } else {
                value
            };
            properties.push(ObjectPatternProp {
                key,
                value,
                shorthand,
            });
            if !self.eat_punct(Punct::Comma) {
                break;
            }
        }
        self.expect_punct(Punct::RBrace, "expected '}' in object pattern")?;
        Ok(Pattern::Object { properties, rest })
    }

    /// Convert an already-parsed expression into an assignment pattern (used for
    /// `for (x of …)` and `[a, b] = …` targets). Only valid target shapes succeed.
    fn expr_to_pattern(&self, e: Expr) -> Result<Pattern, JsError> {
        match e {
            Expr::Ident(n) => Ok(Pattern::Ident(n)),
            Expr::Member { .. } => Ok(Pattern::Member(Box::new(e))),
            _ => Err(self.err_here("invalid assignment/loop target")),
        }
    }

    // ═══ expressions ════════════════════════════════════════════════════════

    fn parse_expr(&mut self) -> Result<Expr, JsError> {
        let first = self.parse_assign()?;
        if self.is_punct(Punct::Comma) {
            let mut seq = alloc::vec![first];
            while self.eat_punct(Punct::Comma) {
                seq.push(self.parse_assign()?);
            }
            Ok(Expr::Sequence(seq))
        } else {
            Ok(first)
        }
    }

    /// Parse an expression but stop before a top-level `in` (for `for (… in …)` heads).
    /// We approximate by parsing a single assignment expression (no comma sequence),
    /// which never consumes the loop's `in`/`of` because those are not assignment ops.
    fn parse_expr_no_in(&mut self) -> Result<Expr, JsError> {
        self.parse_assign_with_in(false)
    }

    fn parse_assign(&mut self) -> Result<Expr, JsError> {
        self.parse_assign_with_in(true)
    }

    fn parse_assign_with_in(&mut self, allow_in: bool) -> Result<Expr, JsError> {
        self.enter()?;
        let r = self.parse_assign_inner(allow_in);
        self.leave();
        r
    }

    fn parse_assign_inner(&mut self, allow_in: bool) -> Result<Expr, JsError> {
        // Arrow-function lookahead: `ident =>` or `( … ) =>` or `async ident =>`.
        if let Some(arrow) = self.try_parse_arrow()? {
            return Ok(arrow);
        }

        let left = self.parse_conditional(allow_in)?;
        if let Some(op) = self.assign_op() {
            self.bump();
            self.node()?;
            let value = self.parse_assign_with_in(allow_in)?;
            return Ok(Expr::Assign {
                op,
                target: Box::new(left),
                value: Box::new(value),
            });
        }
        Ok(left)
    }

    fn assign_op(&self) -> Option<AssignOp> {
        Some(match self.kind() {
            TokenKind::Punct(Punct::Assign) => AssignOp::Assign,
            TokenKind::Punct(Punct::PlusEq) => AssignOp::Add,
            TokenKind::Punct(Punct::MinusEq) => AssignOp::Sub,
            TokenKind::Punct(Punct::StarEq) => AssignOp::Mul,
            TokenKind::Punct(Punct::SlashEq) => AssignOp::Div,
            TokenKind::Punct(Punct::PercentEq) => AssignOp::Mod,
            TokenKind::Punct(Punct::StarStarEq) => AssignOp::Exp,
            TokenKind::Punct(Punct::ShlEq) => AssignOp::Shl,
            TokenKind::Punct(Punct::ShrEq) => AssignOp::Shr,
            TokenKind::Punct(Punct::UShrEq) => AssignOp::UShr,
            TokenKind::Punct(Punct::AmpEq) => AssignOp::BitAnd,
            TokenKind::Punct(Punct::PipeEq) => AssignOp::BitOr,
            TokenKind::Punct(Punct::CaretEq) => AssignOp::BitXor,
            TokenKind::Punct(Punct::AmpAmpEq) => AssignOp::And,
            TokenKind::Punct(Punct::PipePipeEq) => AssignOp::Or,
            TokenKind::Punct(Punct::QuestionQuestionEq) => AssignOp::Nullish,
            _ => return None,
        })
    }

    fn parse_conditional(&mut self, allow_in: bool) -> Result<Expr, JsError> {
        let test = self.parse_binary(0, allow_in)?;
        if self.is_punct(Punct::Question) {
            // Disambiguate from `?.` — that's a single OptionalChain token, so a bare
            // `?` here is always the conditional operator.
            self.bump();
            self.node()?;
            let consequent = self.parse_assign()?;
            self.expect_punct(Punct::Colon, "expected ':' in conditional")?;
            let alternate = self.parse_assign_with_in(allow_in)?;
            return Ok(Expr::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            });
        }
        Ok(test)
    }

    /// Pratt loop over binary/logical operators. `min_bp` is the minimum binding power
    /// this call will consume. Right-associative `**` is handled by recursing with the
    /// same bp; everything else is left-associative (recurse with `bp + 1`).
    fn parse_binary(&mut self, min_bp: u8, allow_in: bool) -> Result<Expr, JsError> {
        self.enter()?;
        let mut left = self.parse_unary()?;
        loop {
            let (op, bp, right_assoc) = match self.binary_op(allow_in) {
                Some(x) => x,
                None => break,
            };
            if bp < min_bp {
                break;
            }
            self.bump();
            self.node()?;
            let next_min = if right_assoc { bp } else { bp + 1 };
            let right = self.parse_binary(next_min, allow_in)?;
            left = self.make_binary(op, left, right);
        }
        self.leave();
        Ok(left)
    }

    fn make_binary(&self, op: BinOp, left: Expr, right: Expr) -> Expr {
        match op {
            BinOp::Logical(l) => Expr::Logical {
                op: l,
                left: Box::new(left),
                right: Box::new(right),
            },
            BinOp::Binary(b) => Expr::Binary {
                op: b,
                left: Box::new(left),
                right: Box::new(right),
            },
        }
    }

    /// Map the current token to a binary operator with its (binding-power, assoc).
    /// Higher binding power binds tighter. Precedence follows the ECMAScript grammar:
    /// `??`(1) `||`(2) `&&`(3) `|`(4) `^`(5) `&`(6) eq(7) rel(8) shift(9) add(10)
    /// mul(11) `**`(12, right-assoc).
    fn binary_op(&self, allow_in: bool) -> Option<(BinOp, u8, bool)> {
        use BinaryOp as B;
        use LogicalOp as L;
        let p = match self.kind() {
            TokenKind::Punct(p) => *p,
            TokenKind::Keyword(Keyword::Instanceof) => {
                return Some((BinOp::Binary(B::Instanceof), 8, false))
            }
            TokenKind::Keyword(Keyword::In) if allow_in => {
                return Some((BinOp::Binary(B::In), 8, false))
            }
            _ => return None,
        };
        Some(match p {
            Punct::QuestionQuestion => (BinOp::Logical(L::Nullish), 1, false),
            Punct::PipePipe => (BinOp::Logical(L::Or), 2, false),
            Punct::AmpAmp => (BinOp::Logical(L::And), 3, false),
            Punct::Pipe => (BinOp::Binary(B::BitOr), 4, false),
            Punct::Caret => (BinOp::Binary(B::BitXor), 5, false),
            Punct::Amp => (BinOp::Binary(B::BitAnd), 6, false),
            Punct::EqEq => (BinOp::Binary(B::EqEq), 7, false),
            Punct::NotEq => (BinOp::Binary(B::NotEq), 7, false),
            Punct::EqEqEq => (BinOp::Binary(B::EqEqEq), 7, false),
            Punct::NotEqEq => (BinOp::Binary(B::NotEqEq), 7, false),
            Punct::Lt => (BinOp::Binary(B::Lt), 8, false),
            Punct::Gt => (BinOp::Binary(B::Gt), 8, false),
            Punct::LtEq => (BinOp::Binary(B::LtEq), 8, false),
            Punct::GtEq => (BinOp::Binary(B::GtEq), 8, false),
            Punct::Shl => (BinOp::Binary(B::Shl), 9, false),
            Punct::Shr => (BinOp::Binary(B::Shr), 9, false),
            Punct::UShr => (BinOp::Binary(B::UShr), 9, false),
            Punct::Plus => (BinOp::Binary(B::Add), 10, false),
            Punct::Minus => (BinOp::Binary(B::Sub), 10, false),
            Punct::Star => (BinOp::Binary(B::Mul), 11, false),
            Punct::Slash => (BinOp::Binary(B::Div), 11, false),
            Punct::Percent => (BinOp::Binary(B::Mod), 11, false),
            Punct::StarStar => (BinOp::Binary(B::Exp), 12, true),
            _ => return None,
        })
    }

    fn parse_unary(&mut self) -> Result<Expr, JsError> {
        self.enter()?;
        let r = self.parse_unary_inner();
        self.leave();
        r
    }

    fn parse_unary_inner(&mut self) -> Result<Expr, JsError> {
        // Prefix unary operators.
        let unop = match self.kind() {
            TokenKind::Punct(Punct::Not) => Some(UnaryOp::Not),
            TokenKind::Punct(Punct::Minus) => Some(UnaryOp::Neg),
            TokenKind::Punct(Punct::Plus) => Some(UnaryOp::Pos),
            TokenKind::Punct(Punct::Tilde) => Some(UnaryOp::BitNot),
            TokenKind::Keyword(Keyword::Typeof) => Some(UnaryOp::Typeof),
            TokenKind::Keyword(Keyword::Void) => Some(UnaryOp::Void),
            TokenKind::Keyword(Keyword::Delete) => Some(UnaryOp::Delete),
            _ => None,
        };
        if let Some(op) = unop {
            self.bump();
            self.node()?;
            let operand = self.parse_unary()?;
            return Ok(Expr::Unary {
                op,
                operand: Box::new(operand),
            });
        }
        // Prefix ++ / --.
        if self.is_punct(Punct::PlusPlus) || self.is_punct(Punct::MinusMinus) {
            let op = if self.is_punct(Punct::PlusPlus) {
                UpdateOp::Inc
            } else {
                UpdateOp::Dec
            };
            self.bump();
            self.node()?;
            let operand = self.parse_unary()?;
            return Ok(Expr::Update {
                op,
                prefix: true,
                operand: Box::new(operand),
            });
        }
        // `await x` — its own AST node so the value flows through (see `Expr::Await`); it is
        // NOT `void` (which discards the operand → the old fake-green `undefined` bug).
        if self.is_kw(Keyword::Await) && !self.next_is_value_break() {
            self.bump();
            let operand = self.parse_unary()?;
            return Ok(Expr::Await {
                operand: Box::new(operand),
            });
        }

        let mut e = self.parse_lhs_expr()?;
        // Postfix ++ / -- (restricted: no newline before).
        if (self.is_punct(Punct::PlusPlus) || self.is_punct(Punct::MinusMinus))
            && !self.peek().newline_before
        {
            let op = if self.is_punct(Punct::PlusPlus) {
                UpdateOp::Inc
            } else {
                UpdateOp::Dec
            };
            self.bump();
            self.node()?;
            e = Expr::Update {
                op,
                prefix: false,
                operand: Box::new(e),
            };
        }
        Ok(e)
    }

    /// Heuristic: would the next token break a value (so a bare `await`/`yield` is an
    /// identifier rather than an operator)? Used only for the contextual keywords.
    fn next_is_value_break(&self) -> bool {
        matches!(
            self.peek_at(1).kind,
            TokenKind::Punct(
                Punct::Semicolon
                    | Punct::RParen
                    | Punct::RBracket
                    | Punct::RBrace
                    | Punct::Comma
                    | Punct::Colon
                    | Punct::Assign
                    | Punct::Dot
            ) | TokenKind::Eof
        )
    }

    /// Left-hand-side expression: `new`, member access, optional chaining and calls.
    fn parse_lhs_expr(&mut self) -> Result<Expr, JsError> {
        self.enter()?;
        let r = self.parse_lhs_inner();
        self.leave();
        r
    }

    fn parse_lhs_inner(&mut self) -> Result<Expr, JsError> {
        let mut expr = if self.is_kw(Keyword::New) {
            self.parse_new()?
        } else {
            self.parse_primary()?
        };

        loop {
            if self.eat_punct(Punct::Dot) {
                let name = self.ident_like_name()?;
                self.node()?;
                expr = Expr::Member {
                    object: Box::new(expr),
                    property: Box::new(MemberProp::Ident(name)),
                    optional: false,
                };
            } else if self.is_punct(Punct::OptionalChain) {
                self.bump();
                self.node()?;
                if self.is_punct(Punct::LParen) {
                    let args = self.parse_args()?;
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                        optional: true,
                    };
                } else if self.eat_punct(Punct::LBracket) {
                    let prop = self.parse_expr()?;
                    self.expect_punct(Punct::RBracket, "expected ']'")?;
                    expr = Expr::Member {
                        object: Box::new(expr),
                        property: Box::new(MemberProp::Computed(prop)),
                        optional: true,
                    };
                } else {
                    let name = self.ident_like_name()?;
                    expr = Expr::Member {
                        object: Box::new(expr),
                        property: Box::new(MemberProp::Ident(name)),
                        optional: true,
                    };
                }
            } else if self.eat_punct(Punct::LBracket) {
                let prop = self.parse_expr()?;
                self.expect_punct(Punct::RBracket, "expected ']'")?;
                self.node()?;
                expr = Expr::Member {
                    object: Box::new(expr),
                    property: Box::new(MemberProp::Computed(prop)),
                    optional: false,
                };
            } else if self.is_punct(Punct::LParen) {
                let args = self.parse_args()?;
                self.node()?;
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                    optional: false,
                };
            } else if matches!(self.kind(), TokenKind::Template { .. }) {
                // Tagged template.
                let (quasis, expressions) = self.parse_template_parts()?;
                expr = Expr::TaggedTemplate {
                    tag: Box::new(expr),
                    quasis,
                    expressions,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_new(&mut self) -> Result<Expr, JsError> {
        self.bump(); // new
                     // `new.target` meta-property — recognize shape, model as a member.
        if self.is_punct(Punct::Dot) {
            self.bump();
            let name = self.ident_like_name()?;
            return Ok(Expr::Member {
                object: Box::new(Expr::Ident(String::from("new"))),
                property: Box::new(MemberProp::Ident(name)),
                optional: false,
            });
        }
        let callee = if self.is_kw(Keyword::New) {
            self.parse_new()?
        } else {
            self.parse_primary()?
        };
        // Member accesses bind to the new-callee before the argument list.
        let mut callee = callee;
        loop {
            if self.eat_punct(Punct::Dot) {
                let name = self.ident_like_name()?;
                callee = Expr::Member {
                    object: Box::new(callee),
                    property: Box::new(MemberProp::Ident(name)),
                    optional: false,
                };
            } else if self.eat_punct(Punct::LBracket) {
                let prop = self.parse_expr()?;
                self.expect_punct(Punct::RBracket, "expected ']'")?;
                callee = Expr::Member {
                    object: Box::new(callee),
                    property: Box::new(MemberProp::Computed(prop)),
                    optional: false,
                };
            } else {
                break;
            }
        }
        let args = if self.is_punct(Punct::LParen) {
            self.parse_args()?
        } else {
            Vec::new()
        };
        Ok(Expr::New {
            callee: Box::new(callee),
            args,
        })
    }

    fn parse_args(&mut self) -> Result<Vec<ArrayElement>, JsError> {
        self.expect_punct(Punct::LParen, "expected '('")?;
        let mut args = Vec::new();
        while !self.is_punct(Punct::RParen) && !self.at_eof() {
            if self.eat_punct(Punct::DotDotDot) {
                args.push(ArrayElement::Spread(self.parse_assign()?));
            } else {
                args.push(ArrayElement::Expr(self.parse_assign()?));
            }
            if !self.eat_punct(Punct::Comma) {
                break;
            }
        }
        self.expect_punct(Punct::RParen, "expected ')'")?;
        Ok(args)
    }

    // ═══ primary expressions ═════════════════════════════════════════════════

    fn parse_primary(&mut self) -> Result<Expr, JsError> {
        self.enter()?;
        let r = self.parse_primary_inner();
        self.leave();
        r
    }

    fn parse_primary_inner(&mut self) -> Result<Expr, JsError> {
        self.node()?;
        match self.kind().clone() {
            TokenKind::Number(n) => {
                self.bump();
                Ok(Expr::Number(n))
            }
            TokenKind::BigInt(s) => {
                self.bump();
                Ok(Expr::BigInt(s))
            }
            TokenKind::String(s) => {
                self.bump();
                Ok(Expr::String(s))
            }
            TokenKind::Regex(p, f) => {
                self.bump();
                Ok(Expr::Regex {
                    pattern: p,
                    flags: f,
                })
            }
            TokenKind::Template { .. } => {
                let (quasis, expressions) = self.parse_template_parts()?;
                Ok(Expr::Template {
                    quasis,
                    expressions,
                })
            }
            TokenKind::Keyword(Keyword::True) => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            TokenKind::Keyword(Keyword::False) => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            TokenKind::Keyword(Keyword::Null) => {
                self.bump();
                Ok(Expr::Null)
            }
            TokenKind::Keyword(Keyword::This) => {
                self.bump();
                Ok(Expr::This)
            }
            TokenKind::Keyword(Keyword::Super) => {
                self.bump();
                Ok(Expr::Super)
            }
            TokenKind::Keyword(Keyword::Function) => {
                let f = self.parse_function(false, false)?;
                Ok(Expr::Function(f))
            }
            TokenKind::Keyword(Keyword::Async)
                if matches!(self.peek_at(1).kind, TokenKind::Keyword(Keyword::Function)) =>
            {
                self.bump();
                let f = self.parse_function(false, true)?;
                Ok(Expr::Function(f))
            }
            TokenKind::Keyword(Keyword::Class) => {
                let c = self.parse_class()?;
                Ok(Expr::Class(c))
            }
            TokenKind::Ident(name) => {
                self.bump();
                if name == "undefined" {
                    Ok(Expr::Undefined)
                } else {
                    Ok(Expr::Ident(name))
                }
            }
            // Contextual keywords usable as identifiers in expression position.
            TokenKind::Keyword(
                Keyword::Of
                | Keyword::Async
                | Keyword::Await
                | Keyword::Yield
                | Keyword::Get
                | Keyword::Set
                | Keyword::Static,
            ) => {
                let n = self.ident_like_name()?;
                Ok(Expr::Ident(n))
            }
            TokenKind::Punct(Punct::LParen) => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect_punct(Punct::RParen, "expected ')'")?;
                Ok(e)
            }
            TokenKind::Punct(Punct::LBracket) => self.parse_array_literal(),
            TokenKind::Punct(Punct::LBrace) => self.parse_object_literal(),
            _ => Err(self.err_here("expected an expression")),
        }
    }

    fn parse_template_parts(&mut self) -> Result<(Vec<String>, Vec<Expr>), JsError> {
        // The current token is a Template; re-parse each ${} source slice.
        let (quasis, sources) = match self.kind() {
            TokenKind::Template {
                quasis,
                expr_sources,
            } => (quasis.clone(), expr_sources.clone()),
            _ => return Err(self.err_here("expected template literal")),
        };
        self.bump();
        let mut expressions = Vec::with_capacity(sources.len());
        for src in &sources {
            // Re-lex+parse the hole as a full expression; bound depth carries over via
            // a fresh parser but we cap recursion by reusing this parser's depth budget.
            self.node()?;
            let toks = crate::lexer::lex(src)?;
            let mut sub = Parser::new(&toks);
            sub.depth = self.depth; // inherit remaining budget
            let e = sub.parse_expr()?;
            if !sub.at_eof() {
                return Err(self.err_here("unexpected token in template expression"));
            }
            expressions.push(e);
        }
        Ok((quasis, expressions))
    }

    fn parse_array_literal(&mut self) -> Result<Expr, JsError> {
        self.bump(); // [
        let mut elements = Vec::new();
        while !self.is_punct(Punct::RBracket) && !self.at_eof() {
            if self.is_punct(Punct::Comma) {
                self.bump();
                elements.push(None); // elision
                continue;
            }
            if self.eat_punct(Punct::DotDotDot) {
                elements.push(Some(ArrayElement::Spread(self.parse_assign()?)));
            } else {
                elements.push(Some(ArrayElement::Expr(self.parse_assign()?)));
            }
            if !self.eat_punct(Punct::Comma) {
                break;
            }
        }
        self.expect_punct(Punct::RBracket, "expected ']'")?;
        Ok(Expr::Array(elements))
    }

    fn parse_object_literal(&mut self) -> Result<Expr, JsError> {
        self.bump(); // {
        let mut props = Vec::new();
        while !self.is_punct(Punct::RBrace) && !self.at_eof() {
            if self.eat_punct(Punct::DotDotDot) {
                props.push(ObjectProp::Spread(self.parse_assign()?));
                if !self.eat_punct(Punct::Comma) {
                    break;
                }
                continue;
            }

            // get/set accessors (only if not immediately a key-terminator).
            let mut accessor: Option<ClassMemberKind> = None;
            if self.is_kw(Keyword::Get) && self.key_follows() {
                self.bump();
                accessor = Some(ClassMemberKind::Getter);
            } else if self.is_kw(Keyword::Set) && self.key_follows() {
                self.bump();
                accessor = Some(ClassMemberKind::Setter);
            }
            let is_async = self.is_kw(Keyword::Async) && self.key_follows();
            if is_async {
                self.bump();
            }
            let is_generator = self.eat_punct(Punct::Star);

            let key = self.parse_property_key()?;

            if self.is_punct(Punct::LParen) {
                // Method / accessor.
                let params = self.parse_params()?;
                let body = self.parse_block()?;
                props.push(ObjectProp::Method {
                    key,
                    kind: accessor.unwrap_or(ClassMemberKind::Method),
                    value: Function {
                        name: None,
                        params,
                        body,
                        is_arrow: false,
                        is_generator,
                        is_async,
                        arrow_expr: None,
                    },
                });
            } else if self.eat_punct(Punct::Colon) {
                let value = self.parse_assign()?;
                props.push(ObjectProp::KeyValue { key, value });
            } else {
                // Shorthand `{a}` or `{a = default}` (the latter only valid as a
                // destructuring target; we keep the key=value as a shorthand here).
                match key {
                    PropertyKey::Ident(n) => {
                        if self.eat_punct(Punct::Assign) {
                            // `{a = 1}` — treat as key:value with a default expression.
                            let default = self.parse_assign()?;
                            props.push(ObjectProp::KeyValue {
                                key: PropertyKey::Ident(n.clone()),
                                value: Expr::Assign {
                                    op: AssignOp::Assign,
                                    target: Box::new(Expr::Ident(n)),
                                    value: Box::new(default),
                                },
                            });
                        } else {
                            props.push(ObjectProp::Shorthand(n));
                        }
                    }
                    _ => return Err(self.err_here("expected ':' after property key")),
                }
            }
            if !self.eat_punct(Punct::Comma) {
                break;
            }
        }
        self.expect_punct(Punct::RBrace, "expected '}'")?;
        Ok(Expr::Object(props))
    }

    /// Does a property key follow the current token (so `get`/`set`/`async` is a
    /// modifier rather than itself a shorthand property name)?
    fn key_follows(&self) -> bool {
        matches!(
            self.peek_at(1).kind,
            TokenKind::Ident(_)
                | TokenKind::String(_)
                | TokenKind::Number(_)
                | TokenKind::Punct(Punct::LBracket)
        ) || matches!(self.peek_at(1).kind, TokenKind::Keyword(_))
    }

    // ═══ arrow functions ═════════════════════════════════════════════════════

    /// Try to parse an arrow function starting at the cursor. Returns `Ok(None)` (with
    /// the cursor restored) if this is not an arrow, so the caller falls through to a
    /// normal expression. Handles `x => …`, `(a, b) => …`, `() => …`, `async x => …`.
    fn try_parse_arrow(&mut self) -> Result<Option<Expr>, JsError> {
        let start = self.pos;
        let is_async = self.is_kw(Keyword::Async)
            && !self.peek_at(1).newline_before
            && matches!(
                self.peek_at(1).kind,
                TokenKind::Ident(_) | TokenKind::Punct(Punct::LParen)
            );
        let probe = if is_async { 1 } else { 0 };

        // Form 1: single identifier param `x =>`.
        if let TokenKind::Ident(_) = self.peek_at(probe).kind {
            if matches!(self.peek_at(probe + 1).kind, TokenKind::Punct(Punct::Arrow)) {
                if is_async {
                    self.bump();
                }
                let name = self.ident_name()?;
                self.bump(); // =>
                let params = alloc::vec![Param {
                    pattern: Pattern::Ident(name),
                    rest: false,
                }];
                return Ok(Some(self.finish_arrow(params, is_async)?));
            }
        }

        // Form 2: parenthesized params `( … ) =>`. Use a balanced scan to find the
        // matching `)` and check the token after it is `=>` before committing.
        if matches!(self.peek_at(probe).kind, TokenKind::Punct(Punct::LParen)) {
            if let Some(after) = self.matching_paren(self.pos + probe) {
                if matches!(self.peek_at_abs(after).kind, TokenKind::Punct(Punct::Arrow)) {
                    if is_async {
                        self.bump();
                    }
                    let params = self.parse_params()?;
                    self.expect_punct(Punct::Arrow, "expected '=>'")?;
                    return Ok(Some(self.finish_arrow(params, is_async)?));
                }
            }
        }

        // Not an arrow — restore and let the normal path handle it.
        self.pos = start;
        Ok(None)
    }

    fn peek_at_abs(&self, abs: usize) -> &Token {
        match self.toks.get(abs) {
            Some(t) => t,
            None => self.eof_tok(),
        }
    }

    /// Given the index of a `(` token, return the index just past its matching `)`,
    /// or `None` if unbalanced within a bounded scan.
    fn matching_paren(&self, lparen_idx: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut i = lparen_idx;
        let mut guard = 0usize;
        while i < self.toks.len() {
            guard += 1;
            if guard > crate::MAX_TOKENS {
                return None;
            }
            match &self.toks[i].kind {
                TokenKind::Punct(Punct::LParen) => depth += 1,
                TokenKind::Punct(Punct::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                TokenKind::Eof => return None,
                _ => {}
            }
            i += 1;
        }
        None
    }

    fn finish_arrow(&mut self, params: Vec<Param>, is_async: bool) -> Result<Expr, JsError> {
        self.node()?;
        if self.is_punct(Punct::LBrace) {
            let body = self.parse_block()?;
            Ok(Expr::Arrow(Function {
                name: None,
                params,
                body,
                is_arrow: true,
                is_generator: false,
                is_async,
                arrow_expr: None,
            }))
        } else {
            // Concise body: a single assignment expression. `x => ({a:1})` works
            // because the `(` makes the object an expression, not a block.
            let expr = self.parse_assign()?;
            Ok(Expr::Arrow(Function {
                name: None,
                params,
                body: Vec::new(),
                is_arrow: true,
                is_generator: false,
                is_async,
                arrow_expr: Some(Box::new(expr)),
            }))
        }
    }

    // ─── identifier helpers ──────────────────────────────────────────────────

    fn ident_name(&mut self) -> Result<String, JsError> {
        match self.kind().clone() {
            TokenKind::Ident(n) => {
                self.bump();
                Ok(n)
            }
            _ => Err(self.err_here("expected an identifier")),
        }
    }

    /// An identifier *or* a keyword used as a name (member names, property keys, etc.
    /// where any reserved word is permitted, e.g. `obj.class`, `{ if: 1 }`).
    fn ident_like_name(&mut self) -> Result<String, JsError> {
        match self.kind().clone() {
            TokenKind::Ident(n) => {
                self.bump();
                Ok(n)
            }
            TokenKind::Keyword(k) => {
                self.bump();
                Ok(String::from(keyword_text(k)))
            }
            _ => Err(self.err_here("expected a name")),
        }
    }
}

/// Internal binary-op tag used by the Pratt loop (logical ops are kept distinct so the
/// AST can preserve short-circuit semantics).
#[derive(Clone, Copy)]
enum BinOp {
    Binary(BinaryOp),
    Logical(LogicalOp),
}

fn keyword_text(k: Keyword) -> &'static str {
    match k {
        Keyword::Var => "var",
        Keyword::Let => "let",
        Keyword::Const => "const",
        Keyword::Function => "function",
        Keyword::Return => "return",
        Keyword::If => "if",
        Keyword::Else => "else",
        Keyword::For => "for",
        Keyword::While => "while",
        Keyword::Do => "do",
        Keyword::Break => "break",
        Keyword::Continue => "continue",
        Keyword::New => "new",
        Keyword::Typeof => "typeof",
        Keyword::Instanceof => "instanceof",
        Keyword::In => "in",
        Keyword::Of => "of",
        Keyword::This => "this",
        Keyword::Null => "null",
        Keyword::True => "true",
        Keyword::False => "false",
        Keyword::Delete => "delete",
        Keyword::Void => "void",
        Keyword::Switch => "switch",
        Keyword::Case => "case",
        Keyword::Default => "default",
        Keyword::Throw => "throw",
        Keyword::Try => "try",
        Keyword::Catch => "catch",
        Keyword::Finally => "finally",
        Keyword::Class => "class",
        Keyword::Extends => "extends",
        Keyword::Super => "super",
        Keyword::Yield => "yield",
        Keyword::Async => "async",
        Keyword::Await => "await",
        Keyword::Static => "static",
        Keyword::Get => "get",
        Keyword::Set => "set",
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
