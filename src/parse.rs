use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::fmt::{Display, Formatter, Write};
use std::ops::Deref;
use std::rc::Rc;

use log::trace;
use string_interner::Symbol;

use crate::lex::*;
use crate::typer::{BinaryOpKind, Linkage, UnaryOpKind};
use TokenKind as K;

pub type AstDefinitionId = u32;
pub type ExpressionId = u32;
pub type StatementId = u32;
pub type FileId = u32;

#[cfg(test)]
mod parse_test;

#[derive(Debug, Clone)]
pub struct ArrayExpr {
    pub elements: Vec<ExpressionId>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Literal {
    None(Span),
    Unit(Span),
    Char(u8, Span),
    Numeric(String, Span),
    Bool(bool, Span),
    /// TODO: Move these into the intern pool?
    String(String, Span),
}

impl Display for Literal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Literal::None(_) => f.write_str("None"),
            Literal::Unit(_) => f.write_str("()"),
            Literal::Char(byte, _) => {
                f.write_char('\'')?;
                f.write_char(*byte as char)?;
                f.write_char('\'')
            }
            Literal::Numeric(n, _) => f.write_str(n),
            Literal::Bool(true, _) => f.write_str("true"),
            Literal::Bool(false, _) => f.write_str("false"),
            Literal::String(s, _) => {
                f.write_char('"')?;
                f.write_str(s)?;
                f.write_char('"')
            }
        }
    }
}

impl Literal {
    pub fn get_span(&self) -> Span {
        match self {
            Literal::None(span) => *span,
            Literal::Unit(span) => *span,
            Literal::Char(_, span) => *span,
            Literal::Numeric(_, span) => *span,
            Literal::Bool(_, span) => *span,
            Literal::String(_, span) => *span,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct IdentifierId(string_interner::symbol::SymbolU32);

impl From<IdentifierId> for usize {
    fn from(value: IdentifierId) -> Self {
        value.0.to_usize()
    }
}

impl Display for IdentifierId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.to_usize())
    }
}

// We use the default StringInterner, which uses a contiguous string as its backend
// and u32 symbols
#[derive(Debug, Default)]
pub struct Identifiers {
    intern_pool: string_interner::StringInterner,
}
impl Identifiers {
    pub fn intern(&mut self, s: impl AsRef<str>) -> IdentifierId {
        let s = self.intern_pool.get_or_intern(&s);
        IdentifierId(s)
    }
    pub fn get_name(&self, id: IdentifierId) -> &str {
        self.intern_pool.resolve(id.0).expect("failed to resolve identifier")
    }
}

#[derive(Debug, Clone)]
pub struct FnCallArg {
    pub name: Option<IdentifierId>,
    pub value: ExpressionId,
}

#[derive(Debug, Clone)]
pub struct FnCallTypeArg {
    pub name: Option<IdentifierId>,
    pub type_expr: ParsedTypeExpression,
}

