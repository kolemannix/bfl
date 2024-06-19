use std::collections::HashMap;
use std::fmt::{Display, Formatter, Write};

use log::trace;
use string_interner::Symbol;

use crate::lex::*;
use crate::typer::{BinaryOpKind, Linkage, UnaryOpKind};
use TokenKind as K;

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedTypeDefnId(u32);
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedFunctionId(u32);
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedConstantId(u32);
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedAbilityId(u32);
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedAbilityImplId(u32);
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedNamespaceId(u32);
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedExpressionId(u32);
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedTypeExpressionId(u32);
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub struct ParsedPatternId(u32);

pub type FileId = u32;

#[cfg(test)]
mod parse_test;

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Hash)]
pub enum ParsedId {
    Function(ParsedFunctionId),
    TypeDefn(ParsedTypeDefnId),
    Namespace(ParsedNamespaceId),
    Ability(ParsedAbilityId),
    AbilityImpl(ParsedAbilityImplId),
    Constant(ParsedConstantId),
    Expression(ParsedExpressionId),
    TypeExpression(ParsedTypeExpressionId),
    Pattern(ParsedPatternId),
}

impl From<ParsedTypeExpressionId> for ParsedId {
    fn from(id: ParsedTypeExpressionId) -> Self {
        ParsedId::TypeExpression(id)
    }
}

impl From<ParsedExpressionId> for ParsedId {
    fn from(id: ParsedExpressionId) -> Self {
        ParsedId::Expression(id)
    }
}