#[derive(Debug, Clone)]
pub struct FnCall {
    pub name: IdentifierId,
    pub type_args: Option<Vec<FnCallTypeArg>>,
    pub args: Vec<FnCallArg>,
    pub namespaces: Vec<IdentifierId>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ValDef {
    pub name: IdentifierId,
    pub type_id: Option<ParsedTypeExpression>,
    pub value: ExpressionId,
    pub is_mutable: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct BinaryOp {
    pub op_kind: BinaryOpKind,
    pub lhs: ExpressionId,
    pub rhs: ExpressionId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct UnaryOp {
    pub op_kind: UnaryOpKind,
    pub expr: ExpressionId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Variable {
    pub name: IdentifierId,
    pub namespaces: Vec<IdentifierId>,
    pub span: Span,
}

impl Display for Variable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("var#{}", self.name))
    }
}

#[derive(Debug, Clone)]
pub struct FieldAccess {
    pub base: ExpressionId,
    pub target: IdentifierId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RecordField {
    pub name: IdentifierId,
    pub expr: ExpressionId,
}

#[derive(Debug, Clone)]
/// Example:
/// { foo: 1, bar: false }
///   ^................^ fields
pub struct Record {
    pub fields: Vec<RecordField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
/// Example: users  [42]
///          ^target ^index_value
pub struct IndexOperation {
    pub target: ExpressionId,
    pub index_expr: ExpressionId,
    pub span: Span,
}

impl Display for IndexOperation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.target.fmt(f)?;
        f.write_char('[')?;
        self.index_expr.fmt(f)?;
        f.write_char(']')?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct MethodCall {
    pub base: ExpressionId,
    pub call: Box<FnCall>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct OptionalGet {
    pub base: ExpressionId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ParsedExpression {
    BinaryOp(BinaryOp),             // a == b
    UnaryOp(UnaryOp),               // !b, *b
    Literal(Literal),               // 42, "asdf"
    FnCall(FnCall),                 // square(1, 2)
    Variable(Variable),             // x
    FieldAccess(FieldAccess),       // x.b
    MethodCall(MethodCall),         // x.load()
    Block(Block),                   // { <expr>; <expr>; <expr> }
    If(IfExpr),                     // if a else b
    Record(Record),                 // { x: 1, y: 3 }
    IndexOperation(IndexOperation), // xs[3]
    Array(ArrayExpr),               // [1, 3, 5, 7]
    OptionalGet(OptionalGet),       // foo!
    For(ForExpr),                   // for i in [1,2,3] do println(i)
}

impl ParsedExpression {
    pub fn is_literal(e: &ParsedExpression) -> bool {
        matches!(e, ParsedExpression::Literal(_))
    }
    #[inline]
    pub fn get_span(&self) -> Span {
        match self {
            ParsedExpression::BinaryOp(op) => op.span,
            ParsedExpression::UnaryOp(op) => op.span,
            ParsedExpression::Literal(lit) => lit.get_span(),
            ParsedExpression::FnCall(call) => call.span,
            ParsedExpression::Variable(var) => var.span,
            ParsedExpression::FieldAccess(acc) => acc.span,
            ParsedExpression::MethodCall(call) => call.span,
            ParsedExpression::Block(block) => block.span,
            ParsedExpression::If(if_expr) => if_expr.span,
            ParsedExpression::Record(record) => record.span,
            ParsedExpression::IndexOperation(op) => op.span,
            ParsedExpression::Array(array_expr) => array_expr.span,
            ParsedExpression::OptionalGet(optional_get) => optional_get.span,
            ParsedExpression::For(for_expr) => for_expr.span,
        }
    }

    pub fn is_assignable(&self) -> bool {
        match self {
            ParsedExpression::Variable(_var) => true,
            ParsedExpression::IndexOperation(_op) => true,
            ParsedExpression::FieldAccess(_acc) => true,
            ParsedExpression::MethodCall(_call) => false,
            ParsedExpression::BinaryOp(_op) => false,
            ParsedExpression::UnaryOp(_op) => false,
            ParsedExpression::Literal(_lit) => false,
            ParsedExpression::FnCall(_call) => false,
            ParsedExpression::Block(_block) => false,
            ParsedExpression::If(_if_expr) => false,
            ParsedExpression::Record(_record) => false,
            ParsedExpression::Array(_array_expr) => false,
            ParsedExpression::OptionalGet(_optional_get) => false,
            ParsedExpression::For(_) => false,
        }
    }
}

impl Display for ParsedExpression {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParsedExpression::BinaryOp(op) => {
                f.write_fmt(format_args!("({} {} {})", op.lhs, op.op_kind, op.rhs))
            }
            ParsedExpression::UnaryOp(op) => {
                let _ = op.op_kind.fmt(f);
                op.expr.fmt(f)
            }
            ParsedExpression::Literal(lit) => lit.fmt(f),
            ParsedExpression::FnCall(call) => std::fmt::Debug::fmt(call, f),
            ParsedExpression::Variable(var) => var.fmt(f),
            ParsedExpression::FieldAccess(acc) => std::fmt::Debug::fmt(acc, f),
            ParsedExpression::MethodCall(call) => std::fmt::Debug::fmt(call, f),
            ParsedExpression::Block(block) => std::fmt::Debug::fmt(block, f),
            ParsedExpression::If(if_expr) => std::fmt::Debug::fmt(if_expr, f),
            ParsedExpression::Record(record) => std::fmt::Debug::fmt(record, f),
            ParsedExpression::IndexOperation(op) => op.fmt(f),
            ParsedExpression::Array(array_expr) => std::fmt::Debug::fmt(array_expr, f),
            ParsedExpression::OptionalGet(optional_get) => std::fmt::Debug::fmt(optional_get, f),
            ParsedExpression::For(for_expr) => std::fmt::Debug::fmt(for_expr, f),
        }
    }
}

enum ExprStackMember {
    Operator(BinaryOpKind, Span),
    Expr(ExpressionId),
}

impl ExprStackMember {
    fn expect_expr(self) -> ExpressionId {
        match self {
            ExprStackMember::Expr(expr) => expr,
            _ => panic!("expected expr"),
        }
    }
    fn expect_operator(self) -> (BinaryOpKind, Span) {
        match self {
            ExprStackMember::Operator(kind, span) => (kind, span),
            _ => panic!("expected operator"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub lhs: ExpressionId,
    pub rhs: ExpressionId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IfExpr {
    pub cond: ExpressionId,
    pub optional_ident: Option<(IdentifierId, Span)>,
    pub cons: ExpressionId,
    pub alt: Option<ExpressionId>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub cond: ExpressionId,
    pub block: Block,
    /// Maybe its better not to store a span on nodes for which a span is trivially calculated
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForExprType {
    Yield,
    Do,
}

#[derive(Debug, Clone)]
pub struct ForExpr {
    pub iterable_expr: ExpressionId,
    pub binding: Option<IdentifierId>,
    pub body_block: Block,
    pub expr_type: ForExprType,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum BlockStmt {
    ValDef(ValDef),               // val x = 42
    Assignment(Assignment),       // x = 42
    LoneExpression(ExpressionId), // println("asdfasdf")
    While(WhileStmt),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<BlockStmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RecordTypeField {
    pub name: IdentifierId,
    pub ty: ParsedTypeExpression,
}

#[derive(Debug, Clone)]
pub struct RecordType {
    pub fields: Vec<RecordTypeField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeApplication {
    pub base: IdentifierId,
    pub params: Vec<ParsedTypeExpression>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ParsedOptional {
    pub base: Box<ParsedTypeExpression>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ParsedReference {
    pub base: Box<ParsedTypeExpression>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ParsedTypeExpression {
    Unit(Span),
    Char(Span),
    Int(Span),
    Bool(Span),
    String(Span),
    Record(RecordType),
    Name(IdentifierId, Span),
    TypeApplication(TypeApplication),
    Optional(ParsedOptional),
    Reference(ParsedReference),
}

impl ParsedTypeExpression {
    #[inline]
    pub fn is_int(&self) -> bool {
        matches!(self, ParsedTypeExpression::Int(_))
    }
    #[inline]
    pub fn get_span(&self) -> Span {
        match self {
            ParsedTypeExpression::Unit(span) => *span,
            ParsedTypeExpression::Char(span) => *span,
            ParsedTypeExpression::Int(span) => *span,
            ParsedTypeExpression::Bool(span) => *span,
            ParsedTypeExpression::String(span) => *span,
            ParsedTypeExpression::Record(record) => record.span,
            ParsedTypeExpression::Name(_, span) => *span,
            ParsedTypeExpression::TypeApplication(app) => app.span,
            ParsedTypeExpression::Optional(opt) => opt.span,
            ParsedTypeExpression::Reference(r) => r.span,
        }
    }
}

impl Display for ParsedTypeExpression {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParsedTypeExpression::Unit(_) => f.write_str("unit"),
            ParsedTypeExpression::Char(_) => f.write_str("char"),
            ParsedTypeExpression::Int(_) => f.write_str("int"),
            ParsedTypeExpression::Bool(_) => f.write_str("bool"),
            ParsedTypeExpression::String(_) => f.write_str("string"),
            ParsedTypeExpression::Record(record_type) => {
                f.write_str("{ ")?;
                for field in record_type.fields.iter() {
                    field.name.fmt(f)?;
                    f.write_str(": ")?;
                    field.ty.fmt(f)?;
                    f.write_str(", ")?;
                }
                f.write_str(" }")
            }
            ParsedTypeExpression::Name(ident, _) => ident.fmt(f),
            ParsedTypeExpression::TypeApplication(tapp) => {
                tapp.base.fmt(f)?;
                f.write_str("<")?;
                for tparam in tapp.params.iter() {
                    tparam.fmt(f)?;
                    f.write_str(", ")?;
                }
                f.write_str(">")
            }
            ParsedTypeExpression::Optional(opt) => {
                opt.base.fmt(f)?;
                f.write_str("?")
            }
            ParsedTypeExpression::Reference(refer) => {
                refer.base.fmt(f)?;
                f.write_str("*")
            }
        }
    }
}

#[derive(Debug)]
pub struct TypeParamDef {
    pub ident: IdentifierId,
    pub span: Span,
}

#[derive(Debug)]
pub struct FnDef {
    pub name: IdentifierId,
    pub type_args: Option<Vec<TypeParamDef>>,
    pub args: Vec<FnArgDef>,
    pub ret_type: Option<ParsedTypeExpression>,
    pub block: Option<Block>,
    pub span: Span,
    pub linkage: Linkage,
    pub definition_id: AstDefinitionId,
}

#[derive(Debug)]
pub struct FnArgDef {
    pub name: IdentifierId,
    pub ty: ParsedTypeExpression,
    pub span: Span,
}

#[derive(Debug)]
pub struct ConstVal {
    pub name: IdentifierId,
    pub ty: ParsedTypeExpression,
    pub value_expr: ExpressionId,
    pub span: Span,
    pub definition_id: AstDefinitionId,
}

#[derive(Debug)]
pub struct TypeDefn {
    pub name: IdentifierId,
    pub value_expr: ParsedTypeExpression,
    pub span: Span,
    pub definition_id: AstDefinitionId,
}

#[derive(Debug)]
pub struct ParsedNamespace {
    pub name: IdentifierId,
    pub definitions: Vec<Definition>,
    pub definition_id: AstDefinitionId,
}

#[derive(Debug)]
pub enum Definition {
    FnDef(Box<FnDef>),
    Const(Box<ConstVal>),
    TypeDef(Box<TypeDefn>),
    Namespace(Box<ParsedNamespace>),
}

impl Definition {
    pub fn definition_id(&self) -> AstDefinitionId {
        match self {
            Definition::FnDef(def) => def.definition_id,
            Definition::Const(def) => def.definition_id,
            Definition::TypeDef(def) => def.definition_id,
            Definition::Namespace(def) => def.definition_id,
        }
    }

    pub fn find_defn(&self, ast_id: AstDefinitionId) -> Option<&Definition> {
        if self.definition_id() == ast_id {
            return Some(self);
        }
        if let Definition::Namespace(ns) = self {
            for defn in &ns.definitions {
                if let Some(found) = defn.find_defn(ast_id) {
                    return Some(found);
                }
            }
            None
        } else {
            None
        }
    }

    pub(crate) fn as_fn_def(&self) -> Option<&FnDef> {
        match self {
            Definition::FnDef(def) => Some(def),
            _ => None,
        }
    }
}

impl Definition {
    pub fn get_name(&self) -> IdentifierId {
        match self {
            Definition::FnDef(def) => def.name,
            Definition::Const(def) => def.name,
            Definition::TypeDef(def) => def.name,
            Definition::Namespace(def) => def.name,
        }
    }
    pub fn get_ast_id(&self) -> AstDefinitionId {
        match self {
            Definition::FnDef(def) => def.definition_id,
            Definition::Const(def) => def.definition_id,
            Definition::TypeDef(def) => def.definition_id,
            Definition::Namespace(def) => def.definition_id,
        }
    }
}

#[derive(Debug, Default)]
pub struct ParsedExpressionPool {
    expressions: Vec<ParsedExpression>,
    type_hints: HashMap<ExpressionId, ParsedTypeExpression>,
}
impl ParsedExpressionPool {
    pub fn add_type_hint(&mut self, id: ExpressionId, ty: ParsedTypeExpression) {
        self.type_hints.insert(id, ty);
    }

    pub fn get_type_hint(&self, id: ExpressionId) -> Option<&ParsedTypeExpression> {
        self.type_hints.get(&id)
    }

    pub fn add_expression(&mut self, expression: ParsedExpression) -> ExpressionId {
        let id = self.expressions.len();
        self.expressions.push(expression);
        id as ExpressionId
    }
    pub fn get_expression(&self, id: ExpressionId) -> &ParsedExpression {
        &self.expressions[id as usize]
    }
}

#[derive(Debug, Default)]
pub struct Sources {
    sources: HashMap<FileId, Rc<Source>>,
}

impl Sources {
    pub fn insert(&mut self, source: Rc<Source>) {
        self.sources.insert(source.file_id, source);
    }

    pub fn get_main(&self) -> Rc<Source> {
        self.sources.get(&0).unwrap().clone()
    }

    pub fn get_line_for_span(&self, span: Span) -> &str {
        self.sources.get(&span.file_id).unwrap().get_line_by_index(span.line)
    }

    pub fn get_span_content(&self, span: Span) -> &str {
        self.sources.get(&span.file_id).unwrap().get_span_content(span)
    }

    pub fn source_by_span(&self, span: Span) -> Rc<Source> {
        self.sources.get(&span.file_id).unwrap().clone()
    }
}

#[derive(Debug)]
pub struct ParsedModule {
    pub name: String,
    pub name_id: IdentifierId,
    pub defs: Vec<Definition>,
    pub sources: Sources,
    /// Using RefCell here just so we can mutably access
    /// the identifiers without having mutable access to
    /// the entire AST module. Lets me wait to decide
    /// where things actually live
    ///
    /// After reading the Roc codebase, I think the move
    /// is to move away from these big structs and just have top-level functions
    /// so we can be more granular about what is mutable when. You can create an 'Env'
    /// struct to reduce the number of parameters, but this Env will also buffer from that problem sometimes
    pub identifiers: Rc<RefCell<Identifiers>>,
    pub expressions: Rc<RefCell<ParsedExpressionPool>>,
    pub ast_id_index: u32,
}

impl ParsedModule {
    pub fn make(name: String) -> ParsedModule {
        let identifiers = Rc::new(RefCell::new(Identifiers::default()));
        let name_id = identifiers.borrow_mut().intern(&name);
        ParsedModule {
            name,
            name_id,
            defs: Vec::new(),
            sources: Sources::default(),
            identifiers,
            expressions: Rc::new(RefCell::new(ParsedExpressionPool::default())),
            ast_id_index: 0,
        }
    }

    pub fn ident_id(&self, ident: &str) -> IdentifierId {
        self.identifiers.borrow_mut().intern(ident)
    }
    pub fn get_ident_str(&self, id: IdentifierId) -> impl Deref<Target = str> + '_ {
        Ref::map(self.identifiers.borrow(), |idents| idents.get_name(id))
    }

    pub fn get_defn_by_id(&self, ast_id: AstDefinitionId) -> &Definition {
        for defn in &self.defs {
            if let Some(found) = defn.find_defn(ast_id) {
                return found;
            }
        }
        panic!("failed to find defn with ast_id {}", ast_id);
    }

    pub fn defns_iter(&self) -> impl Iterator<Item = &Definition> {
        self.defs.iter()
    }

    pub fn get_expression(&self, id: ExpressionId) -> impl Deref<Target = ParsedExpression> + '_ {
        Ref::map(self.expressions.borrow(), |e| e.get_expression(id))
    }

    pub fn add_expression(&self, expression: ParsedExpression) -> ExpressionId {
        self.expressions.borrow_mut().add_expression(expression)
    }

    pub fn get_expression_type_hint(&self, id: ExpressionId) -> Option<Ref<ParsedTypeExpression>> {
        match Ref::filter_map(self.expressions.borrow(), |e| e.get_type_hint(id)) {
            Err(_) => None,
            Ok(r) => Some(r),
        }
    }
}

pub type ParseResult<A> = anyhow::Result<A, ParseError>;

#[derive(Debug)]
pub struct ParseError {
    pub expected: String,
    pub token: Token,
    pub cause: Option<Box<ParseError>>,
}

impl ParseError {
    pub fn span(&self) -> Span {
        self.token.span
    }
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{:?}", self))
    }
}
impl std::error::Error for ParseError {}

#[derive(Debug)]
pub struct Source {
    pub file_id: FileId,
    pub directory: String,
    pub filename: String,
    pub content: String,
    /// This is an inefficient copy but we need the lines cached because utf8
    /// Eventually it can be references not copies
    pub lines: Vec<String>,
}

impl Source {
    pub fn make(file_id: FileId, directory: String, filename: String, content: String) -> Source {
        let lines: Vec<_> = content.lines().map(|l| l.to_owned()).collect();
        Source { file_id, directory, filename, content, lines }
    }

    pub fn get_span_content(&self, span: Span) -> &str {
        &self.content[span.start as usize..span.end as usize]
    }

    pub fn get_line_by_index(&self, line_index: u32) -> &str {
        &self.lines[line_index as usize]
    }
}

pub struct Parser<'toks, 'module> {
    tokens: TokenIter<'toks>,
    source: Rc<Source>,
    // FIXME: Remove my copies of identifiers, expressions
    identifiers: Rc<RefCell<Identifiers>>,
    expressions: Rc<RefCell<ParsedExpressionPool>>,
    parsed_module: &'module mut ParsedModule,
}

impl<'toks, 'module> Parser<'toks, 'module> {
    pub fn make(
        tokens: &'toks [Token],
        source: Rc<Source>,
        module: &'module mut ParsedModule,
    ) -> Parser<'toks, 'module> {
        module.sources.insert(source.clone());
        Parser {
            tokens: TokenIter::make(tokens),
            source,
            identifiers: module.identifiers.clone(),
            expressions: module.expressions.clone(),
            parsed_module: module,
        }
    }

    fn expect<A>(what: &str, current: Token, value: ParseResult<Option<A>>) -> ParseResult<A> {
        match value {
            Ok(None) => Err(ParseError { expected: what.to_string(), token: current, cause: None }),
            Ok(Some(a)) => Ok(a),
            Err(e) => Err(e),
        }
    }
}

impl<'toks, 'module> Parser<'toks, 'module> {
    pub fn ident_id(&self, s: impl AsRef<str>) -> IdentifierId {
        self.identifiers.borrow_mut().intern(s.as_ref())
    }

    pub fn print_error(&self, parse_error: &ParseError) {
        let span = parse_error.span();
        let line_text = self.source.get_line_by_index(parse_error.span().line);
        let span_text = &self.source.get_span_content(span);
        use colored::*;

        if let Some(cause) = &parse_error.cause {
            self.print_error(cause);
        }
        println!(
            "{} on line {}. Expected '{}', but got '{}'",
            "parse error".red(),
            span.line_number(),
            parse_error.expected.blue(),
            parse_error.token.kind.as_ref().red()
        );
        println!();
        println!("{line_text}");
        println!("{span_text}");
    }

    fn peek(&self) -> Token {
        self.tokens.peek()
    }

    fn chars_at(&self, start: u32, end: u32) -> &str {
        &self.source.content[start as usize..end as usize]
    }
    fn chars_at_span(&self, span: Span) -> &str {
        self.chars_at(span.start, span.end)
    }
    fn tok_chars(&self, tok: Token) -> &str {
        let s = self.chars_at_span(tok.span);
        trace!("{} chars '{}'", tok.kind, s);
        s
    }

    fn eat_token(&mut self, target_token: TokenKind) -> Option<Token> {
        let tok = self.peek();
        if tok.kind == target_token {
            self.tokens.advance();
            trace!("eat_token SUCCESS '{}'", target_token);
            Some(tok)
        } else {
            trace!("eat_token MISS '{}'", target_token);
            None
        }
    }

    fn error(expected: impl AsRef<str>, token: Token) -> ParseError {
        ParseError { expected: expected.as_ref().to_owned(), token, cause: None }
    }
    fn error_cause(expected: impl AsRef<str>, token: Token, cause: ParseError) -> ParseError {
        ParseError { expected: expected.as_ref().to_owned(), token, cause: Some(Box::new(cause)) }
    }

    fn expect_eat_token(&mut self, target_token: TokenKind) -> ParseResult<Token> {
        let result = self.eat_token(target_token);
        match result {
            None => {
                let actual = self.peek();
                Err(Parser::error(target_token, actual))
            }
            Some(t) => Ok(t),
        }
    }

    fn intern_ident_token(&mut self, token: Token) -> IdentifierId {
        let source = self.source.clone();
        let tok_chars = Source::get_span_content(&source, token.span);
        self.ident_id(tok_chars)
    }

    pub fn add_expression(&self, expression: ParsedExpression) -> ExpressionId {
        self.expressions.borrow_mut().add_expression(expression)
    }

    pub fn get_expression(
        &self,
        id: ExpressionId,
    ) -> impl std::ops::Deref<Target = ParsedExpression> + '_ {
        std::cell::Ref::map(self.expressions.borrow(), |e| e.get_expression(id))
    }

    pub fn get_expression_span(&self, id: ExpressionId) -> Span {
        self.expressions.borrow().get_expression(id).get_span()
    }

    fn parse_literal(&mut self) -> ParseResult<Option<Literal>> {
        let (first, second) = self.tokens.peek_two();
        trace!("parse_literal {} {}", first.kind, second.kind);
        return match (first.kind, second.kind) {
            (K::OpenParen, K::CloseParen) => {
                trace!("parse_literal unit");
                let span = first.span.extended(second.span);
                self.tokens.advance();
                self.tokens.advance();
                Ok(Some(Literal::Unit(span)))
            }
            (K::Char, _) => {
                trace!("parse_literal char");
                self.tokens.advance();
                let text = self.tok_chars(first);
                assert!(text.starts_with('\''));
                assert!(text.ends_with('\''));
                let bytes = text.as_bytes();
                if bytes[1] == b'\\' {
                    assert_eq!(bytes.len(), 4);
                    let esc_char = bytes[2];
                    match esc_char {
                        b'n' => Ok(Some(Literal::Char(b'\n', first.span))),
                        b'\'' => Ok(Some(Literal::Char(b'\'', first.span))),
                        b't' => Ok(Some(Literal::Char(b'\t', first.span))),
                        _ => Err(Parser::error(
                            format!(
                                "Valid escaped char following escape sequence: {}",
                                char::from(esc_char)
                            ),
                            first,
                        )),
                    }
                } else {
                    assert_eq!(bytes.len(), 3);
                    let byte = bytes[1];
                    Ok(Some(Literal::Char(byte, first.span)))
                }
            }
            (K::String, _) => {
                trace!("parse_literal string");
                self.tokens.advance();
                let text = self.tok_chars(first);
                Ok(Some(Literal::String(text.to_string(), first.span)))
            }
            (K::Minus, K::Ident) if !second.is_whitespace_preceeded() => {
                let text = self.tok_chars(second);
                if text.chars().next().unwrap().is_numeric() {
                    let mut s = "-".to_string();
                    s.push_str(text);
                    self.tokens.advance();
                    self.tokens.advance();
                    Ok(Some(Literal::Numeric(s, first.span.extended(second.span))))
                } else {
                    Err(Parser::error("number following '-'", second))
                }
            }
            (K::Ident, _) => {
                let text = self.tok_chars(first);
                if text == "true" {
                    self.tokens.advance();
                    Ok(Some(Literal::Bool(true, first.span)))
                } else if text == "false" {
                    self.tokens.advance();
                    Ok(Some(Literal::Bool(false, first.span)))
                } else if text == "None" {
                    self.tokens.advance();
                    Ok(Some(Literal::None(first.span)))
                } else {
                    match text.chars().next() {
                        Some(c) if c.is_numeric() || c == '-' => {
                            let s = text.to_string();
                            self.tokens.advance();
                            Ok(Some(Literal::Numeric(s, first.span)))
                        }
                        _ => Ok(None),
                    }
                }
            }
            _ => Ok(None),
        };
    }

    fn parse_record_type_field(&mut self) -> ParseResult<Option<RecordTypeField>> {
        let name_token = self.expect_eat_token(K::Ident)?;
        let ident_id = self.intern_ident_token(name_token);
        self.expect_eat_token(K::Colon)?;
        let typ_expr =
            Parser::expect("Type expression", self.peek(), self.parse_type_expression())?;
        Ok(Some(RecordTypeField { name: ident_id, ty: typ_expr }))
    }

    fn expect_type_expression(&mut self) -> ParseResult<ParsedTypeExpression> {
        Parser::expect("type_expression", self.peek(), self.parse_type_expression())
    }

    fn parse_type_expression(&mut self) -> ParseResult<Option<ParsedTypeExpression>> {
        let Some(result) = self.parse_base_type_expression()? else {
            return Ok(None);
        };
        let next = self.peek();
        if next.kind.is_postfix_type_operator() {
            if next.kind == K::QuestionMark {
                // Optional Type
                self.tokens.advance();
                Ok(Some(ParsedTypeExpression::Optional(ParsedOptional {
                    base: Box::new(result),
                    span: next.span,
                })))
            } else if next.kind == K::Asterisk {
                // Reference Type
                self.tokens.advance();
                Ok(Some(ParsedTypeExpression::Reference(ParsedReference {
                    base: Box::new(result),
                    span: next.span,
                })))
            } else {
                panic!("unhandled postfix type operator {:?}", next.kind);
            }
        } else {
            Ok(Some(result))
        }
    }

    fn parse_base_type_expression(&mut self) -> ParseResult<Option<ParsedTypeExpression>> {
        let tok = self.peek();
        if tok.kind == K::Ident {
            let source = self.source.clone();
            let text_str = Source::get_span_content(&source, tok.span);
            if text_str == "unit" {
                self.tokens.advance();
                Ok(Some(ParsedTypeExpression::Unit(tok.span)))
            } else if text_str == "string" {
                self.tokens.advance();
                Ok(Some(ParsedTypeExpression::String(tok.span)))
            } else if text_str == "int" {
                self.tokens.advance();
                Ok(Some(ParsedTypeExpression::Int(tok.span)))
            } else if text_str == "bool" {
                self.tokens.advance();
                Ok(Some(ParsedTypeExpression::Bool(tok.span)))
            } else if text_str == "char" {
                self.tokens.advance();
                Ok(Some(ParsedTypeExpression::Char(tok.span)))
            } else {
                self.tokens.advance();
                let next = self.tokens.peek();
                if next.kind == K::OpenAngle {
                    // parameterized type: Dict<int, int>
                    self.tokens.advance();
                    let (type_parameters, params_span) =
                        self.eat_delimited("Type parameters", K::Comma, K::CloseAngle, |p| {
                            Parser::expect_type_expression(p)
                        })?;
                    let ident = self.intern_ident_token(tok);
                    Ok(Some(ParsedTypeExpression::TypeApplication(TypeApplication {
                        base: ident,
                        params: type_parameters,
                        span: tok.span.extended(params_span),
                    })))
                } else {
                    Ok(Some(ParsedTypeExpression::Name(self.ident_id(text_str), tok.span)))
                }
            }
        } else if tok.kind == K::OpenBrace {
            let open_brace = self.expect_eat_token(K::OpenBrace)?;
            let (fields, fields_span) =
                self.eat_delimited("Record fields", K::Comma, K::CloseBrace, |p| {
                    let field_res = Parser::parse_record_type_field(p);
                    Parser::expect("Record Field", open_brace, field_res)
                })?;
            let mut record_span = tok.span;
            record_span.end = fields_span.end;
            let record = RecordType { fields, span: record_span };
            Ok(Some(ParsedTypeExpression::Record(record)))
        } else {
            Ok(None)
        }
    }

    fn parse_fn_arg(&mut self) -> ParseResult<Option<FnCallArg>> {
        let (one, two) = self.tokens.peek_two();
        let named = if one.kind == K::Ident && two.kind == K::Equals {
            self.tokens.advance();
            self.tokens.advance();
            true
        } else {
            false
        };
        match self.parse_expression() {
            Ok(Some(expr)) => {
                let name = if named { Some(self.intern_ident_token(one)) } else { None };
                Ok(Some(FnCallArg { name, value: expr }))
            }
            Ok(None) => {
                if named {
                    Err(Parser::error("expression", self.peek()))
                } else {
                    Ok(None)
                }
            }
            Err(e) => Err(e),
        }
    }

    fn expect_fn_arg(&mut self) -> ParseResult<FnCallArg> {
        let res = self.parse_fn_arg();
        Parser::expect("fn_arg", self.peek(), res)
    }

    fn parse_record(&mut self) -> ParseResult<Option<Record>> {
        let Some(open_brace) = self.eat_token(K::OpenBrace) else {
            return Ok(None);
        };
        let (fields, fields_span) =
            self.eat_delimited("Record values", K::Comma, K::CloseBrace, |parser| {
                let name = parser.expect_eat_token(K::Ident)?;
                parser.expect_eat_token(K::Colon)?;
                let expr = Parser::expect("expression", parser.peek(), parser.parse_expression())?;
                Ok(RecordField { name: parser.intern_ident_token(name), expr })
            })?;
        let span = open_brace.span.extended(fields_span);
        Ok(Some(Record { fields, span }))
    }

    fn parse_expression_with_postfix_ops(&mut self) -> ParseResult<Option<ExpressionId>> {
        let Some(mut result) = self.parse_base_expression()? else { return Ok(None) };
        // Looping for postfix ops inspired by Jakt's parser
        let with_postfix: ExpressionId = loop {
            let next = self.peek();
            if next.kind.is_postfix_operator() {
                // Optional uwrap `config!.url`
                if next.kind == K::Bang {
                    self.tokens.advance();
                    let span = self.get_expression_span(result).extended(next.span);
                    result = self.add_expression(ParsedExpression::OptionalGet(OptionalGet {
                        base: result,
                        span,
                    }));
                } else if next.kind == K::Dot {
                    // Field access syntax; a.b
                    self.tokens.advance();
                    let target = self.expect_eat_token(K::Ident)?;
                    let next = self.peek();
                    // method call access syntax; a.b() a.b<int>(c)
                    if next.kind == K::OpenParen
                        || (next.kind == K::OpenAngle && !next.is_whitespace_preceeded())
                    {
                        let type_args = self.parse_optional_type_args()?;
                        self.expect_eat_token(K::OpenParen)?;
                        let (args, args_span) = self.eat_delimited(
                            "Function arguments",
                            K::Comma,
                            K::CloseParen,
                            Parser::expect_fn_arg,
                        )?;
                        let span = self.get_expression(result).get_span().extended(args_span);
                        let name = self.intern_ident_token(target);
                        result = self.add_expression(ParsedExpression::MethodCall(MethodCall {
                            base: result,
                            call: Box::new(FnCall {
                                name,
                                type_args,
                                args,
                                namespaces: Vec::new(),
                                span,
                            }),
                            span,
                        }));
                    } else {
                        let span = self.get_expression(result).get_span().extended(next.span);
                        let target = self.intern_ident_token(target);
                        result = self.add_expression(ParsedExpression::FieldAccess(FieldAccess {
                            base: result,
                            target,
                            span,
                        }));
                    }
                } else if next.kind == K::OpenBracket {
                    self.tokens.advance();
                    let index_expr = Parser::expect(
                        "expression inside []",
                        self.peek(),
                        self.parse_expression(),
                    )?;
                    let close = self.expect_eat_token(K::CloseBracket)?;
                    let span = self.get_expression(result).get_span().extended(close.span);
                    result =
                        self.add_expression(ParsedExpression::IndexOperation(IndexOperation {
                            target: result,
                            index_expr,
                            span,
                        }));
                }
            } else {
                break result;
            }
        };
        if self.peek().kind == K::Colon {
            self.tokens.advance();
            let type_hint = self.expect_type_expression()?;
            self.expressions.borrow_mut().add_type_hint(with_postfix, type_hint);
        }
        Ok(Some(with_postfix))
    }

    fn expect_block(&mut self) -> ParseResult<Block> {
        Parser::expect("block", self.peek(), self.parse_block())
    }

    fn expect_expression(&mut self) -> ParseResult<ExpressionId> {
        Parser::expect("expression", self.peek(), self.parse_expression())
    }

    fn parse_expression(&mut self) -> ParseResult<Option<ExpressionId>> {
        let Some(expr) = self.parse_expression_with_postfix_ops()? else {
            return Ok(None);
        };
        if !self.peek().kind.is_binary_operator() {
            return Ok(Some(expr));
        }
        let mut expr_stack: Vec<ExprStackMember> = vec![ExprStackMember::Expr(expr)];
        let mut last_precedence = 100_000;
        loop {
            let tok = self.peek();
            let Some(op_kind) = BinaryOpKind::from_tokenkind(tok.kind) else {
                break;
            };
            let precedence = op_kind.precedence();
            self.tokens.advance();
            let rhs = Parser::expect(
                "rhs of binary op",
                self.peek(),
                self.parse_expression_with_postfix_ops(),
            )?;
            while precedence <= last_precedence && expr_stack.len() > 1 {
                trace!(
                    "expr_stack at {:?}, precedence={}, last={}, stacklen={}",
                    op_kind,
                    precedence,
                    last_precedence,
                    expr_stack.len()
                );
                let rhs = expr_stack.pop().unwrap().expect_expr();
                let (op_kind, op_span) = expr_stack.pop().unwrap().expect_operator();
                last_precedence = op_kind.precedence();
                if last_precedence < precedence {
                    expr_stack.push(ExprStackMember::Operator(op_kind, op_span));
                    expr_stack.push(ExprStackMember::Expr(rhs));
                    break;
                }
                let ExprStackMember::Expr(lhs) = expr_stack.pop().unwrap() else {
                    panic!("expected expr on stack")
                };
                let new_span = self
                    .get_expression(lhs)
                    .get_span()
                    .extended(self.get_expression(rhs).get_span());
                let bin_op = self.add_expression(ParsedExpression::BinaryOp(BinaryOp {
                    op_kind,
                    lhs,
                    rhs,
                    span: new_span,
                }));
                expr_stack.push(ExprStackMember::Expr(bin_op))
            }
            expr_stack.push(ExprStackMember::Operator(op_kind, tok.span));
            expr_stack.push(ExprStackMember::Expr(rhs));

            last_precedence = precedence;
        }

        // Pop and build now that everything is right
        while expr_stack.len() > 1 {
            let ExprStackMember::Expr(rhs) = expr_stack.pop().unwrap() else {
                panic!("expected expr")
            };
            let ExprStackMember::Operator(op_kind, _) = expr_stack.pop().unwrap() else {
                panic!("expected operator")
            };
            let ExprStackMember::Expr(lhs) = expr_stack.pop().unwrap() else {
                panic!("expected expr")
            };
            let new_span = self.extended_span(lhs, rhs);
            let bin_op = self.add_expression(ParsedExpression::BinaryOp(BinaryOp {
                op_kind,
                lhs,
                rhs,
                span: new_span,
            }));
            expr_stack.push(ExprStackMember::Expr(bin_op));
        }
        let final_expr = expr_stack.pop().unwrap().expect_expr();
        Ok(Some(final_expr))
    }

    fn extended_span(&self, expr1: ExpressionId, expr2: ExpressionId) -> Span {
        self.get_expression(expr1).get_span().extended(self.get_expression(expr2).get_span())
    }

    fn parse_optional_type_args(&mut self) -> ParseResult<Option<Vec<FnCallTypeArg>>> {
        let next = self.peek();
        if next.kind == K::OpenAngle {
            // Eat the OpenAngle
            self.tokens.advance();
            let (type_expressions, _type_args_span) = self.eat_delimited(
                "Type arguments",
                K::Comma,
                K::CloseAngle,
                Parser::expect_type_expression,
            )?;
            // TODO named type arguments
            let type_args: Vec<_> = type_expressions
                .into_iter()
                .map(|type_expr| FnCallTypeArg { name: None, type_expr })
                .collect();
            Ok(Some(type_args))
        } else {
            Ok(None)
        }
    }

    /// Base expression meaning no postfix or binary ops
    fn parse_base_expression(&mut self) -> ParseResult<Option<ExpressionId>> {
        let (first, second, third) = self.tokens.peek_three();
        trace!("parse_expression {} {}", first.kind, second.kind);
        if let Some(lit) = self.parse_literal()? {
            let literal_id = self.add_expression(ParsedExpression::Literal(lit));
            return Ok(Some(literal_id));
        }
        if first.kind == K::OpenParen {
            self.tokens.advance();
            let expr = self.expect_expression()?;
            // TODO: If comma, parse a tuple
            self.expect_eat_token(K::CloseParen)?;
            Ok(Some(expr))
        } else if first.kind == K::KeywordFor {
            self.tokens.advance();
            let binding = if third.kind == K::KeywordIn {
                if second.kind != K::Ident {
                    return Err(Parser::error(
                        "Expected identifiers between for and in keywords",
                        second,
                    ));
                }
                let binding_ident = self.intern_ident_token(second);
                self.tokens.advance();
                self.tokens.advance();
                Some(binding_ident)
            } else {
                None
            };
            let iterable_expr = self.expect_expression()?;
            let expr_type_keyword = self.tokens.peek();
            let for_expr_type = if expr_type_keyword.kind == K::KeywordYield {
                Ok(ForExprType::Yield)
            } else if expr_type_keyword.kind == K::KeywordDo {
                Ok(ForExprType::Do)
            } else {
                Err(Parser::error("Expected yield or do keyword", expr_type_keyword))
            }?;
            self.tokens.advance();
            let body_expr = self.expect_block()?;
            let span = first.span.extended(body_expr.span);
            Ok(Some(self.add_expression(ParsedExpression::For(ForExpr {
                iterable_expr,
                binding,
                body_block: body_expr,
                expr_type: for_expr_type,
                span,
            }))))
        } else if first.kind.is_prefix_operator() {
            let Some(op_kind) = UnaryOpKind::from_tokenkind(first.kind) else {
                return Err(Parser::error("unexpected prefix operator", first));
            };
            self.tokens.advance();
            let expr = self.expect_expression()?;
            let span = first.span.extended(self.get_expression(expr).get_span());
            Ok(Some(self.add_expression(ParsedExpression::UnaryOp(UnaryOp {
                expr,
                op_kind,
                span,
            }))))
        } else if first.kind == K::Ident {
            // FnCall
            // Here we use is_whitespace_preceeded to distinguish between:
            // square<int>(42) -> FnCall
            // square < int > (42) -> square LESS THAN int GREATER THAN (42)
            let mut namespaces = Vec::new();
            if second.kind == K::Colon
                && third.kind == K::Colon
                && !second.is_whitespace_preceeded()
            {
                // Namespaced expression; foo::
                // Loop until we don't see a ::
                namespaces.push(self.intern_ident_token(first));
                self.tokens.advance(); // ident
                self.tokens.advance(); // colon
                self.tokens.advance(); // colon
                loop {
                    trace!("Parsing namespaces {:?}", namespaces);
                    let (a, b, c) = self.tokens.peek_three();
                    trace!("Parsing namespaces peeked 3 {} {} {}", a.kind, b.kind, c.kind);
                    if a.kind == K::Ident && b.kind == K::Colon && c.kind == K::Colon {
                        self.tokens.advance(); // ident
                        self.tokens.advance(); // colon
                        self.tokens.advance(); // colon
                        namespaces.push(self.intern_ident_token(a));
                    } else {
                        break;
                    }
                }
            }
            let (first, second) = self.tokens.peek_two();
            if (second.kind == K::OpenAngle && !second.is_whitespace_preceeded())
                || second.kind == K::OpenParen
            {
                trace!("parse_expression FnCall");
                // Eat the name
                self.tokens.advance();
                let type_args = self.parse_optional_type_args()?;
                self.expect_eat_token(K::OpenParen)?;
                let (args, args_span) = self.eat_delimited(
                    "Function arguments",
                    K::Comma,
                    K::CloseParen,
                    Parser::expect_fn_arg,
                )?;
                let name = self.intern_ident_token(first);
                Ok(Some(self.add_expression(ParsedExpression::FnCall(FnCall {
                    name,
                    type_args,
                    args,
                    namespaces,
                    span: first.span.extended(args_span),
                }))))
            } else {
                // The last thing it can be is a simple variable reference expression
                self.tokens.advance();
                let name = self.intern_ident_token(first);
                Ok(Some(self.add_expression(ParsedExpression::Variable(Variable {
                    name,
                    namespaces,
                    span: first.span,
                }))))
            }
        } else if first.kind == K::OpenBrace {
            // The syntax {} means empty record, not empty block
            // If you want a void or empty block, the required syntax is { () }
            trace!("parse_expr {:?} {:?} {:?}", first, second, third);
            if second.kind == K::CloseBrace {
                let span = first.span.extended(second.span);
                Ok(Some(
                    self.add_expression(ParsedExpression::Record(Record { fields: vec![], span })),
                ))
            } else if second.kind == K::Ident && third.kind == K::Colon {
                let record = Parser::expect("record", first, self.parse_record())?;
                Ok(Some(self.add_expression(ParsedExpression::Record(record))))
            } else {
                match self.parse_block()? {
                    None => Err(Parser::error("block", self.peek())),
                    Some(block) => Ok(Some(self.add_expression(ParsedExpression::Block(block)))),
                }
            }
        } else if first.kind == K::KeywordIf {
            let if_expr = Parser::expect("If Expression", first, self.parse_if_expr())?;
            Ok(Some(self.add_expression(ParsedExpression::If(if_expr))))
        } else if first.kind == K::OpenBracket {
            // Array
            let start = self.expect_eat_token(K::OpenBracket)?;
            let (elements, span) = self.eat_delimited(
                "Array elements",
                TokenKind::Comma,
                TokenKind::CloseBracket,
                |p| Parser::expect("expression", start, p.parse_expression()),
            )?;
            let span = start.span.extended(span);
            Ok(Some(self.add_expression(ParsedExpression::Array(ArrayExpr { elements, span }))))
        } else {
            // More expression types
            Ok(None)
        }
    }

    fn parse_mut(&mut self) -> ParseResult<Option<ValDef>> {
        self.parse_val(true)
    }

    fn parse_val(&mut self, mutable: bool) -> ParseResult<Option<ValDef>> {
        trace!("parse_val");
        let keyword = if mutable { K::KeywordMut } else { K::KeywordVal };
        let Some(eaten_keyword) = self.eat_token(keyword) else {
            return Ok(None);
        };
        let name_token = self.expect_eat_token(K::Ident)?;
        let typ = match self.eat_token(K::Colon) {
            None => Ok(None),
            Some(_) => self.parse_type_expression(),
        }?;
        self.expect_eat_token(K::Equals)?;
        let initializer_expression =
            Parser::expect("expression", self.peek(), self.parse_expression())?;
        let span =
            eaten_keyword.span.extended(self.get_expression(initializer_expression).get_span());
        Ok(Some(ValDef {
            name: self.intern_ident_token(name_token),
            type_id: typ,
            value: initializer_expression,
            is_mutable: mutable,
            span,
        }))
    }

    fn parse_const(&mut self) -> ParseResult<Option<ConstVal>> {
        trace!("parse_const");
        let Some(keyword_val_token) = self.eat_token(K::KeywordVal) else {
            return Ok(None);
        };
        let name_token = self.expect_eat_token(K::Ident)?;
        let _colon = self.expect_eat_token(K::Colon);
        let typ = Parser::expect("type_expression", self.peek(), self.parse_type_expression())?;
        self.expect_eat_token(K::Equals)?;
        let value_expr = Parser::expect("expression", self.peek(), self.parse_expression())?;
        let span = keyword_val_token.span.extended(self.get_expression(value_expr).get_span());
        Ok(Some(ConstVal {
            name: self.intern_ident_token(name_token),
            ty: typ,
            value_expr,
            span,
            definition_id: self.next_definition_id(),
        }))
    }

    fn parse_assignment(&mut self, lhs: ExpressionId) -> ParseResult<Assignment> {
        let _valid_lhs = match &*self.get_expression(lhs) {
            ParsedExpression::FieldAccess(_) => true,
            ParsedExpression::Variable(_) => true,
            ParsedExpression::IndexOperation(_) => true,
            _ => false,
        };
        self.expect_eat_token(K::Equals)?;
        let rhs = self.expect_expression()?;
        let span = self.extended_span(lhs, rhs);
        Ok(Assignment { lhs, rhs, span })
    }

    fn eat_fn_arg_def(&mut self) -> ParseResult<FnArgDef> {
        trace!("eat_fn_arg_def");
        let name_token = self.expect_eat_token(K::Ident)?;
        self.expect_eat_token(K::Colon)?;
        let typ = Parser::expect("type_expression", self.peek(), self.parse_type_expression())?;
        let span = name_token.span.extended(typ.get_span());
        Ok(FnArgDef { name: self.intern_ident_token(name_token), ty: typ, span })
    }

    fn eat_fndef_args(&mut self) -> ParseResult<(Vec<FnArgDef>, Span)> {
        self.eat_delimited("Function arguments", K::Comma, K::CloseParen, Parser::eat_fn_arg_def)
    }

    fn eat_delimited<T, F>(
        &mut self,
        name: &str,
        delim: TokenKind,
        terminator: TokenKind,
        parse: F,
    ) -> ParseResult<(Vec<T>, Span)>
    where
        F: Fn(&mut Parser<'toks, 'module>) -> ParseResult<T>,
    {
        trace!("eat_delimited delim='{}' terminator='{}'", delim, terminator);
        // TODO @Allocation Use smallvec
        let mut v = Vec::with_capacity(32);
        let mut span = self.peek().span;

        loop {
            if let Some(terminator) = self.eat_token(terminator) {
                trace!("eat_delimited found terminator after {} results.", v.len());
                span.end = terminator.span.end;
                break Ok((v, span));
            }
            match parse(self) {
                Ok(parsed) => {
                    v.push(parsed);
                    trace!("eat_delimited got result {}", v.len());
                    if let Some(terminator) = self.eat_token(terminator) {
                        trace!("eat_delimited found terminator after {} results.", v.len());
                        span.end = terminator.span.end;
                        break Ok((v, span));
                    }
                    let found_delim = self.eat_token(delim);
                    if found_delim.is_none() {
                        trace!("eat_delimited missing delimiter.");
                        break Err(Parser::error(delim, self.peek()));
                    }
                }
                Err(e) => {
                    // trace!("eat_delimited got err from 'parse': {}", e);
                    break Err(Parser::error_cause(
                        format!("Failed to parse {} separated by '{delim}' and terminated by '{terminator}'", name),
                        self.peek(),
                        e,
                    ));
                }
            }
        }
    }

    fn parse_if_expr(&mut self) -> ParseResult<Option<IfExpr>> {
        let Some(if_keyword) = self.eat_token(TokenKind::KeywordIf) else { return Ok(None) };
        let condition_expr =
            Parser::expect("conditional expression", if_keyword, self.parse_expression())?;
        let optional_ident = if self.peek().kind == K::Pipe {
            self.tokens.advance();
            let ident = self.expect_eat_token(K::Ident)?;
            self.expect_eat_token(K::Pipe)?;
            Some((self.intern_ident_token(ident), ident.span))
        } else {
            None
        };
        let consequent_expr =
            Parser::expect("block following condition", if_keyword, self.parse_expression())?;
        let else_peek = self.peek();
        let alt = if else_peek.kind == K::KeywordElse {
            self.tokens.advance();
            let alt_result = Parser::expect("else block", else_peek, self.parse_expression())?;
            Some(alt_result)
        } else {
            None
        };
        let end_span = alt
            .as_ref()
            .map(|a| self.get_expression(*a).get_span())
            .unwrap_or(self.get_expression(consequent_expr).get_span());
        let span = if_keyword.span.extended(end_span);
        let if_expr =
            IfExpr { cond: condition_expr, optional_ident, cons: consequent_expr, alt, span };
        Ok(Some(if_expr))
    }

    fn parse_while_loop(&mut self) -> ParseResult<Option<WhileStmt>> {
        let while_token = self.peek();
        if while_token.kind != K::KeywordWhile {
            return Ok(None);
        }
        self.tokens.advance();
        let cond = self.expect_expression()?;
        let block = Parser::expect("block for while loop", while_token, self.parse_block())?;
        let span = while_token.span.extended(block.span);
        Ok(Some(WhileStmt { cond, block, span }))
    }

    fn parse_statement(&mut self) -> ParseResult<Option<BlockStmt>> {
        trace!("eat_statement {:?}", self.peek());
        if let Some(while_loop) = self.parse_while_loop()? {
            Ok(Some(BlockStmt::While(while_loop)))
        } else if let Some(mut_def) = self.parse_mut()? {
            Ok(Some(BlockStmt::ValDef(mut_def)))
        } else if let Some(val_def) = self.parse_val(false)? {
            Ok(Some(BlockStmt::ValDef(val_def)))
        } else if let Some(expr) = self.parse_expression()? {
            let peeked = self.peek();
            // Assignment:
            // - Validate expr type, since only some exprs can be LHS of an assignment
            // - Build assignment
            if peeked.kind == K::Equals {
                let assgn = self.parse_assignment(expr)?;
                Ok(Some(BlockStmt::Assignment(assgn)))
            } else {
                Ok(Some(BlockStmt::LoneExpression(expr)))
            }
        } else {
            Ok(None)
        }
    }

    fn parse_block(&mut self) -> ParseResult<Option<Block>> {
        let Some(block_start) = self.eat_token(K::OpenBrace) else {
            return Ok(None);
        };
        let closure =
            |p: &mut Parser| Parser::expect("statement", p.peek(), Parser::parse_statement(p));
        let (block_statements, statements_span) =
            self.eat_delimited("Block statements", K::Semicolon, K::CloseBrace, closure)?;
        let span = block_start.span.extended(statements_span);
        Ok(Some(Block { stmts: block_statements, span }))
    }

    fn expect_type_param(&mut self) -> ParseResult<TypeParamDef> {
        let s = self.expect_eat_token(K::Ident)?;
        let ident_id = self.intern_ident_token(s);
        Ok(TypeParamDef { ident: ident_id, span: s.span })
    }

    fn parse_function(&mut self) -> ParseResult<Option<FnDef>> {
        trace!("parse_fndef");
        let is_intrinsic = if self.peek().kind == K::KeywordIntern {
            self.tokens.advance();
            true
        } else {
            false
        };
        let linkage = if is_intrinsic {
            Linkage::Intrinsic
        } else if !is_intrinsic && self.peek().kind == K::KeywordExtern {
            self.tokens.advance();
            Linkage::External
        } else {
            Linkage::Standard
        };

        let Some(fn_keyword) = self.eat_token(K::KeywordFn) else {
            return if is_intrinsic {
                Err(ParseError { expected: "fn".to_string(), token: self.peek(), cause: None })
            } else {
                Ok(None)
            };
        };
        let func_name = self.expect_eat_token(K::Ident)?;
        let func_name_id = self.intern_ident_token(func_name);
        let type_arguments: Option<Vec<TypeParamDef>> =
            if let TokenKind::OpenAngle = self.peek().kind {
                self.tokens.advance();
                let (type_args, _type_arg_span) = self.eat_delimited(
                    "Type arguments",
                    TokenKind::Comma,
                    TokenKind::CloseAngle,
                    Parser::expect_type_param,
                )?;
                Some(type_args)
            } else {
                None
            };
        self.expect_eat_token(K::OpenParen)?;
        let (args, args_span) = self.eat_fndef_args()?;
        self.expect_eat_token(K::Colon)?;
        let ret_type = self.parse_type_expression()?;
        let block = self.parse_block()?;
        let mut span = fn_keyword.span;
        span.end = block.as_ref().map(|b| b.span.end).unwrap_or(args_span.end);
        Ok(Some(FnDef {
            name: func_name_id,
            type_args: type_arguments,
            args,
            ret_type,
            block,
            span,
            linkage,
            definition_id: self.next_definition_id(),
        }))
    }
    fn next_definition_id(&mut self) -> AstDefinitionId {
        let id = self.parsed_module.ast_id_index;
        self.parsed_module.ast_id_index += 1;
        id
    }

    fn parse_typedef(&mut self) -> ParseResult<Option<TypeDefn>> {
        let keyword_type = self.eat_token(K::KeywordType);
        if let Some(keyword_type) = keyword_type {
            let name = self.expect_eat_token(K::Ident)?;
            let equals = self.expect_eat_token(K::Equals)?;
            let type_expr =
                Parser::expect("Type expression", equals, self.parse_type_expression())?;
            let span = keyword_type.span.extended(type_expr.get_span());
            Ok(Some(TypeDefn {
                name: self.intern_ident_token(name),
                value_expr: type_expr,
                span,
                definition_id: self.next_definition_id(),
            }))
        } else {
            Ok(None)
        }
    }

    fn parse_namespace(&mut self) -> ParseResult<Option<ParsedNamespace>> {
        let next = self.peek();
        if next.kind != K::KeywordNamespace {
            return Ok(None);
        };
        self.tokens.advance();
        let ident = self.expect_eat_token(K::Ident)?;
        self.expect_eat_token(K::OpenBrace)?;
        let mut definitions = Vec::new();
        while let Some(def) = self.parse_definition()? {
            definitions.push(def);
        }
        self.expect_eat_token(K::CloseBrace)?;
        Ok(Some(ParsedNamespace {
            name: self.intern_ident_token(ident),
            definitions,
            definition_id: self.next_definition_id(),
        }))
    }

    fn parse_definition(&mut self) -> ParseResult<Option<Definition>> {
        if let Some(ns) = self.parse_namespace()? {
            Ok(Some(Definition::Namespace(ns.into())))
        } else if let Some(const_def) = self.parse_const()? {
            self.expect_eat_token(K::Semicolon)?;
            Ok(Some(Definition::Const(const_def.into())))
        } else if let Some(fn_def) = self.parse_function()? {
            Ok(Some(Definition::FnDef(fn_def.into())))
        } else if let Some(type_def) = self.parse_typedef()? {
            Ok(Some(Definition::TypeDef(type_def.into())))
        } else {
            Ok(None)
        }
    }

    pub fn parse_module(&mut self) -> ParseResult<()> {
        let mut defs: Vec<Definition> = vec![];

        while let Some(def) = self.parse_definition()? {
            defs.push(def)
        }
        self.parsed_module.defs.extend(defs);

        Ok(())
    }
}

#[allow(unused)]
pub fn print_tokens(content: &str, tokens: &[Token]) {
    let mut line_idx = 0;
    for tok in tokens.iter() {
        if tok.span.line > line_idx {
            line_idx += 1;
            println!()
        }
        if tok.kind == K::Ident {
            print!("{}", &content[tok.span.start as usize..tok.span.end as usize]);
        } else if tok.kind.is_keyword() {
            print!("{} ", tok.kind);
        } else {
            print!("{}", tok.kind);
        }
    }
    println!()
}

pub fn lex_text(text: &str, file_id: FileId) -> ParseResult<Vec<Token>> {
    let mut lexer = Lexer::make(text, file_id);
    let tokens = lexer.run().map_err(|lex_error| ParseError {
        expected: lex_error.msg,
        token: EOF_TOKEN,
        cause: None,
    })?;

    let token_vec: Vec<Token> =
        tokens.into_iter().filter(|token| token.kind != K::LineComment).collect();
    Ok(token_vec)
}

#[cfg(test)]
pub fn parse_module(source: Rc<Source>) -> ParseResult<ParsedModule> {
    let module_name = source.filename.split('.').next().unwrap().to_string();
    let mut module = ParsedModule::make(module_name);

    let token_vec = lex_text(&source.content, source.file_id)?;
    let mut parser = Parser::make(&token_vec, source, &mut module);

    let result = parser.parse_module();
    if let Err(e) = result {
        parser.print_error(&e);
        Err(e)
    } else {
        Ok(module)
    }
}

// pub fn print_ast(ast: &Module, identifiers: &Identifiers) -> Result<String, std::fmt::Error> {
//     let mut output = String::new();
//     output.write_str(&ast.name)?;
//     output.write_str("\n")?;
//     for def in &ast.defs {
//         match def {
//             Definition::Type(type_def) => output
//                 .write_fmt(format_args!("Type {} {:?}", type_def.name, type_def.value_expr))?,
//             Definition::FnDef(fn_def) => {
//                 output.write_fmt(format_args!("Function {} {:?}", fn_def.name, fn_def.))?
//             }
//         }
//     }
//     output
// }