impl ParsedId {
    pub fn is_definition(&self) -> bool {
        match self {
            ParsedId::Function(_) => true,
            ParsedId::TypeDefn(_) => true,
            ParsedId::Ability(_) => true,
            ParsedId::AbilityImpl(_) => true,
            ParsedId::Constant(_) => true,
            ParsedId::Namespace(_) => true,
            ParsedId::Expression(_) => false,
            ParsedId::TypeExpression(_) => false,
            ParsedId::Pattern(_) => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArrayExpr {
    pub elements: Vec<ParsedExpressionId>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub enum Literal {
    None(SpanId),
    Unit(SpanId),
    Char(u8, SpanId),
    Numeric(String, SpanId),
    Bool(bool, SpanId),
    /// TODO: Need second span, "content_span_id"
    String(String, SpanId),
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
    pub fn get_span(&self) -> SpanId {
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
#[derive(Debug, Clone)]
pub struct Identifiers {
    intern_pool: string_interner::StringInterner,
}
impl Identifiers {
    pub const BUILTIN_IDENTS: [&'static str; 7] =
        ["unit", "char", "string", "length", "iteree", "it_index", "as"];

    pub fn intern(&mut self, s: impl AsRef<str>) -> IdentifierId {
        let s = self.intern_pool.get_or_intern(&s);
        IdentifierId(s)
    }
    pub fn get(&self, s: impl AsRef<str>) -> Option<IdentifierId> {
        match self.intern_pool.get(&s) {
            Some(s) => Some(IdentifierId(s)),
            None => None,
        }
    }
    pub fn get_name(&self, id: IdentifierId) -> &str {
        self.intern_pool.resolve(id.0).expect("failed to resolve identifier")
    }
}

impl Default for Identifiers {
    fn default() -> Self {
        let intern_pool = string_interner::StringInterner::default();
        let mut this = Identifiers { intern_pool };
        for builtin_ident in Identifiers::BUILTIN_IDENTS.iter() {
            this.intern(builtin_ident);
        }
        this
    }
}

#[derive(Debug, Clone)]
pub struct FnCallArg {
    pub name: Option<IdentifierId>,
    pub value: ParsedExpressionId,
}

#[derive(Debug, Clone)]
pub struct FnCallTypeArg {
    pub name: Option<IdentifierId>,
    pub type_expr: ParsedTypeExpressionId,
}

#[derive(Debug, Clone)]
pub struct FnCall {
    pub name: IdentifierId,
    pub type_args: Option<Vec<FnCallTypeArg>>,
    pub args: Vec<FnCallArg>,
    pub namespaces: Vec<IdentifierId>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ValDef {
    pub name: IdentifierId,
    pub type_expr: Option<ParsedTypeExpressionId>,
    pub value: ParsedExpressionId,
    pub is_mutable: bool,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct BinaryOp {
    pub op_kind: BinaryOpKind,
    pub lhs: ParsedExpressionId,
    pub rhs: ParsedExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct UnaryOp {
    pub op_kind: UnaryOpKind,
    pub expr: ParsedExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct Variable {
    pub name: IdentifierId,
    pub namespaces: Vec<IdentifierId>,
    pub span: SpanId,
}

impl Display for Variable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("var#{}", self.name))
    }
}

#[derive(Debug, Clone)]
pub struct FieldAccess {
    pub base: ParsedExpressionId,
    pub target: IdentifierId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: IdentifierId,
    pub expr: ParsedExpressionId,
}

#[derive(Debug, Clone)]
/// Example:
/// { foo: 1, bar: false }
///   ^................^ fields
pub struct Struct {
    pub fields: Vec<StructField>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
/// Example: users  [42]
///          ^target ^index_value
pub struct IndexOperation {
    pub target: ParsedExpressionId,
    pub index_expr: ParsedExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct MethodCall {
    pub base: ParsedExpressionId,
    pub call: Box<FnCall>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct OptionalGet {
    pub base: ParsedExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TagExpr {
    pub tag: IdentifierId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedEnumConstructor {
    pub tag: IdentifierId,
    pub payload: ParsedExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedIsExpression {
    pub target_expression: ParsedExpressionId,
    pub pattern: ParsedPatternId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedMatchCase {
    pub pattern: ParsedPatternId,
    pub expression: ParsedExpressionId,
}

#[derive(Debug, Clone)]
pub struct ParsedMatchExpression {
    pub target_expression: ParsedExpressionId,
    pub cases: Vec<ParsedMatchCase>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub enum ParsedExpression {
    BinaryOp(BinaryOp),                     // a == b
    UnaryOp(UnaryOp),                       // !b, *b
    Literal(Literal),                       // 42, "asdf"
    FnCall(FnCall),                         // square(1, 2)
    Variable(Variable),                     // x
    FieldAccess(FieldAccess),               // x.b
    MethodCall(MethodCall),                 // x.load()
    Block(Block),                           // { <expr>; <expr>; <expr> }
    If(IfExpr),                             // if a else b
    Struct(Struct),                         // { x: 1, y: 3 }
    IndexOperation(IndexOperation),         // xs[3]
    Array(ArrayExpr),                       // [1, 3, 5, 7]
    OptionalGet(OptionalGet),               // foo!
    For(ForExpr),                           // for i in [1,2,3] do println(i)
    Tag(TagExpr),                           // .A
    EnumConstructor(ParsedEnumConstructor), // .A(<expr>)
    Is(ParsedIsExpression),                 // x is T
    Match(ParsedMatchExpression),           // when x is {
                                            // | a => ...
                                            // | b => ...
                                            // }
}

impl ParsedExpression {
    pub fn is_literal(e: &ParsedExpression) -> bool {
        matches!(e, ParsedExpression::Literal(_))
    }
    pub fn expect_literal(&self) -> &Literal {
        match self {
            ParsedExpression::Literal(lit) => lit,
            _ => panic!("expected literal"),
        }
    }
    #[inline]
    pub fn get_span(&self) -> SpanId {
        match self {
            Self::BinaryOp(op) => op.span,
            Self::UnaryOp(op) => op.span,
            Self::Literal(lit) => lit.get_span(),
            Self::FnCall(call) => call.span,
            Self::Variable(var) => var.span,
            Self::FieldAccess(acc) => acc.span,
            Self::MethodCall(call) => call.span,
            Self::Block(block) => block.span,
            Self::If(if_expr) => if_expr.span,
            Self::Struct(struc) => struc.span,
            Self::IndexOperation(op) => op.span,
            Self::Array(array_expr) => array_expr.span,
            Self::OptionalGet(optional_get) => optional_get.span,
            Self::For(for_expr) => for_expr.span,
            Self::Tag(tag_expr) => tag_expr.span,
            Self::EnumConstructor(e) => e.span,
            Self::Is(is_expr) => is_expr.span,
            Self::Match(match_expr) => match_expr.span,
        }
    }

    pub fn is_assignable(&self) -> bool {
        match self {
            Self::Variable(_var) => true,
            Self::IndexOperation(_op) => true,
            Self::FieldAccess(_acc) => true,
            Self::MethodCall(_call) => false,
            Self::BinaryOp(_op) => false,
            Self::UnaryOp(_op) => false,
            Self::Literal(_lit) => false,
            Self::FnCall(_call) => false,
            Self::Block(_block) => false,
            Self::If(_if_expr) => false,
            Self::Struct(_struct) => false,
            Self::Array(_array_expr) => false,
            Self::OptionalGet(_optional_get) => false,
            Self::For(_) => false,
            Self::Tag(_) => false,
            Self::EnumConstructor(_) => false,
            Self::Is(_) => false,
            Self::Match(_) => false,
        }
    }

    pub fn as_match(&self) -> Option<&ParsedMatchExpression> {
        if let Self::Match(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn as_index_operation(&self) -> Option<&IndexOperation> {
        if let Self::IndexOperation(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

enum ExprStackMember {
    Operator(BinaryOpKind, SpanId),
    Expr(ParsedExpressionId),
}

impl ExprStackMember {
    fn expect_expr(self) -> ParsedExpressionId {
        match self {
            ExprStackMember::Expr(expr) => expr,
            _ => panic!("expected expr"),
        }
    }
    fn expect_operator(self) -> (BinaryOpKind, SpanId) {
        match self {
            ExprStackMember::Operator(kind, span) => (kind, span),
            _ => panic!("expected operator"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedStructPattern {
    pub fields: Vec<(IdentifierId, ParsedPatternId)>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedEnumPattern {
    pub variant_tag: IdentifierId,
    pub payload_pattern: Option<ParsedPatternId>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedSomePattern {
    pub inner_pattern: ParsedPatternId,
    pub span: SpanId,
}

// https://bnfplayground.pauliankline.com/?bnf=%3Cpattern%3E%20%3A%3A%3D%20%3Cliteral%3E%20%7C%20%3Cvariable%3E%20%7C%20%3Cenum%3E%20%7C%20%3Cstruct%3E%0A%3Cliteral%3E%20%3A%3A%3D%20%22(%22%20%22)%22%20%7C%20%22%5C%22%22%20%3Cident%3E%20%22%5C%22%22%20%7C%20%5B0-9%5D%2B%20%7C%20%22%27%22%20%5Ba-z%5D%20%22%27%22%20%7C%20%22None%22%0A%3Cvariable%3E%20%3A%3A%3D%20%3Cident%3E%0A%3Cident%3E%20%3A%3A%3D%20%5Ba-z%5D*%0A%3Cenum%3E%20%3A%3A%3D%20%22.%22%20%3Cident%3E%20(%20%22(%22%20%3Cpattern%3E%20%22)%22%20)%3F%0A%3Cstruct%3E%20%3A%3A%3D%20%22%7B%22%20(%20%3Cident%3E%20%22%3A%20%22%20%3Cpattern%3E%20%22%2C%22%3F%20)*%20%22%7D%22%20&name=
// <pattern> ::= <literal> | <variable> | <enum> | <struct>
// <literal> ::= "(" ")" | "\"" <ident> "\"" | [0-9]+ | "'" [a-z] "'" | "None"
// <variable> ::= <ident>
// <ident> ::= [a-z]*
// <enum> ::= "." <ident> ( "(" <pattern> ")" )?
// <struct> ::= "{" ( <ident> ": " <pattern> ","? )* "}"
#[derive(Debug, Clone)]
pub enum ParsedPattern {
    Literal(ParsedExpressionId),
    Variable(IdentifierId, SpanId),
    Struct(ParsedStructPattern),
    Enum(ParsedEnumPattern),
    Wildcard(SpanId),
    Some(ParsedSomePattern),
    // Tag(TagExpr),
    // Array(ArrayExpr),
}

impl ParsedPattern {}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub lhs: ParsedExpressionId,
    pub rhs: ParsedExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct IfExpr {
    pub cond: ParsedExpressionId,
    pub cons: ParsedExpressionId,
    pub alt: Option<ParsedExpressionId>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub cond: ParsedExpressionId,
    pub block: Block,
    /// Maybe its better not to store a span on nodes for which a span is trivially calculated
    pub span: SpanId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForExprType {
    Yield,
    Do,
}

#[derive(Debug, Clone)]
pub struct ForExpr {
    pub iterable_expr: ParsedExpressionId,
    pub binding: Option<IdentifierId>,
    pub body_block: Block,
    pub expr_type: ForExprType,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub enum BlockStmt {
    ValDef(ValDef),                     // val x = 42
    Assignment(Assignment),             // x = 42
    LoneExpression(ParsedExpressionId), // println("asdfasdf")
    While(WhileStmt),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<BlockStmt>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct StructTypeField {
    pub name: IdentifierId,
    pub ty: ParsedTypeExpressionId,
}

#[derive(Debug, Clone)]
pub struct StructType {
    pub fields: Vec<StructTypeField>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypeApplication {
    pub base: IdentifierId,
    pub params: Vec<ParsedTypeExpressionId>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedOptional {
    pub base: ParsedTypeExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedReference {
    pub base: ParsedTypeExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedEnumVariant {
    pub tag_name: IdentifierId,
    pub payload_expression: Option<ParsedTypeExpressionId>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedEnumType {
    pub variants: Vec<ParsedEnumVariant>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedDotMemberAccess {
    pub base: ParsedTypeExpressionId,
    pub member_name: IdentifierId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub enum ParsedTypeExpression {
    Unit(SpanId),
    Char(SpanId),
    Int(SpanId),
    Bool(SpanId),
    String(SpanId),
    Struct(StructType),
    Name(IdentifierId, SpanId),
    TagName(IdentifierId, SpanId),
    TypeApplication(TypeApplication),
    Optional(ParsedOptional),
    Reference(ParsedReference),
    Enum(ParsedEnumType),
    DotMemberAccess(ParsedDotMemberAccess),
}

impl ParsedTypeExpression {
    #[inline]
    pub fn is_int(&self) -> bool {
        matches!(self, ParsedTypeExpression::Int(_))
    }

    #[inline]
    pub fn is_string(&self) -> bool {
        matches!(self, ParsedTypeExpression::String(_))
    }
    #[inline]
    pub fn get_span(&self) -> SpanId {
        match self {
            ParsedTypeExpression::Unit(span) => *span,
            ParsedTypeExpression::Char(span) => *span,
            ParsedTypeExpression::Int(span) => *span,
            ParsedTypeExpression::Bool(span) => *span,
            ParsedTypeExpression::String(span) => *span,
            ParsedTypeExpression::Struct(struc) => struc.span,
            ParsedTypeExpression::Name(_, span) => *span,
            ParsedTypeExpression::TagName(_, span) => *span,
            ParsedTypeExpression::TypeApplication(app) => app.span,
            ParsedTypeExpression::Optional(opt) => opt.span,
            ParsedTypeExpression::Reference(r) => r.span,
            ParsedTypeExpression::Enum(e) => e.span,
            ParsedTypeExpression::DotMemberAccess(a) => a.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypeParamDef {
    pub ident: IdentifierId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedFunction {
    pub name: IdentifierId,
    pub type_args: Option<Vec<TypeParamDef>>,
    pub args: Vec<FnArgDef>,
    pub ret_type: Option<ParsedTypeExpressionId>,
    pub block: Option<Block>,
    pub span: SpanId,
    pub linkage: Linkage,
    pub id: ParsedFunctionId,
}

#[derive(Debug, Clone)]
pub struct FnArgDef {
    pub name: IdentifierId,
    pub ty: ParsedTypeExpressionId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedConstant {
    pub name: IdentifierId,
    pub ty: ParsedTypeExpressionId,
    pub value_expr: ParsedExpressionId,
    pub span: SpanId,
    pub id: ParsedConstantId,
}

#[derive(Debug, Clone)]
pub struct ParsedTypeDefn {
    pub name: IdentifierId,
    pub value_expr: ParsedTypeExpressionId,
    pub span: SpanId,
    pub id: ParsedTypeDefnId,
}

#[derive(Debug, Clone)]
pub struct ParsedAbility {
    pub name: IdentifierId,
    pub functions: Vec<ParsedFunctionId>,
    pub span: SpanId,
    pub id: ParsedAbilityId,
}

#[derive(Debug, Clone)]
pub struct ParsedAbilityImplementation {
    pub ability_name: IdentifierId,
    pub target_type: ParsedTypeExpressionId,
    pub functions: Vec<ParsedFunctionId>,
    pub id: ParsedAbilityImplId,
    pub auto: bool,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ParsedNamespace {
    pub name: IdentifierId,
    pub definitions: Vec<ParsedId>,
    pub id: ParsedNamespaceId,
    pub span: SpanId,
}

#[derive(Debug, Default, Clone)]
pub struct ParsedExpressionPool {
    expressions: Vec<ParsedExpression>,
    type_hints: HashMap<ParsedExpressionId, ParsedTypeExpressionId>,
}
impl ParsedExpressionPool {
    pub fn add_type_hint(&mut self, id: ParsedExpressionId, ty: ParsedTypeExpressionId) {
        self.type_hints.insert(id, ty);
    }

    pub fn get_type_hint(&self, id: ParsedExpressionId) -> Option<ParsedTypeExpressionId> {
        self.type_hints.get(&id).copied()
    }

    pub fn add_expression(&mut self, expression: ParsedExpression) -> ParsedExpressionId {
        let id = self.expressions.len();
        self.expressions.push(expression);
        ParsedExpressionId(id as u32)
    }
    pub fn get_expression(&self, id: ParsedExpressionId) -> &ParsedExpression {
        &self.expressions[id.0 as usize]
    }
}

#[derive(Debug, Default, Clone)]
pub struct ParsedTypeExpressionPool {
    type_expressions: Vec<ParsedTypeExpression>,
}
impl ParsedTypeExpressionPool {
    pub fn add(&mut self, expression: ParsedTypeExpression) -> ParsedTypeExpressionId {
        let id = self.type_expressions.len();
        self.type_expressions.push(expression);
        ParsedTypeExpressionId(id as u32)
    }
    pub fn get(&self, id: ParsedTypeExpressionId) -> &ParsedTypeExpression {
        &self.type_expressions[id.0 as usize]
    }
}

#[derive(Debug, Default, Clone)]
pub struct ParsedPatternPool {
    patterns: Vec<ParsedPattern>,
}
impl ParsedPatternPool {
    pub fn add_pattern(&mut self, pattern: ParsedPattern) -> ParsedPatternId {
        let id = self.patterns.len();
        self.patterns.push(pattern);
        ParsedPatternId(id as u32)
    }
    pub fn get_pattern(&self, id: ParsedPatternId) -> &ParsedPattern {
        &self.patterns[id.0 as usize]
    }
}

#[derive(Debug, Default, Clone)]
pub struct Sources {
    sources: HashMap<FileId, Source>,
}

impl Sources {
    pub fn insert(&mut self, source: Source) {
        self.sources.insert(source.file_id, source);
    }

    pub fn get_main(&self) -> &Source {
        self.sources.get(&0).unwrap()
    }

    pub fn get_source(&self, file_id: FileId) -> &Source {
        self.sources.get(&file_id).unwrap()
    }

    pub fn get_line_for_span(&self, span: Span) -> Option<&Line> {
        self.sources.get(&span.file_id).unwrap().get_line_for_span(span)
    }

    pub fn get_span_content(&self, span: Span) -> &str {
        self.sources.get(&span.file_id).unwrap().get_span_content(span)
    }

    pub fn source_by_span(&self, span: Span) -> &Source {
        self.get_source(span.file_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (FileId, &Source)> {
        self.sources.iter().map(|(file_id, source)| (*file_id, source))
    }
}

#[derive(Debug, Clone)]
pub struct ParsedModule {
    pub name: String,
    pub name_id: IdentifierId,
    pub spans: Spans,
    pub functions: Vec<ParsedFunction>,
    pub constants: Vec<ParsedConstant>,
    pub type_defns: Vec<ParsedTypeDefn>,
    pub namespaces: Vec<ParsedNamespace>,
    pub abilities: Vec<ParsedAbility>,
    pub ability_impls: Vec<ParsedAbilityImplementation>,
    pub sources: Sources,
    /// Using RefCell here just so we can mutably access
    /// the identifiers without having mutable access to
    /// the entire AST module. Lets me wait to decide
    /// where things actually live
    ///
    /// After reading the Roc codebase, I think the move
    /// is to move away from these big structs and just have top-level functions
    /// so we can be more granular about what is mutable when. You can create an 'Env'
    /// struct to reduce the number of parameters, but this Env will also suffer from that problem sometimes
    pub identifiers: Identifiers,
    pub expressions: ParsedExpressionPool,
    pub type_expressions: ParsedTypeExpressionPool,
    pub patterns: ParsedPatternPool,
    pub errors: Vec<ParseError>,
}

impl ParsedModule {
    pub fn make(name: String) -> ParsedModule {
        let mut identifiers = Identifiers::default();
        let name_id = identifiers.intern(&name);
        ParsedModule {
            name,
            name_id,
            spans: Spans::new(),
            functions: Vec::new(),
            constants: Vec::new(),
            type_defns: Vec::new(),
            namespaces: Vec::new(),
            abilities: Vec::new(),
            ability_impls: Vec::new(),
            sources: Sources::default(),
            identifiers,
            expressions: ParsedExpressionPool::default(),
            type_expressions: ParsedTypeExpressionPool::default(),
            patterns: ParsedPatternPool::default(),
            errors: Vec::new(),
        }
    }

    pub fn get_function(&self, id: ParsedFunctionId) -> &ParsedFunction {
        &self.functions[id.0 as usize]
    }

    pub fn add_function(&mut self, mut function: ParsedFunction) -> ParsedFunctionId {
        let id = self.functions.len();
        let id = ParsedFunctionId(id as u32);
        function.id = id;
        self.functions.push(function);
        id
    }

    pub fn get_namespace(&self, id: ParsedNamespaceId) -> &ParsedNamespace {
        &self.namespaces[id.0 as usize]
    }

    pub fn get_namespace_mut(&mut self, id: ParsedNamespaceId) -> &mut ParsedNamespace {
        &mut self.namespaces[id.0 as usize]
    }

    pub fn add_namespace(&mut self, mut namespace: ParsedNamespace) -> ParsedNamespaceId {
        let id = ParsedNamespaceId(self.namespaces.len() as u32);
        namespace.id = id;
        self.namespaces.push(namespace);
        id
    }

    pub fn get_pattern_span(&self, id: ParsedPatternId) -> SpanId {
        match self.patterns.get_pattern(id) {
            ParsedPattern::Literal(literal_id) => {
                self.expressions.get_expression(*literal_id).get_span()
            }
            ParsedPattern::Enum(enum_pattern) => enum_pattern.span,
            ParsedPattern::Variable(_var_pattern, span) => *span,
            ParsedPattern::Struct(struct_pattern) => struct_pattern.span,
            ParsedPattern::Wildcard(span) => *span,
            ParsedPattern::Some(some_pattern) => some_pattern.span,
        }
    }

    pub fn get_constant(&self, id: ParsedConstantId) -> &ParsedConstant {
        &self.constants[id.0 as usize]
    }

    pub fn add_constant(&mut self, mut constant: ParsedConstant) -> ParsedConstantId {
        let id = ParsedConstantId(self.constants.len() as u32);
        constant.id = id;
        self.constants.push(constant);
        id
    }

    pub fn get_ability(&self, id: ParsedAbilityId) -> &ParsedAbility {
        &self.abilities[id.0 as usize]
    }

    pub fn add_ability(&mut self, mut ability: ParsedAbility) -> ParsedAbilityId {
        let id = ParsedAbilityId(self.abilities.len() as u32);
        ability.id = id;
        self.abilities.push(ability);
        id
    }

    pub fn get_ability_impl(&self, id: ParsedAbilityImplId) -> &ParsedAbilityImplementation {
        &self.ability_impls[id.0 as usize]
    }

    pub fn add_ability_impl(
        &mut self,
        mut ability_impl: ParsedAbilityImplementation,
    ) -> ParsedAbilityImplId {
        let id = ParsedAbilityImplId(self.ability_impls.len() as u32);
        ability_impl.id = id;
        self.ability_impls.push(ability_impl);
        id
    }

    pub fn get_type_defn(&self, id: ParsedTypeDefnId) -> &ParsedTypeDefn {
        &self.type_defns[id.0 as usize]
    }

    pub fn add_typedefn(&mut self, mut type_defn: ParsedTypeDefn) -> ParsedTypeDefnId {
        let id = ParsedTypeDefnId(self.type_defns.len() as u32);
        type_defn.id = id;
        self.type_defns.push(type_defn);
        id
    }

    pub fn get_root_namespace(&self) -> &ParsedNamespace {
        &self.namespaces[0]
    }

    pub fn get_expression_type_hint(
        &self,
        id: ParsedExpressionId,
    ) -> Option<ParsedTypeExpressionId> {
        self.expressions.get_type_hint(id)
    }

    pub fn get_type_expression_span(&self, type_expression_id: ParsedTypeExpressionId) -> SpanId {
        self.type_expressions.get(type_expression_id).get_span()
    }

    pub fn get_span_for_maybe_id(&self, parsed_id: Option<ParsedId>) -> SpanId {
        match parsed_id {
            Some(parsed_id) => self.get_span_for_id(parsed_id),
            None => SpanId::NONE,
        }
    }

    pub fn get_span_for_id(&self, parsed_id: ParsedId) -> SpanId {
        match parsed_id {
            ParsedId::Function(id) => self.get_function(id).span,
            ParsedId::Namespace(id) => self.get_namespace(id).span,
            ParsedId::Constant(id) => self.get_constant(id).span,
            ParsedId::Ability(id) => self.get_ability(id).span,
            ParsedId::AbilityImpl(id) => self.get_ability_impl(id).span,
            ParsedId::TypeDefn(id) => self.get_type_defn(id).span,
            ParsedId::Expression(id) => self.expressions.get_expression(id).get_span(),
            ParsedId::TypeExpression(id) => self.get_type_expression_span(id),
            ParsedId::Pattern(id) => self.get_pattern_span(id),
        }
    }
}

pub type ParseResult<A> = anyhow::Result<A, ParseError>;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub expected: String,
    pub token: Token,
    pub cause: Option<Box<ParseError>>,
}

impl ParseError {
    pub fn span(&self) -> SpanId {
        self.token.span
    }
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{:?}", self))
    }
}
impl std::error::Error for ParseError {}

pub fn print_error_location(spans: &Spans, sources: &Sources, span_id: SpanId) {
    let span = spans.get(span_id);
    let source = sources.source_by_span(span);
    let Some(line) = source.get_line_for_span(span) else {
        println!("Error: could not find line for span {:?}", span);
        return;
    };
    use colored::*;

    // If the span is longer than the line, just highlight the whole line
    let highlight_length =
        if span.len as usize > line.content.len() { line.content.len() } else { span.len as usize };
    let thingies = "^".repeat(highlight_length);
    let spaces = " ".repeat((span.start - line.start_char) as usize);
    let code = format!("  ->{}\n  ->{spaces}{thingies}", &line.content);
    println!(
        "  {} at {}/{}:{}\n\n{code}",
        "Error".red(),
        source.directory,
        source.filename,
        line.line_index + 1,
    );
}

#[derive(Debug, Clone)]
pub struct Line {
    pub start_char: u32,
    pub line_index: u32,
    pub content: String,
}

impl Line {
    pub fn end_char(&self) -> u32 {
        // TODO(unicode): Assuming ascii for now
        self.start_char + self.content.len() as u32
    }
}

#[derive(Debug, Clone)]
pub struct Source {
    pub file_id: FileId,
    pub directory: String,
    pub filename: String,
    pub content: String,
    /// This is an inefficient copy but we need the lines cached because utf8
    /// Eventually it can be references not copies
    pub lines: Vec<Line>,
}

impl Source {
    pub fn make(file_id: FileId, directory: String, filename: String, content: String) -> Source {
        let mut lines = Vec::new();
        let mut iter = content.chars().enumerate().peekable();
        let mut buf: String = String::new();
        // We compute lines ourselves because we need to know the start offset of each line
        // in chars
        while let Some((c_index, c)) = iter.next() {
            let mut push_line = || {
                let start: u32 = (c_index - buf.len()) as u32;
                lines.push(Line {
                    start_char: start,
                    line_index: lines.len() as u32,
                    content: buf.clone(),
                });
                buf.clear();
            };
            if c == '\n' {
                push_line();
            } else if c == '\r' && iter.peek().is_some_and(|(_, c)| *c == '\n') {
                // Skip over the \n
                iter.next();
                push_line();
            } else {
                buf.push(c);
            }
        }
        Source { file_id, directory, filename, content, lines }
    }

    pub fn get_content(&self, start: u32, len: u32) -> &str {
        &self.content[start as usize..(start + len) as usize]
    }

    pub fn get_span_content(&self, span: Span) -> &str {
        self.get_content(span.start, span.len)
    }

    pub fn get_line_for_span(&self, span: Span) -> Option<&Line> {
        self.lines.iter().find(|line| {
            let line_end = line.end_char() as u32;
            line.start_char <= span.start && line_end >= span.start
        })
    }
}

pub struct Parser<'toks, 'module> {
    tokens: TokenIter<'toks>,
    file_id: FileId,
    pub module: &'module mut ParsedModule,
}

impl<'toks, 'module> Parser<'toks, 'module> {
    pub fn make(
        tokens: &'toks [Token],
        source: Source,
        module: &'module mut ParsedModule,
    ) -> Parser<'toks, 'module> {
        let parser = Parser { tokens: TokenIter::make(tokens), file_id: source.file_id, module };
        parser.module.sources.insert(source);
        parser
    }

    fn source(&self) -> &Source {
        &self.module.sources.get_source(self.file_id)
    }

    fn expect<A>(what: &str, current: Token, value: ParseResult<Option<A>>) -> ParseResult<A> {
        match value {
            Ok(None) => Err(ParseError { expected: what.to_string(), token: current, cause: None }),
            Ok(Some(a)) => Ok(a),
            Err(e) => Err(e),
        }
    }

    fn extend_token_span(&mut self, tok1: Token, tok2: Token) -> SpanId {
        self.extend_span(tok1.span, tok2.span)
    }

    fn extend_span(&mut self, span1: SpanId, span2: SpanId) -> SpanId {
        self.module.spans.extend(span1, span2)
    }

    fn expect_pattern(&mut self) -> ParseResult<ParsedPatternId> {
        let first = self.peek();
        if let Some(literal_id) = self.parse_literal()? {
            let pattern = ParsedPattern::Literal(literal_id);
            let id = self.module.patterns.add_pattern(pattern);
            Ok(id)
        } else if first.kind == K::OpenBrace {
            // Struct
            let open_brace = self.tokens.next();
            let mut fields = Vec::new();
            while self.peek().kind != K::CloseBrace {
                let ident_token = self.expect_eat_token(K::Ident)?;
                let ident = self.intern_ident_token(ident_token);
                let maybe_colon = self.peek();
                let pattern_id = if maybe_colon.kind == K::Colon {
                    self.tokens.advance();
                    self.expect_pattern()?
                } else {
                    // Assume variable binding pattern with same name as field
                    let pattern = ParsedPattern::Variable(ident, ident_token.span);
                    self.module.patterns.add_pattern(pattern)
                };
                fields.push((ident, pattern_id));
                let next = self.peek();
                if next.kind == K::Comma {
                    self.tokens.advance();
                } else if next.kind != K::CloseBrace {
                    return Err(ParseError {
                        expected: "comma or close brace".to_string(),
                        token: next,
                        cause: None,
                    });
                }
            }
            let end = self.expect_eat_token(K::CloseBrace)?;
            let span = self.extend_token_span(open_brace, end);
            let pattern = ParsedStructPattern { fields, span };
            let pattern_id = self.module.patterns.add_pattern(ParsedPattern::Struct(pattern));
            Ok(pattern_id)
        } else if first.kind == K::Dot {
            let dot = self.tokens.next();
            let ident_token = self.expect_eat_token(K::Ident)?;
            let ident = self.intern_ident_token(ident_token);
            let (payload_pattern, span) = if self.peek().kind == K::OpenParen {
                self.tokens.advance();
                let payload_pattern_id = self.expect_pattern()?;
                let close_paren = self.expect_eat_token(K::CloseParen)?;
                (Some(payload_pattern_id), self.module.spans.extend(dot.span, close_paren.span))
            } else {
                (None, ident_token.span)
            };
            let pattern_id =
                self.module.patterns.add_pattern(ParsedPattern::Enum(ParsedEnumPattern {
                    variant_tag: ident,
                    payload_pattern,
                    span,
                }));
            Ok(pattern_id)
        } else if first.kind == K::Ident {
            // Variable
            let ident_token = self.expect_eat_token(K::Ident)?;
            let ident = self.intern_ident_token(ident_token);
            if self.module.identifiers.get_name(ident) == "_" {
                let pattern_id =
                    self.module.patterns.add_pattern(ParsedPattern::Wildcard(ident_token.span));
                Ok(pattern_id)
            } else if self.module.identifiers.get_name(ident) == "Some" {
                self.expect_eat_token(K::OpenParen)?;
                let inner_pattern = self.expect_pattern()?;
                let close = self.expect_eat_token(K::CloseParen)?;
                let span = self.extend_token_span(first, close);
                let pattern_id = self
                    .module
                    .patterns
                    .add_pattern(ParsedPattern::Some(ParsedSomePattern { inner_pattern, span }));
                Ok(pattern_id)
            } else {
                let pattern_id = self
                    .module
                    .patterns
                    .add_pattern(ParsedPattern::Variable(ident, ident_token.span));
                Ok(pattern_id)
            }
        } else {
            Err(Parser::error("Expected pattern expression", self.peek()))
        }
    }
}

impl<'toks, 'module> Parser<'toks, 'module> {
    pub fn print_error(&self, parse_error: &ParseError) {
        let span = parse_error.span();

        if let Some(cause) = &parse_error.cause {
            self.print_error(cause);
        }
        let got_str = if parse_error.token.kind == K::Ident {
            Parser::tok_chars(&self.module.spans, self.source(), parse_error.token).to_string()
        } else {
            parse_error.token.kind.to_string()
        };

        use colored::*;

        print_error_location(&self.module.spans, &self.module.sources, span);
        println!("\tExpected '{}', but got '{}'\n", parse_error.expected.blue(), got_str,);
    }

    #[inline]
    fn peek(&self) -> Token {
        self.tokens.peek()
    }

    #[inline]
    fn peek_two(&self) -> (Token, Token) {
        self.tokens.peek_two()
    }

    fn chars_at_span<'source>(
        spans: &Spans,
        source: &'source Source,
        span_id: SpanId,
    ) -> &'source str {
        let span = spans.get(span_id);
        source.get_span_content(span)
    }

    fn tok_chars<'source>(spans: &Spans, source: &'source Source, tok: Token) -> &'source str {
        let s = Parser::chars_at_span(spans, source, tok.span);
        trace!("{}.chars='{}'", tok.kind, s);
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
        let tok_chars = Parser::tok_chars(
            &self.module.spans,
            self.module.sources.get_source(self.file_id),
            token,
        );
        self.module.identifiers.intern(tok_chars)
    }

    pub fn add_expression(&mut self, expression: ParsedExpression) -> ParsedExpressionId {
        self.module.expressions.add_expression(expression)
    }

    pub fn get_expression(&self, id: ParsedExpressionId) -> &ParsedExpression {
        self.module.expressions.get_expression(id)
    }

    pub fn get_expression_span(&self, id: ParsedExpressionId) -> SpanId {
        self.module.expressions.get_expression(id).get_span()
    }

    fn parse_literal(&mut self) -> ParseResult<Option<ParsedExpressionId>> {
        let (first, second) = self.tokens.peek_two();
        trace!("parse_literal {} {}", first.kind, second.kind);
        return match (first.kind, second.kind) {
            (K::OpenParen, K::CloseParen) => {
                trace!("parse_literal unit");
                self.tokens.advance();
                self.tokens.advance();
                let span = self.extend_token_span(first, second);
                Ok(Some(self.add_expression(ParsedExpression::Literal(Literal::Unit(span)))))
            }
            (K::Char, _) => {
                trace!("parse_literal char");
                self.tokens.advance();
                let text = Parser::tok_chars(&self.module.spans, self.source(), first);
                assert!(text.starts_with('\''));
                assert!(text.ends_with('\''));
                let bytes = text.as_bytes();
                if bytes[1] == b'\\' {
                    assert_eq!(bytes.len(), 4);
                    let esc_char = bytes[2];
                    let literal = match esc_char {
                        b'n' => Ok(Literal::Char(b'\n', first.span)),
                        b'\'' => Ok(Literal::Char(b'\'', first.span)),
                        b't' => Ok(Literal::Char(b'\t', first.span)),
                        _ => Err(Parser::error(
                            format!(
                                "Valid escaped char following escape sequence: {}",
                                char::from(esc_char)
                            ),
                            first,
                        )),
                    }?;
                    Ok(Some(self.add_expression(ParsedExpression::Literal(literal))))
                } else {
                    assert_eq!(bytes.len(), 3);
                    let byte = bytes[1];
                    Ok(Some(self.add_expression(ParsedExpression::Literal(Literal::Char(
                        byte, first.span,
                    )))))
                }
            }
            (K::String, _) => {
                trace!("parse_literal string");
                self.tokens.advance();
                let text = Parser::tok_chars(&self.module.spans, self.source(), first);
                let literal = Literal::String(text.to_string(), first.span);
                Ok(Some(self.add_expression(ParsedExpression::Literal(literal))))
            }
            (K::Minus, K::Ident) if !second.is_whitespace_preceeded() => {
                let text = Parser::tok_chars(&self.module.spans, self.source(), second);
                if text.chars().next().unwrap().is_numeric() {
                    let mut s = "-".to_string();
                    s.push_str(text);
                    self.tokens.advance();
                    self.tokens.advance();
                    let span = self.extend_token_span(first, second);
                    let numeric = Literal::Numeric(s, span);
                    Ok(Some(self.add_expression(ParsedExpression::Literal(numeric))))
                } else {
                    Err(Parser::error("number following '-'", second))
                }
            }
            (K::Ident, _) => {
                let text = Parser::tok_chars(&self.module.spans, self.source(), first);
                if text == "true" {
                    self.tokens.advance();
                    Ok(Some(self.add_expression(ParsedExpression::Literal(Literal::Bool(
                        true, first.span,
                    )))))
                } else if text == "false" {
                    self.tokens.advance();
                    Ok(Some(self.add_expression(ParsedExpression::Literal(Literal::Bool(
                        false, first.span,
                    )))))
                } else if text == "None" {
                    self.tokens.advance();
                    Ok(Some(
                        self.add_expression(ParsedExpression::Literal(Literal::None(first.span))),
                    ))
                } else {
                    match text.chars().next() {
                        Some(c) if c.is_numeric() || c == '-' => {
                            let s = text.to_string();
                            self.tokens.advance();
                            Ok(Some(self.add_expression(ParsedExpression::Literal(
                                Literal::Numeric(s, first.span),
                            ))))
                        }
                        _ => Ok(None),
                    }
                }
            }
            _ => Ok(None),
        };
    }

    fn parse_struct_type_field(&mut self) -> ParseResult<Option<StructTypeField>> {
        let name_token = self.expect_eat_token(K::Ident)?;
        let ident_id = self.intern_ident_token(name_token);
        self.expect_eat_token(K::Colon)?;
        let typ_expr =
            Parser::expect("Type expression", self.peek(), self.parse_type_expression())?;
        Ok(Some(StructTypeField { name: ident_id, ty: typ_expr }))
    }

    fn expect_type_expression(&mut self) -> ParseResult<ParsedTypeExpressionId> {
        Parser::expect("type_expression", self.peek(), self.parse_type_expression())
    }

    fn parse_type_expression(&mut self) -> ParseResult<Option<ParsedTypeExpressionId>> {
        let Some(mut result) = self.parse_base_type_expression()? else {
            return Ok(None);
        };
        loop {
            let next = self.peek();
            if next.kind.is_postfix_type_operator() {
                if next.kind == K::Dot {
                    self.tokens.advance();
                    let ident_token = self.expect_eat_token(K::Ident)?;
                    let ident = self.intern_ident_token(ident_token);
                    let span = self.extend_span(
                        self.module.get_type_expression_span(result),
                        ident_token.span,
                    );
                    let new = ParsedTypeExpression::DotMemberAccess(ParsedDotMemberAccess {
                        base: result,
                        member_name: ident,
                        span,
                    });
                    let new_id = self.module.type_expressions.add(new);
                    result = new_id;
                } else if next.kind == K::QuestionMark {
                    // Optional Type
                    // For now, we say you can't nest options, at least syntactically, by not recursing / looping
                    self.tokens.advance();
                    result = self.module.type_expressions.add(ParsedTypeExpression::Optional(
                        ParsedOptional { base: result, span: next.span },
                    ));
                    break;
                } else if next.kind == K::Asterisk {
                    // Reference Type
                    // For now, we say you can't nest references, at least syntactically, by not recursing / looping
                    self.tokens.advance();
                    result = self.module.type_expressions.add(ParsedTypeExpression::Reference(
                        ParsedReference { base: result, span: next.span },
                    ));
                    break;
                } else {
                    panic!("unhandled postfix type operator {:?}", next.kind);
                }
            } else {
                break;
            }
        }
        Ok(Some(result))
    }

    fn parse_base_type_expression(&mut self) -> ParseResult<Option<ParsedTypeExpressionId>> {
        let (first, second) = self.peek_two();
        if first.kind == K::KeywordEnum {
            let enumm = self.expect_enum_type_expression()?;
            let type_expr_id = self.module.type_expressions.add(ParsedTypeExpression::Enum(enumm));
            Ok(Some(type_expr_id))
        } else if first.kind == K::Ident {
            let ident_chars = Parser::tok_chars(&self.module.spans, self.source(), first);
            if ident_chars == "unit" {
                self.tokens.advance();
                Ok(Some(self.module.type_expressions.add(ParsedTypeExpression::Unit(first.span))))
            } else if ident_chars == "string" {
                self.tokens.advance();
                Ok(Some(self.module.type_expressions.add(ParsedTypeExpression::String(first.span))))
            } else if ident_chars == "int" {
                self.tokens.advance();
                Ok(Some(self.module.type_expressions.add(ParsedTypeExpression::Int(first.span))))
            } else if ident_chars == "bool" {
                self.tokens.advance();
                Ok(Some(self.module.type_expressions.add(ParsedTypeExpression::Bool(first.span))))
            } else if ident_chars == "char" {
                self.tokens.advance();
                Ok(Some(self.module.type_expressions.add(ParsedTypeExpression::Char(first.span))))
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
                    let ident = self.intern_ident_token(first);
                    let span = self.extend_span(first.span, params_span);
                    Ok(Some(self.module.type_expressions.add(
                        ParsedTypeExpression::TypeApplication(TypeApplication {
                            base: ident,
                            params: type_parameters,
                            span,
                        }),
                    )))
                } else {
                    let ident = self.intern_ident_token(first);
                    Ok(Some(
                        self.module
                            .type_expressions
                            .add(ParsedTypeExpression::Name(ident, first.span)),
                    ))
                }
            }
        } else if first.kind == K::Dot && second.kind == K::Ident {
            // Tag Literal Type
            self.tokens.advance();
            let tag_name = self.expect_eat_token(K::Ident)?;
            let tag_name_ident = self.intern_ident_token(tag_name);
            let span = self.extend_token_span(first, tag_name);
            Ok(Some(
                self.module
                    .type_expressions
                    .add(ParsedTypeExpression::TagName(tag_name_ident, span)),
            ))
        } else if first.kind == K::OpenBrace {
            let open_brace = self.expect_eat_token(K::OpenBrace)?;
            let (fields, fields_span) =
                self.eat_delimited("Struct fields", K::Comma, K::CloseBrace, |p| {
                    let field_res = Parser::parse_struct_type_field(p);
                    Parser::expect("Struct Field", open_brace, field_res)
                })?;
            let span = self.extend_span(first.span, fields_span);
            let struc = StructType { fields, span };
            Ok(Some(self.module.type_expressions.add(ParsedTypeExpression::Struct(struc))))
        } else {
            Ok(None)
        }
    }

    fn expect_enum_type_expression(&mut self) -> ParseResult<ParsedEnumType> {
        let keyword = self.expect_eat_token(K::KeywordEnum)?;
        let mut variants = Vec::new();
        let mut first = true;
        loop {
            // Expect comma
            if !first {
                if self.peek().kind == K::Pipe {
                    self.tokens.advance();
                } else {
                    break;
                }
            }

            let tag = self.peek();
            if tag.kind != K::Ident {
                return Err(Parser::error("Identifier for enum variant", tag));
            }
            let tag_name = self.intern_ident_token(tag);
            self.tokens.advance();
            let maybe_payload_paren = self.peek();
            let payload_expression = if maybe_payload_paren.kind == K::OpenParen {
                self.tokens.advance();
                let payload_expr = self.expect_type_expression()?;
                let _close_paren = self.expect_eat_token(K::CloseParen)?;
                Some(payload_expr)
            } else {
                None
            };
            let span = match payload_expression {
                None => tag.span,
                Some(expr) => self.module.type_expressions.get(expr).get_span(),
            };
            variants.push(ParsedEnumVariant { tag_name, payload_expression, span });
            first = false;
        }
        let last_variant_span =
            variants.last().ok_or(Parser::error("At least one variant", keyword))?.span;
        let span = self.extend_span(keyword.span, last_variant_span);
        Ok(ParsedEnumType { variants, span })
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

    fn parse_struct(&mut self) -> ParseResult<Option<Struct>> {
        let Some(open_brace) = self.eat_token(K::OpenBrace) else {
            return Ok(None);
        };
        let (fields, fields_span) =
            self.eat_delimited("Struct values", K::Comma, K::CloseBrace, |parser| {
                let name = parser.expect_eat_token(K::Ident)?;
                parser.expect_eat_token(K::Colon)?;
                let expr = Parser::expect("expression", parser.peek(), parser.parse_expression())?;
                Ok(StructField { name: parser.intern_ident_token(name), expr })
            })?;
        // TODO: This pattern creates extra spans because we don't include the opening brace in eat_delimited
        let span = self.extend_span(open_brace.span, fields_span);
        Ok(Some(Struct { fields, span }))
    }

    fn parse_expression_with_postfix_ops(&mut self) -> ParseResult<Option<ParsedExpressionId>> {
        let Some(mut result) = self.parse_base_expression()? else { return Ok(None) };
        // Looping for postfix ops inspired by Jakt's parser
        let with_postfix: ParsedExpressionId = loop {
            let next = self.peek();
            if next.kind.is_postfix_operator() {
                // Optional uwrap `config!.url`
                if next.kind == K::Bang {
                    self.tokens.advance();
                    let span = self.extend_span(self.get_expression_span(result), next.span);
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
                        let span = self.extend_span(self.get_expression_span(result), args_span);
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
                        let span = self.extend_span(self.get_expression_span(result), next.span);
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
                    let span = self.extend_span(self.get_expression_span(result), close.span);
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
            self.module.expressions.add_type_hint(with_postfix, type_hint);
        }
        Ok(Some(with_postfix))
    }

    fn expect_block(&mut self) -> ParseResult<Block> {
        Parser::expect("block", self.peek(), self.parse_block())
    }

    fn expect_expression(&mut self) -> ParseResult<ParsedExpressionId> {
        Parser::expect("expression", self.peek(), self.parse_expression())
    }

    fn parse_expression(&mut self) -> ParseResult<Option<ParsedExpressionId>> {
        let Some(mut expr) = self.parse_expression_with_postfix_ops()? else {
            return Ok(None);
        };
        if self.peek().kind.is_binary_operator() {
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
                    let span = self.extend_expr_span(lhs, rhs);
                    let bin_op = self.add_expression(ParsedExpression::BinaryOp(BinaryOp {
                        op_kind,
                        lhs,
                        rhs,
                        span,
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
                let new_span = self.extend_expr_span(lhs, rhs);
                let bin_op = self.add_expression(ParsedExpression::BinaryOp(BinaryOp {
                    op_kind,
                    lhs,
                    rhs,
                    span: new_span,
                }));
                expr_stack.push(ExprStackMember::Expr(bin_op));
            }
            let with_correct_binops = expr_stack.pop().unwrap().expect_expr();
            expr = with_correct_binops;
        };
        if self.peek().kind == K::KeywordIs {
            self.tokens.advance();
            let pattern = self.expect_pattern()?;

            let original_span = self.get_expression_span(expr);
            let pattern_span = self.module.get_pattern_span(pattern);
            let span = self.extend_span(original_span, pattern_span);
            let is_expression_id = self.add_expression(ParsedExpression::Is(ParsedIsExpression {
                target_expression: expr,
                pattern,
                span,
            }));
            Ok(Some(is_expression_id))
        } else {
            Ok(Some(expr))
        }
    }

    fn extend_expr_span(&mut self, expr1: ParsedExpressionId, expr2: ParsedExpressionId) -> SpanId {
        self.extend_span(self.get_expression_span(expr1), self.get_expression_span(expr2))
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
    fn parse_base_expression(&mut self) -> ParseResult<Option<ParsedExpressionId>> {
        let (first, second, third) = self.tokens.peek_three();
        trace!("parse_base_expression {} {} {}", first.kind, second.kind, third.kind);
        if let Some(literal_id) = self.parse_literal()? {
            Ok(Some(literal_id))
        } else if first.kind == K::KeywordWhen {
            let when_keyword = self.tokens.next();
            let target_expression = self.expect_expression()?;

            self.expect_eat_token(K::OpenBrace)?;

            // Allow an opening pipe for symmetry
            if self.peek().kind == K::Pipe {
                self.tokens.advance();
            }
            let mut cases = Vec::new();
            while self.peek().kind != K::CloseBrace {
                let arm_pattern_id = self.expect_pattern()?;

                self.expect_eat_token(K::Minus)?;
                self.expect_eat_token(K::CloseAngle)?;

                let arm_expr_id = self.expect_expression()?;
                cases.push(ParsedMatchCase { pattern: arm_pattern_id, expression: arm_expr_id });
                let next = self.peek();
                if next.kind == K::Pipe {
                    self.tokens.advance();
                } else if next.kind != K::CloseBrace {
                    return Err(ParseError {
                        expected: "pipe or close brace".to_string(),
                        token: next,
                        cause: None,
                    });
                }
            }
            let close = self.expect_eat_token(K::CloseBrace)?;
            let span = self.extend_token_span(when_keyword, close);
            let match_expr = ParsedMatchExpression { target_expression, cases, span };
            Ok(Some(self.add_expression(ParsedExpression::Match(match_expr))))
        } else if first.kind == K::OpenParen {
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
            let span = self.extend_span(first.span, body_expr.span);
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
            let span = self.extend_span(first.span, self.get_expression_span(expr));
            Ok(Some(self.add_expression(ParsedExpression::UnaryOp(UnaryOp {
                expr,
                op_kind,
                span,
            }))))
        } else if first.kind == K::Dot && second.kind == K::Ident {
            // Tag Literal: .Red
            self.tokens.advance();
            self.tokens.advance();

            if Parser::tok_chars(&self.module.spans, self.source(), second)
                .chars()
                .next()
                .unwrap()
                .is_lowercase()
            {
                return Err(Parser::error("Uppercase tag name", second));
            }
            let tag_name = self.intern_ident_token(second);

            if third.kind == K::OpenParen {
                // Enum Constructor
                self.tokens.advance();
                let payload = self.expect_expression()?;
                let close_paren = self.expect_eat_token(K::CloseParen)?;
                let span = self.extend_token_span(first, close_paren);
                Ok(Some(self.add_expression(ParsedExpression::EnumConstructor(
                    ParsedEnumConstructor { tag: tag_name, payload, span },
                ))))
            } else {
                // Tag Literal
                let span = self.extend_token_span(first, second);
                Ok(Some(
                    self.add_expression(ParsedExpression::Tag(TagExpr { tag: tag_name, span })),
                ))
            }
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
                let span = self.extend_span(first.span, args_span);
                Ok(Some(self.add_expression(ParsedExpression::FnCall(FnCall {
                    name,
                    type_args,
                    args,
                    namespaces,
                    span,
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
            // The syntax {} means empty struct, not empty block
            // If you want a void or empty block, the required syntax is { () }
            trace!("parse_expr {:?} {:?} {:?}", first, second, third);
            if second.kind == K::CloseBrace {
                let span = self.extend_token_span(first, second);
                Ok(Some(
                    self.add_expression(ParsedExpression::Struct(Struct { fields: vec![], span })),
                ))
            } else if second.kind == K::Ident && third.kind == K::Colon {
                let struc = Parser::expect("struct", first, self.parse_struct())?;
                Ok(Some(self.add_expression(ParsedExpression::Struct(struc))))
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
            let (elements, elements_span) = self.eat_delimited(
                "Array elements",
                TokenKind::Comma,
                TokenKind::CloseBracket,
                |p| Parser::expect("expression", start, p.parse_expression()),
            )?;
            let span = self.extend_span(start.span, elements_span);
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
            self.extend_span(eaten_keyword.span, self.get_expression_span(initializer_expression));
        Ok(Some(ValDef {
            name: self.intern_ident_token(name_token),
            type_expr: typ,
            value: initializer_expression,
            is_mutable: mutable,
            span,
        }))
    }

    fn parse_const(&mut self) -> ParseResult<Option<ParsedConstantId>> {
        trace!("parse_const");
        let Some(keyword_val_token) = self.eat_token(K::KeywordVal) else {
            return Ok(None);
        };
        let name_token = self.expect_eat_token(K::Ident)?;
        let _colon = self.expect_eat_token(K::Colon);
        let typ = Parser::expect("type_expression", self.peek(), self.parse_type_expression())?;
        self.expect_eat_token(K::Equals)?;
        let value_expr = Parser::expect("expression", self.peek(), self.parse_expression())?;
        let span = self.extend_span(keyword_val_token.span, self.get_expression_span(value_expr));
        let name = self.intern_ident_token(name_token);
        let constant_id = self.module.add_constant(ParsedConstant {
            name,
            ty: typ,
            value_expr,
            span,
            id: ParsedConstantId(0),
        });
        Ok(Some(constant_id))
    }

    fn parse_assignment(&mut self, lhs: ParsedExpressionId) -> ParseResult<Assignment> {
        let _valid_lhs = match &*self.get_expression(lhs) {
            ParsedExpression::FieldAccess(_) => true,
            ParsedExpression::Variable(_) => true,
            ParsedExpression::IndexOperation(_) => true,
            _ => false,
        };
        self.expect_eat_token(K::Equals)?;
        let rhs = self.expect_expression()?;
        let span = self.extend_expr_span(lhs, rhs);
        Ok(Assignment { lhs, rhs, span })
    }

    fn eat_fn_arg_def(&mut self) -> ParseResult<FnArgDef> {
        trace!("eat_fn_arg_def");
        let name_token = self.expect_eat_token(K::Ident)?;
        self.expect_eat_token(K::Colon)?;
        let typ = Parser::expect("type_expression", self.peek(), self.parse_type_expression())?;
        let span =
            self.extend_span(name_token.span, self.module.type_expressions.get(typ).get_span());
        Ok(FnArgDef { name: self.intern_ident_token(name_token), ty: typ, span })
    }

    fn eat_fndef_args(&mut self) -> ParseResult<(Vec<FnArgDef>, SpanId)> {
        self.eat_delimited("Function arguments", K::Comma, K::CloseParen, Parser::eat_fn_arg_def)
    }

    fn eat_delimited<T, F>(
        &mut self,
        name: &str,
        delim: TokenKind,
        terminator: TokenKind,
        parse: F,
    ) -> ParseResult<(Vec<T>, SpanId)>
    where
        F: Fn(&mut Parser<'toks, 'module>) -> ParseResult<T>,
    {
        trace!("eat_delimited delim='{}' terminator='{}'", delim, terminator);
        // TODO @Allocation Use smallvec
        let mut v = Vec::with_capacity(8);

        // FIXME(spans) this should probably be passed in since it doesn't cover the opening delimiter
        let start_span = self.peek().span;

        loop {
            if let Some(terminator) = self.eat_token(terminator) {
                trace!("eat_delimited found terminator after {} results.", v.len());
                let span = self.module.spans.extend(start_span, terminator.span);
                break Ok((v, span));
            }
            match parse(self) {
                Ok(parsed) => {
                    v.push(parsed);
                    trace!("eat_delimited got result {}", v.len());
                    if let Some(terminator) = self.eat_token(terminator) {
                        trace!("eat_delimited found terminator after {} results.", v.len());
                        let span = self.module.spans.extend(start_span, terminator.span);
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
            .map(|a| self.get_expression_span(*a))
            .unwrap_or(self.get_expression_span(consequent_expr));
        let span = self.extend_span(if_keyword.span, end_span);
        let if_expr = IfExpr { cond: condition_expr, cons: consequent_expr, alt, span };
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
        let span = self.extend_span(while_token.span, block.span);
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
        let parse_statement =
            |p: &mut Parser| Parser::expect("statement", p.peek(), Parser::parse_statement(p));
        let (block_statements, statements_span) =
            self.eat_delimited("Block statements", K::Semicolon, K::CloseBrace, parse_statement)?;
        let span = self.extend_span(block_start.span, statements_span);
        Ok(Some(Block { stmts: block_statements, span }))
    }

    fn expect_type_param(&mut self) -> ParseResult<TypeParamDef> {
        let s = self.expect_eat_token(K::Ident)?;
        let ident_id = self.intern_ident_token(s);
        Ok(TypeParamDef { ident: ident_id, span: s.span })
    }

    fn parse_function(&mut self) -> ParseResult<Option<ParsedFunctionId>> {
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
        let end_span = block.as_ref().map(|b| b.span).unwrap_or(args_span);
        let span = self.extend_span(fn_keyword.span, end_span);
        let function_id = self.module.add_function(ParsedFunction {
            name: func_name_id,
            type_args: type_arguments,
            args,
            ret_type,
            block,
            span,
            linkage,
            id: ParsedFunctionId(0),
        });
        Ok(Some(function_id))
    }

    fn parse_ability_defn(&mut self) -> ParseResult<Option<ParsedAbilityId>> {
        let keyword_ability = self.eat_token(K::KeywordAbility);
        let Some(keyword_ability) = keyword_ability else {
            return Ok(None);
        };
        let name_token = self.expect_eat_token(K::Ident)?;
        let name_identifier = self.intern_ident_token(name_token);
        self.expect_eat_token(K::OpenBrace)?;
        let mut functions = Vec::new();
        while let Some(parsed_function) = self.parse_function()? {
            functions.push(parsed_function);
        }
        let close_token = self.expect_eat_token(K::CloseBrace)?;
        let span = self.extend_token_span(keyword_ability, close_token);
        let ability_id = self.module.add_ability(ParsedAbility {
            name: name_identifier,
            functions,
            span,
            id: ParsedAbilityId(0),
        });
        Ok(Some(ability_id))
    }

    fn parse_ability_impl(&mut self) -> ParseResult<Option<ParsedAbilityImplId>> {
        let keyword_impl = self.eat_token(K::KeywordImpl);
        let Some(keyword_impl) = keyword_impl else {
            return Ok(None);
        };
        let ability_name = self.expect_eat_token(K::Ident)?;
        self.expect_eat_token(K::KeywordFor)?;
        let target_type = self.expect_type_expression()?;

        // Functions or auto
        let next = self.peek();
        let mut functions = Vec::new();
        let final_token = if next.kind == K::KeywordAuto {
            self.tokens.advance();
            next
        } else {
            self.expect_eat_token(K::OpenBrace)?;

            while let Some(parsed_function) = self.parse_function()? {
                functions.push(parsed_function);
            }
            let close_brace = self.expect_eat_token(K::CloseBrace)?;
            close_brace
        };
        let auto = final_token.kind == K::KeywordAuto;

        let ability_name_ident = self.intern_ident_token(ability_name);
        let span = self.extend_token_span(keyword_impl, final_token);
        let ability_impl_id = self.module.add_ability_impl(ParsedAbilityImplementation {
            ability_name: ability_name_ident,
            target_type,
            functions,
            id: ParsedAbilityImplId(0),
            auto,
            span,
        });
        Ok(Some(ability_impl_id))
    }

    fn parse_typedef(&mut self) -> ParseResult<Option<ParsedTypeDefnId>> {
        let keyword_type = self.eat_token(K::KeywordType);
        let Some(keyword_type) = keyword_type else {
            return Ok(None);
        };
        let name = self.expect_eat_token(K::Ident)?;
        let equals = self.expect_eat_token(K::Equals)?;
        let type_expr = Parser::expect("Type expression", equals, self.parse_type_expression())?;
        let span =
            self.extend_span(keyword_type.span, self.module.get_type_expression_span(type_expr));
        let name = self.intern_ident_token(name);
        let type_defn_id = self.module.add_typedefn(ParsedTypeDefn {
            name,
            value_expr: type_expr,
            span,
            id: ParsedTypeDefnId(0),
        });
        Ok(Some(type_defn_id))
    }

    fn parse_namespace(&mut self) -> ParseResult<Option<ParsedNamespaceId>> {
        let keyword = self.peek();
        if keyword.kind != K::KeywordNamespace {
            return Ok(None);
        };
        self.tokens.advance();
        let ident = self.expect_eat_token(K::Ident)?;
        self.expect_eat_token(K::OpenBrace)?;
        let mut definitions = Vec::new();
        while let Some(def) = self.parse_definition()? {
            definitions.push(def);
        }
        let close = self.expect_eat_token(K::CloseBrace)?;
        let name = self.intern_ident_token(ident);
        let span = self.extend_token_span(keyword, close);
        let namespace_id = self.module.add_namespace(ParsedNamespace {
            name,
            definitions,
            id: ParsedNamespaceId(0),
            span,
        });
        Ok(Some(namespace_id))
    }

    fn parse_definition(&mut self) -> ParseResult<Option<ParsedId>> {
        if let Some(ns) = self.parse_namespace()? {
            Ok(Some(ParsedId::Namespace(ns)))
        } else if let Some(constant_id) = self.parse_const()? {
            self.expect_eat_token(K::Semicolon)?;
            Ok(Some(ParsedId::Constant(constant_id)))
        } else if let Some(function_id) = self.parse_function()? {
            Ok(Some(ParsedId::Function(function_id)))
        } else if let Some(type_defn_id) = self.parse_typedef()? {
            Ok(Some(ParsedId::TypeDefn(type_defn_id)))
        } else if let Some(ability_id) = self.parse_ability_defn()? {
            Ok(Some(ParsedId::Ability(ability_id)))
        } else if let Some(ability_impl_id) = self.parse_ability_impl()? {
            Ok(Some(ParsedId::AbilityImpl(ability_impl_id)))
        } else {
            Ok(None)
        }
    }

    pub fn parse_module(&mut self) -> ParseResult<()> {
        let root_namespace_id = if self.module.namespaces.is_empty() {
            let name = self.module.identifiers.intern("_root");
            self.module.add_namespace(ParsedNamespace {
                name,
                definitions: Vec::new(),
                id: ParsedNamespaceId(0),
                span: self.peek().span,
            })
        } else {
            self.module.get_root_namespace().id
        };

        let mut new_definitions: Vec<ParsedId> = vec![];
        loop {
            match self.parse_definition() {
                Ok(Some(def)) => new_definitions.push(def),
                Err(err) => {
                    self.module.errors.push(err);
                    // For now, break on first parse error
                    break;
                }
                Ok(None) => break,
            }
        }
        while let Some(def) = self.parse_definition()? {
            new_definitions.push(def)
        }
        if self.tokens.peek().kind != K::Eof {
            let err = Parser::error("End or definition", self.tokens.peek());
            self.module.errors.push(err.clone());
            return Err(err);
        }

        self.module.get_namespace_mut(root_namespace_id).definitions.extend(new_definitions);

        Ok(())
    }
}

// Display
impl ParsedModule {
    pub fn expression_to_string(&self, expr: ParsedExpressionId) -> String {
        let mut buffer = String::new();
        self.display_expression_id(expr, &mut buffer).unwrap();
        buffer
    }

    pub fn display_expression_id(
        &self,
        expr: ParsedExpressionId,
        f: &mut impl Write,
    ) -> std::fmt::Result {
        match self.expressions.get_expression(expr) {
            ParsedExpression::BinaryOp(op) => {
                f.write_str("(")?;
                self.display_expression_id(op.lhs, f)?;
                f.write_fmt(format_args!(" {} ", op.op_kind))?;
                self.display_expression_id(op.rhs, f)?;
                f.write_str(")")
            }
            ParsedExpression::UnaryOp(op) => {
                f.write_fmt(format_args!("{}", op.op_kind))?;
                self.display_expression_id(op.expr, f)
            }
            ParsedExpression::Literal(lit) => f.write_fmt(format_args!("{}", lit)),
            ParsedExpression::FnCall(call) => f.write_fmt(format_args!("{:?}", call)),
            ParsedExpression::Variable(var) => f.write_fmt(format_args!("{}", var)),
            ParsedExpression::FieldAccess(acc) => f.write_fmt(format_args!("{:?}", acc)),
            ParsedExpression::MethodCall(call) => f.write_fmt(format_args!("{:?}", call)),
            ParsedExpression::Block(block) => f.write_fmt(format_args!("{:?}", block)),
            ParsedExpression::If(if_expr) => f.write_fmt(format_args!("{:?}", if_expr)),
            ParsedExpression::Struct(struc) => f.write_fmt(format_args!("{:?}", struc)),
            ParsedExpression::IndexOperation(op) => f.write_fmt(format_args!("{:?}", op)),
            ParsedExpression::Array(array_expr) => f.write_fmt(format_args!("{:?}", array_expr)),
            ParsedExpression::OptionalGet(optional_get) => {
                f.write_fmt(format_args!("{:?}", optional_get))
            }
            ParsedExpression::For(for_expr) => f.write_fmt(format_args!("{:?}", for_expr)),
            ParsedExpression::Tag(tag_expr) => {
                f.write_char('.')?;
                f.write_str(self.identifiers.get_name(tag_expr.tag))
            }
            ParsedExpression::EnumConstructor(e) => {
                f.write_char('.')?;
                f.write_str(self.identifiers.get_name(e.tag))?;
                f.write_str("(")?;
                self.display_expression_id(e.payload, f)?;
                f.write_str(")")
            }
            ParsedExpression::Is(is_expr) => {
                self.display_expression_id(is_expr.target_expression, f)?;
                f.write_str(" is ")?;
                self.display_pattern_expression_id(is_expr.pattern, f)
            }
            ParsedExpression::Match(match_expr) => {
                f.write_str("when ")?;
                self.display_expression_id(match_expr.target_expression, f)?;
                f.write_str(" is {")?;
                for ParsedMatchCase { pattern, expression } in match_expr.cases.iter() {
                    f.write_str(" | ")?;
                    self.display_pattern_expression_id(*pattern, f)?;
                    f.write_str(" => ")?;
                    self.display_expression_id(*expression, f)?;
                }
                f.write_str(" }")
            }
        }
    }

    pub fn display_pattern_expression_id(
        &self,
        _pattern_expr_id: ParsedPatternId,
        f: &mut impl Write,
    ) -> std::fmt::Result {
        f.write_str("<pattern expr todo>")
    }

    pub fn type_expression_to_string(&self, type_expr_id: ParsedTypeExpressionId) -> String {
        let mut buffer = String::new();
        self.display_type_expression_id(type_expr_id, &mut buffer).unwrap();
        buffer
    }

    pub fn display_type_expression_id(
        &self,
        ty_expr_id: ParsedTypeExpressionId,
        f: &mut impl Write,
    ) -> std::fmt::Result {
        match self.type_expressions.get(ty_expr_id) {
            ParsedTypeExpression::Unit(_) => f.write_str("unit"),
            ParsedTypeExpression::Char(_) => f.write_str("char"),
            ParsedTypeExpression::Int(_) => f.write_str("int"),
            ParsedTypeExpression::Bool(_) => f.write_str("bool"),
            ParsedTypeExpression::String(_) => f.write_str("string"),
            ParsedTypeExpression::Struct(struct_type) => {
                f.write_str("{ ")?;
                for field in struct_type.fields.iter() {
                    f.write_str(self.identifiers.get_name(field.name))?;
                    f.write_str(": ")?;
                    self.display_type_expression_id(ty_expr_id, f)?;
                    f.write_str(", ")?;
                }
                f.write_str(" }")
            }
            ParsedTypeExpression::Name(ident, _) => f.write_str(self.identifiers.get_name(*ident)),
            ParsedTypeExpression::TagName(ident, _) => {
                f.write_str(".")?;
                f.write_str(self.identifiers.get_name(*ident))
            }
            ParsedTypeExpression::TypeApplication(tapp) => {
                f.write_str(self.identifiers.get_name(tapp.base))?;
                f.write_str("<")?;
                for tparam in tapp.params.iter() {
                    self.display_type_expression_id(*tparam, f)?;
                    f.write_str(", ")?;
                }
                f.write_str(">")
            }
            ParsedTypeExpression::Optional(opt) => {
                self.display_type_expression_id(opt.base, f)?;
                f.write_str("?")
            }
            ParsedTypeExpression::Reference(refer) => {
                self.display_type_expression_id(refer.base, f)?;
                f.write_str("*")
            }
            ParsedTypeExpression::Enum(e) => {
                f.write_str("enum ")?;
                for variant in &e.variants {
                    f.write_str(self.identifiers.get_name(variant.tag_name))?;
                    if let Some(payload) = &variant.payload_expression {
                        f.write_str("(")?;
                        self.display_type_expression_id(*payload, f)?;
                        f.write_str(")")?;
                    }
                }
                Ok(())
            }
            ParsedTypeExpression::DotMemberAccess(acc) => {
                self.display_type_expression_id(acc.base, f)?;
                f.write_char('.')?;
                f.write_str(self.identifiers.get_name(acc.member_name))
            }
        }
    }
}

pub fn lex_text(spans: &mut Spans, text: &str, file_id: FileId) -> ParseResult<Vec<Token>> {
    let mut lexer = Lexer::make(text, spans, file_id);
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
pub fn test_parse_module(source: Source) -> ParseResult<ParsedModule> {
    let module_name = source.filename.split('.').next().unwrap().to_string();
    let mut module = ParsedModule::make(module_name);

    let token_vec = lex_text(&mut module.spans, &source.content, source.file_id)?;
    let mut parser = Parser::make(&token_vec, source, &mut module);

    let result = parser.parse_module();
    if let Err(e) = result {
        parser.print_error(&e);
        Err(e)
    } else {
        Ok(module)
    }
}
