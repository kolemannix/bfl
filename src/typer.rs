#![allow(clippy::match_like_matches_macro)]

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter, Write};
use std::rc::Rc;

use anyhow::bail;
use colored::Colorize;
use either::Either;
use log::{debug, error, trace, Level};

use crate::lex::{Span, TokenKind};
use crate::parse::{
    self, ExpressionValue, FnCallArg, ForExpr, ForExprType, IfExpr, IndexOperation, ParsedNamespace,
};
use crate::parse::{
    AstId, Block, BlockStmt, Definition, Expression, FnCall, FnDef, IdentifierId, Literal,
    ParsedModule,
};

pub type ScopeId = u32;
pub type FunctionId = u32;
pub type VariableId = u32;
pub type TypeId = u32;
pub type NamespaceId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    Standard,
    External,
    Intrinsic,
}

#[derive(Debug, Clone)]
pub struct RecordTypeField {
    pub name: IdentifierId,
    pub type_id: TypeId,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct RecordType {
    pub fields: Vec<RecordTypeField>,
    pub name_if_named: Option<IdentifierId>,
    pub span: Span,
}

impl RecordType {
    pub fn find_field(&self, field_name: IdentifierId) -> Option<(usize, &RecordTypeField)> {
        self.fields.iter().enumerate().find(|(_, field)| field.name == field_name)
    }
}

pub const UNIT_TYPE_ID: TypeId = 0;
pub const CHAR_TYPE_ID: TypeId = 1;
pub const INT_TYPE_ID: TypeId = 2;
pub const BOOL_TYPE_ID: TypeId = 3;
pub const STRING_TYPE_ID: TypeId = 4;

#[derive(Debug, Clone)]
pub struct TypeExpression {
    pub type_id: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ArrayType {
    pub element_type: TypeId,
    // todo: Lose the span!
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeVariable {
    identifier_id: IdentifierId,
    /// This is where trait bounds would go
    _constraints: Option<Vec<()>>,
}

#[derive(Debug, Clone)]
pub struct OptionalType {
    pub inner_type: TypeId,
}

#[derive(Debug, Clone)]
pub struct ReferenceType {
    pub inner_type: TypeId,
}

#[derive(Debug, Clone)]
pub enum Type {
    Unit,
    Char,
    Int,
    Bool,
    String,
    Record(RecordType),
    Array(ArrayType),
    Optional(OptionalType),
    Reference(ReferenceType),
    TypeVariable(TypeVariable),
}

impl Type {
    pub fn as_reference(&self) -> Option<&ReferenceType> {
        match self {
            Type::Reference(r) => Some(r),
            _ => None,
        }
    }
    pub fn expect_reference(&self) -> &ReferenceType {
        match self {
            Type::Reference(r) => r,
            _ => panic!("expect_reference called on: {:?}", self),
        }
    }
    pub fn as_optional(&self) -> Option<&OptionalType> {
        match self {
            Type::Optional(opt) => Some(opt),
            _ => None,
        }
    }
    pub fn expect_optional(&self) -> &OptionalType {
        match self {
            Type::Optional(opt) => opt,
            _ => panic!("expect_optional called on: {:?}", self),
        }
    }

    pub fn expect_array(&self) -> &ArrayType {
        match self {
            Type::Array(array) => array,
            _ => panic!("expect_array called on: {:?}", self),
        }
    }

    pub fn as_array(&self) -> Option<&ArrayType> {
        match self {
            Type::Array(array) => Some(array),
            _ => None,
        }
    }

    pub fn as_record(&self) -> Option<&RecordType> {
        match self {
            Type::Record(record) => Some(record),
            _ => None,
        }
    }

    pub fn expect_record(&self) -> &RecordType {
        match self {
            Type::Record(record) => record,
            _ => panic!("expect_record called on: {:?}", self),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypedBlock {
    // If this block is just an expression, the type of the expression
    pub expr_type: TypeId,
    pub scope_id: ScopeId,
    pub statements: Vec<TypedStmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FnArgDefn {
    pub name: IdentifierId,
    pub variable_id: VariableId,
    pub position: u32,
    pub type_id: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct SpecializationRecord {
    pub specialized_type_params: Vec<TypeId>,
    pub specialized_function_id: FunctionId,
    pub specialized_params: Vec<FnArgDefn>,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: IdentifierId,
    pub scope: ScopeId,
    pub ret_type: TypeId,
    pub params: Vec<FnArgDefn>,
    pub type_params: Option<Vec<TypeParam>>,
    pub block: Option<TypedBlock>,
    pub intrinsic_type: Option<IntrinsicFunctionType>,
    pub linkage: Linkage,
    pub specializations: Vec<SpecializationRecord>,
    pub ast_id: AstId,
    pub span: Span,
}

impl Function {
    pub fn is_generic(&self) -> bool {
        match &self.type_params {
            None => false,
            Some(vec) => !vec.is_empty(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypeParam {
    pub ident: IdentifierId,
    pub type_id: TypeId,
}

#[derive(Debug, Clone)]
pub struct VariableExpr {
    pub variable_id: VariableId,
    pub type_id: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOpKind {
    Add,
    Subtract,
    Multiply,
    Divide,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Equals,
    NotEquals,
}

impl Display for BinaryOpKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryOpKind::Add => f.write_char('+'),
            BinaryOpKind::Subtract => f.write_char('-'),
            BinaryOpKind::Multiply => f.write_char('*'),
            BinaryOpKind::Divide => f.write_char('/'),
            BinaryOpKind::Less => f.write_char('<'),
            BinaryOpKind::Greater => f.write_char('>'),
            BinaryOpKind::LessEqual => f.write_str("<="),
            BinaryOpKind::GreaterEqual => f.write_str(">="),
            BinaryOpKind::And => f.write_str("and"),
            BinaryOpKind::Or => f.write_str("or"),
            BinaryOpKind::Equals => f.write_str("=="),
            BinaryOpKind::NotEquals => f.write_str("!="),
        }
    }
}

impl BinaryOpKind {
    pub fn precedence(&self) -> usize {
        use BinaryOpKind as B;
        match self {
            B::Multiply | B::Divide => 100,
            B::Add | B::Subtract => 90,
            B::Less | B::LessEqual | B::Greater | B::GreaterEqual | B::Equals | B::NotEquals => 80,
            B::And => 70,
            B::Or => 66,
        }
    }

    pub fn from_tokenkind(kind: TokenKind) -> Option<BinaryOpKind> {
        match kind {
            TokenKind::Plus => Some(BinaryOpKind::Add),
            TokenKind::Minus => Some(BinaryOpKind::Subtract),
            TokenKind::Asterisk => Some(BinaryOpKind::Multiply),
            TokenKind::Slash => Some(BinaryOpKind::Divide),
            TokenKind::OpenAngle => Some(BinaryOpKind::Less),
            TokenKind::CloseAngle => Some(BinaryOpKind::Greater),
            TokenKind::KeywordAnd => Some(BinaryOpKind::And),
            TokenKind::KeywordOr => Some(BinaryOpKind::Or),
            TokenKind::EqualsEquals => Some(BinaryOpKind::Equals),
            TokenKind::BangEquals => Some(BinaryOpKind::NotEquals),
            _ => None,
        }
    }

    pub fn is_integer_op(&self) -> bool {
        use BinaryOpKind as B;
        match self {
            B::Add | B::Subtract => true,
            B::Multiply | B::Divide => true,
            B::Less | B::Greater | B::LessEqual | B::GreaterEqual => true,
            B::Or | B::And => true,
            B::Equals | B::NotEquals => true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BinaryOp {
    pub kind: BinaryOpKind,
    pub ty: TypeId,
    pub lhs: Box<TypedExpr>,
    pub rhs: Box<TypedExpr>,
    pub span: Span,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum UnaryOpKind {
    BooleanNegation,
    Reference,
    Dereference,
}

impl Display for UnaryOpKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UnaryOpKind::BooleanNegation => f.write_char('!'),
            UnaryOpKind::Reference => f.write_char('&'),
            UnaryOpKind::Dereference => f.write_char('*'),
        }
    }
}

impl UnaryOpKind {
    pub fn from_tokenkind(kind: TokenKind) -> Option<UnaryOpKind> {
        match kind {
            TokenKind::Bang => Some(UnaryOpKind::BooleanNegation),
            TokenKind::Ampersand => Some(UnaryOpKind::Reference),
            TokenKind::Asterisk => Some(UnaryOpKind::Dereference),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnaryOp {
    pub kind: UnaryOpKind,
    pub type_id: TypeId,
    pub expr: Box<TypedExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub callee_function_id: FunctionId,
    pub args: Vec<TypedExpr>,
    pub ret_type: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RecordField {
    pub name: IdentifierId,
    pub expr: TypedExpr,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub fields: Vec<RecordField>,
    pub type_id: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ArrayLiteral {
    pub elements: Vec<TypedExpr>,
    pub type_id: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypedIf {
    pub condition: TypedExpr,
    pub consequent: TypedBlock,
    pub alternate: TypedBlock,
    pub ty: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldAccess {
    pub base: Box<TypedExpr>,
    pub target_field: IdentifierId,
    pub target_field_index: u32,
    pub ty: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IndexOp {
    pub base_expr: Box<TypedExpr>,
    pub index_expr: Box<TypedExpr>,
    pub result_type: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct OptionalSome {
    pub inner_expr: Box<TypedExpr>,
    pub type_id: TypeId,
}

#[derive(Debug, Clone)]
pub struct OptionalGet {
    pub inner_expr: Box<TypedExpr>,
    pub result_type_id: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypedForExpr {
    pub iterable_expr: Box<TypedExpr>,
    pub binding: IdentifierId,
    pub body_block: Box<TypedBlock>,
    pub result_type_id: TypeId,
    pub for_expr_type: ForExprType,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypedExpr {
    Unit(Span),
    Char(u8, Span),
    Bool(bool, Span),
    Int(i64, Span),
    Str(String, Span),
    None(TypeId, Span),
    Record(Record),
    Array(ArrayLiteral),
    Variable(VariableExpr),
    RecordFieldAccess(FieldAccess),
    BinaryOp(BinaryOp),
    UnaryOp(UnaryOp),
    Block(TypedBlock),
    FunctionCall(Call),
    If(Box<TypedIf>),
    ArrayIndex(IndexOp),
    StringIndex(IndexOp),
    // TODO: Can some of these just be unary ops
    OptionalSome(OptionalSome),
    OptionalHasValue(Box<TypedExpr>),
    OptionalGet(OptionalGet),
    For(TypedForExpr),
}

// pub enum BuiltinType {
//     Unit,
//     Char,
//     Int,
//     Bool,
//     String,
// }

// impl BuiltinType {
//     pub fn id() -> TypeId {
//         match self {
//             BuiltinType::Unit => UNIT_TYPE_ID,
//             BuiltinType::Char => CHAR_TYPE_ID,
//             BuiltinType::Int => INT_TYPE_ID,
//             BuiltinType::Bool => BOOL_TYPE_ID,
//             BuiltinType::String => STRING_TYPE_ID,
//         }
//     }
// }

impl TypedExpr {
    pub fn unit_literal(span: Span) -> TypedExpr {
        TypedExpr::Unit(span)
    }

    #[inline]
    pub fn get_type(&self) -> TypeId {
        match self {
            TypedExpr::None(type_id, _) => *type_id,
            TypedExpr::Unit(_) => UNIT_TYPE_ID,
            TypedExpr::Char(_, _) => CHAR_TYPE_ID,
            TypedExpr::Str(_, _) => STRING_TYPE_ID,
            TypedExpr::Int(_, _) => INT_TYPE_ID,
            TypedExpr::Bool(_, _) => BOOL_TYPE_ID,
            TypedExpr::Record(record) => record.type_id,
            TypedExpr::Array(arr) => arr.type_id,
            TypedExpr::Variable(var) => var.type_id,
            TypedExpr::RecordFieldAccess(field_access) => field_access.ty,
            TypedExpr::BinaryOp(binary_op) => binary_op.ty,
            TypedExpr::UnaryOp(unary_op) => unary_op.type_id,
            TypedExpr::Block(b) => b.expr_type,
            TypedExpr::FunctionCall(call) => call.ret_type,
            TypedExpr::If(ir_if) => ir_if.ty,
            TypedExpr::ArrayIndex(op) => op.result_type,
            TypedExpr::StringIndex(op) => op.result_type,
            TypedExpr::OptionalSome(opt) => opt.type_id,
            TypedExpr::OptionalHasValue(_opt) => BOOL_TYPE_ID,
            TypedExpr::OptionalGet(opt_get) => opt_get.result_type_id,
            TypedExpr::For(for_expr) => for_expr.result_type_id,
        }
    }
    #[inline]
    pub fn get_span(&self) -> Span {
        match self {
            TypedExpr::Unit(span) => *span,
            TypedExpr::Char(_, span) => *span,
            TypedExpr::Bool(_, span) => *span,
            TypedExpr::Int(_, span) => *span,
            TypedExpr::Str(_, span) => *span,
            TypedExpr::None(_, span) => *span,
            TypedExpr::Record(record) => record.span,
            TypedExpr::Array(array) => array.span,
            TypedExpr::Variable(var) => var.span,
            TypedExpr::RecordFieldAccess(field_access) => field_access.span,
            TypedExpr::BinaryOp(binary_op) => binary_op.span,
            TypedExpr::UnaryOp(unary_op) => unary_op.span,
            TypedExpr::Block(b) => b.span,
            TypedExpr::FunctionCall(call) => call.span,
            TypedExpr::If(ir_if) => ir_if.span,
            TypedExpr::ArrayIndex(op) => op.span,
            TypedExpr::StringIndex(op) => op.span,
            TypedExpr::OptionalSome(opt) => opt.inner_expr.get_span(),
            TypedExpr::OptionalHasValue(opt) => opt.get_span(),
            TypedExpr::OptionalGet(get) => get.span,
            TypedExpr::For(for_expr) => for_expr.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValDef {
    pub variable_id: VariableId,
    pub ty: TypeId,
    pub initializer: TypedExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ReturnStmt {
    pub expr: TypedExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub destination: Box<TypedExpr>,
    pub value: Box<TypedExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypedWhileLoop {
    pub cond: TypedExpr,
    pub block: TypedBlock,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypedStmt {
    Expr(Box<TypedExpr>),
    ValDef(Box<ValDef>),
    Assignment(Box<Assignment>),
    WhileLoop(Box<TypedWhileLoop>),
}

impl TypedStmt {
    #[inline]
    pub fn get_span(&self) -> Span {
        match self {
            TypedStmt::Expr(e) => e.get_span(),
            TypedStmt::ValDef(v) => v.span,
            TypedStmt::Assignment(ass) => ass.span,
            TypedStmt::WhileLoop(w) => w.span,
        }
    }
}

#[derive(Debug)]
pub struct TyperError {
    message: String,
    span: Span,
}

impl TyperError {
    fn make(message: impl AsRef<str>, span: Span) -> TyperError {
        TyperError { message: message.as_ref().to_owned(), span }
    }
}

impl Display for TyperError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("error on line {}: {}", self.span.line, self.message))
    }
}

impl Error for TyperError {}

pub type TyperResult<A> = Result<A, TyperError>;

#[derive(Debug)]
pub struct Variable {
    pub name: IdentifierId,
    pub type_id: TypeId,
    pub is_mutable: bool,
    pub owner_scope: Option<ScopeId>,
}

#[derive(Debug)]
pub struct Constant {
    pub variable_id: VariableId,
    pub expr: TypedExpr,
    pub ty: TypeId,
    pub span: Span,
}

pub struct Namespace {
    pub name: IdentifierId,
    pub scope_id: ScopeId,
}

pub struct Scopes {
    scopes: Vec<Scope>,
}

impl Scopes {
    fn make() -> Self {
        let scopes = vec![Scope::default()];
        Scopes { scopes }
    }

    pub fn get_root_scope_id(&self) -> ScopeId {
        0 as ScopeId
    }

    fn add_scope_to_root(&mut self) -> ScopeId {
        self.add_child_scope(0)
    }
    fn add_child_scope(&mut self, parent_scope_id: ScopeId) -> ScopeId {
        let scope = Scope { parent: Some(parent_scope_id), ..Scope::default() };
        let id = self.scopes.len() as ScopeId;
        self.scopes.push(scope);
        let parent_scope = self.get_scope_mut(parent_scope_id);
        parent_scope.children.push(id);
        id
    }

    pub fn get_scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id as usize]
    }

    pub fn get_scope_mut(&mut self, id: ScopeId) -> &mut Scope {
        &mut self.scopes[id as usize]
    }

    fn find_namespace(&self, scope: ScopeId, ident: IdentifierId) -> Option<NamespaceId> {
        let scope = self.get_scope(scope);
        if let ns @ Some(_r) = scope.find_namespace(ident) {
            return ns;
        }
        match scope.parent {
            Some(parent) => self.find_namespace(parent, ident),
            None => None,
        }
    }

    fn find_variable(&self, scope: ScopeId, ident: IdentifierId) -> Option<VariableId> {
        let scope = self.get_scope(scope);
        if let v @ Some(_r) = scope.find_variable(ident) {
            return v;
        }
        match scope.parent {
            Some(parent) => self.find_variable(parent, ident),
            None => None,
        }
    }

    fn add_variable(&mut self, scope_id: ScopeId, ident: IdentifierId, variable_id: VariableId) {
        let scope = self.get_scope_mut(scope_id);
        scope.add_variable(ident, variable_id);
    }

    fn find_function(&self, scope: ScopeId, ident: IdentifierId) -> Option<FunctionId> {
        let scope = self.get_scope(scope);
        if let f @ Some(_r) = scope.find_function(ident) {
            return f;
        }
        match scope.parent {
            Some(parent) => self.find_function(parent, ident),
            None => None,
        }
    }

    fn add_function(
        &mut self,
        scope_id: ScopeId,
        identifier: IdentifierId,
        function_id: FunctionId,
    ) {
        self.get_scope_mut(scope_id).add_function(identifier, function_id)
    }

    fn add_type(&mut self, scope_id: ScopeId, ident: IdentifierId, ty: TypeId) {
        self.get_scope_mut(scope_id).add_type(ident, ty)
    }

    fn find_type(&self, scope_id: ScopeId, ident: IdentifierId) -> Option<TypeId> {
        let scope = self.get_scope(scope_id);
        trace!("Find type {} in {:?}", ident, scope.types);
        if let v @ Some(_r) = scope.find_type(ident) {
            return v;
        }
        match scope.parent {
            Some(parent) => self.find_type(parent, ident),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntrinsicFunctionType {
    Exit,
    PrintInt,
    PrintString,
    StringLength,
    ArrayLength,
    ArrayNew,
    ArrayGrow,
    ArraySetLength,
    ArrayCapacity,
    StringFromCharArray,
    StringEquals,
}

impl IntrinsicFunctionType {
    pub fn from_function_name(value: &str) -> Option<Self> {
        match value {
            "printInt" => Some(IntrinsicFunctionType::PrintInt),
            "print" => Some(IntrinsicFunctionType::PrintString),
            "exit" => Some(IntrinsicFunctionType::Exit),
            _ => None,
        }
    }
}

#[derive(Default, Debug)]
pub struct Scope {
    variables: HashMap<IdentifierId, VariableId>,
    functions: HashMap<IdentifierId, FunctionId>,
    namespaces: HashMap<IdentifierId, NamespaceId>,
    types: HashMap<IdentifierId, TypeId>,
    parent: Option<ScopeId>,
    children: Vec<ScopeId>,
}

impl Scope {
    fn find_variable(&self, ident: IdentifierId) -> Option<VariableId> {
        self.variables.get(&ident).copied()
    }
    fn add_variable(&mut self, ident: IdentifierId, value: VariableId) {
        self.variables.insert(ident, value);
    }

    fn add_type(&mut self, ident: IdentifierId, ty: TypeId) {
        self.types.insert(ident, ty);
    }

    fn find_type(&self, ident: IdentifierId) -> Option<TypeId> {
        self.types.get(&ident).copied()
    }

    fn add_function(&mut self, ident: IdentifierId, function_id: FunctionId) {
        self.functions.insert(ident, function_id);
    }

    fn find_function(&self, ident: IdentifierId) -> Option<FunctionId> {
        self.functions.get(&ident).copied()
    }

    fn add_namespace(&mut self, ident: IdentifierId, namespace_id: NamespaceId) {
        self.namespaces.insert(ident, namespace_id);
    }

    fn find_namespace(&self, ident: IdentifierId) -> Option<NamespaceId> {
        self.namespaces.get(&ident).copied()
    }
}

fn make_err<T: AsRef<str>>(message: T, span: Span) -> TyperError {
    TyperError::make(message.as_ref(), span)
}

fn make_fail<A, T: AsRef<str>>(message: T, span: Span) -> TyperResult<A> {
    Err(make_err(message, span))
}

pub struct TypedModule {
    pub ast: Rc<ParsedModule>,
    functions: Vec<Function>,
    pub variables: Vec<Variable>,
    pub types: Vec<Type>,
    pub constants: Vec<Constant>,
    pub scopes: Scopes,
    pub errors: Vec<TyperError>,
    pub namespaces: Vec<Namespace>,
}

impl TypedModule {
    pub fn new(parsed_module: Rc<ParsedModule>) -> TypedModule {
        let scopes = Scopes::make();
        let root_ident = parsed_module.ident_id("_root");
        let types = vec![Type::Unit, Type::Char, Type::Int, Type::Bool, Type::String];
        TypedModule {
            ast: parsed_module,
            functions: Vec::new(),
            variables: Vec::new(),
            types,
            constants: Vec::new(),
            scopes: Scopes::make(),
            errors: Vec::new(),
            namespaces: vec![Namespace { name: root_ident, scope_id: scopes.get_root_scope_id() }],
        }
    }

    pub fn function_iter(&self) -> impl Iterator<Item = (FunctionId, &Function)> {
        self.functions.iter().enumerate().map(|(idx, f)| (idx as FunctionId, f))
    }

    fn internal_compiler_error(&self, message: impl AsRef<str>, span: Span) -> ! {
        self.print_error(message, span);
        panic!()
    }

    pub fn get_line_number(&self, span: Span) -> u32 {
        if span.line < crate::prelude::PRELUDE_LINES as u32 {
            // FIXME: Prelude needs to be in another file
            0
        } else {
            span.line_number() - crate::prelude::PRELUDE_LINES as u32
        }
    }

    fn print_error(&self, message: impl AsRef<str>, span: Span) {
        let adjusted_line = span.line as i32 - crate::prelude::PRELUDE_LINES as i32 + 1;
        let line_no =
            if adjusted_line < 0 { "PRELUDE".to_string() } else { adjusted_line.to_string() };
        eprintln!("{} at {}:{}\n  -> {}", "error".red(), self.name(), line_no, message.as_ref());
        eprintln!("{}", self.ast.source.get_line_by_index(span.line).red());
        eprintln!(" -> {}", self.ast.source.get_span_content(span).red());
    }

    pub fn name(&self) -> &str {
        &self.ast.name
    }

    fn get_ident_str(&self, id: IdentifierId) -> impl std::ops::Deref<Target = str> + '_ {
        self.ast.get_ident_str(id)
    }

    fn add_type(&mut self, typ: Type) -> TypeId {
        let id = self.types.len();
        self.types.push(typ);
        id as u32
    }

    // Should namespaces live in scopes instead of the module? Maybe scopes just have ident -> namespace_id
    fn get_namespace(&self, namespace_id: NamespaceId) -> Option<&Namespace> {
        self.namespaces.get(namespace_id as usize)
    }

    pub fn get_type(&self, type_id: TypeId) -> &Type {
        &self.types[type_id as usize]
    }

    pub fn get_type_dereferenced(&self, type_id: TypeId) -> &Type {
        match self.get_type(type_id) {
            Type::Reference(r) => self.get_type(r.inner_type),
            _ => self.get_type(type_id),
        }
    }

    pub fn get_type_mut(&mut self, type_id: TypeId) -> &mut Type {
        &mut self.types[type_id as usize]
    }

    // pub fn is_reference_type(&self, ty: TypeId) -> bool {
    //     match ty {
    //         TypeId::Unit => false,
    //         TypeId::Char => false,
    //         TypeId::Int => false,
    //         TypeId::Bool => false,
    //         TypeId::String => false,
    //         TypeId::TypeId(type_id) => {
    //             let ty = self.get_type(type_id);
    //             match ty {
    //                 Type::Record(_) => true,
    //                 Type::Array(_) => true,
    //                 Type::TypeVariable(_) => true,
    //                 Type::Optional(opt) => true,
    //             }
    //         }
    //     }
    // }

    /// Recursively checks if given type contains any type variables
    fn is_generic(&self, type_id: TypeId) -> bool {
        match self.get_type(type_id) {
            Type::Unit => false,
            Type::Char => false,
            Type::Int => false,
            Type::Bool => false,
            Type::String => false,
            Type::Array(arr) => self.is_generic(arr.element_type),
            // We don't yet support generics in records
            Type::Record(_record) => false,
            Type::Optional(opt) => self.is_generic(opt.inner_type),
            Type::Reference(refer) => self.is_generic(refer.inner_type),
            Type::TypeVariable(_) => true,
        }
    }

    fn item_type_of_iterable(&self, type_id: TypeId) -> Option<TypeId> {
        match self.get_type(type_id) {
            Type::Unit => None,
            Type::Char => None,
            Type::Int => None,
            Type::Bool => None,
            Type::String => Some(CHAR_TYPE_ID),
            Type::Array(arr) => Some(arr.element_type),
            Type::Record(_record) => None,
            Type::Optional(_opt) => None,
            Type::Reference(_refer) => None,
            Type::TypeVariable(_) => None,
        }
    }

    fn eval_type_defn(&mut self, defn: &parse::TypeDefn, scope_id: ScopeId) -> TyperResult<TypeId> {
        let type_id = self.eval_type_expr(&defn.value_expr, scope_id)?;
        match self.get_type_mut(type_id) {
            Type::Record(record_defn) => {
                // Add the name to this record defn so it can have associated
                // methods and constants
                record_defn.name_if_named = Some(defn.name);
                Ok(type_id)
            }
            _ => make_fail("Invalid rhs for named type definition", defn.value_expr.get_span()),
        }?;
        self.scopes.add_type(scope_id, defn.name, type_id);
        Ok(type_id)
    }

    fn eval_type_expr(
        &mut self,
        expr: &parse::ParsedTypeExpression,
        scope_id: ScopeId,
    ) -> TyperResult<TypeId> {
        let base = match expr {
            parse::ParsedTypeExpression::Unit(_) => Ok(UNIT_TYPE_ID),
            parse::ParsedTypeExpression::Char(_) => Ok(CHAR_TYPE_ID),
            parse::ParsedTypeExpression::Int(_) => Ok(INT_TYPE_ID),
            parse::ParsedTypeExpression::Bool(_) => Ok(BOOL_TYPE_ID),
            parse::ParsedTypeExpression::String(_) => Ok(STRING_TYPE_ID),
            parse::ParsedTypeExpression::Record(record_defn) => {
                let mut fields: Vec<RecordTypeField> = Vec::new();
                for (index, ast_field) in record_defn.fields.iter().enumerate() {
                    let ty = self.eval_type_expr(&ast_field.ty, scope_id)?;
                    fields.push(RecordTypeField { name: ast_field.name, type_id: ty, index })
                }
                let record_defn =
                    RecordType { fields, name_if_named: None, span: record_defn.span };
                let type_id = self.add_type(Type::Record(record_defn));
                Ok(type_id)
            }
            parse::ParsedTypeExpression::Name(ident, span) => {
                let ty_ref = self.scopes.find_type(scope_id, *ident);

                ty_ref.ok_or_else(|| {
                    error!("Scope {} Types: {:?}", scope_id, self.scopes.get_scope(scope_id).types);
                    error!(
                        "Scope {} Vars: {:?}",
                        scope_id,
                        self.scopes.get_scope(scope_id).variables
                    );
                    make_err(
                        format!(
                            "could not find type for identifier {}",
                            &*self.ast.get_ident_str(*ident)
                        ),
                        *span,
                    )
                })
            }
            parse::ParsedTypeExpression::TypeApplication(ty_app) => {
                if self.ast.ident_id("Array") == ty_app.base {
                    if ty_app.params.len() == 1 {
                        let element_ty = self.eval_type_expr(&ty_app.params[0], scope_id)?;
                        let array_ty = ArrayType { span: ty_app.span, element_type: element_ty };
                        let type_id = self.add_type(Type::Array(array_ty));
                        Ok(type_id)
                    } else {
                        self.internal_compiler_error(
                            "Expected 1 type parameter for Array",
                            ty_app.span,
                        )
                    }
                } else {
                    todo!("not supported: generic non builtin types")
                }
            }
            parse::ParsedTypeExpression::Optional(opt) => {
                let inner_ty = self.eval_type_expr(&opt.base, scope_id)?;
                let optional_type = Type::Optional(OptionalType { inner_type: inner_ty });
                let type_id = self.add_type(optional_type);
                Ok(type_id)
            }
            parse::ParsedTypeExpression::Reference(r) => {
                let inner_ty = self.eval_type_expr(&r.base, scope_id)?;
                let reference_type = Type::Reference(ReferenceType { inner_type: inner_ty });
                let type_id = self.add_type(reference_type);
                Ok(type_id)
            }
        }?;
        Ok(base)
    }

    fn eval_const_type_expr(&mut self, expr: &parse::ParsedTypeExpression) -> TyperResult<TypeId> {
        let ty = self.eval_type_expr(expr, self.scopes.get_root_scope_id())?;
        match ty {
            UNIT_TYPE_ID => Ok(ty),
            CHAR_TYPE_ID => Ok(ty),
            INT_TYPE_ID => Ok(ty),
            BOOL_TYPE_ID => Ok(ty),
            STRING_TYPE_ID => Ok(ty),
            _ => make_fail("Only scalar types allowed in constants", expr.get_span()),
        }
    }

    fn typecheck_record(&self, expected: &RecordType, actual: &RecordType) -> Result<(), String> {
        if expected.fields.len() != actual.fields.len() {
            return Err(format!(
                "expected record with {} fields, got {}",
                expected.fields.len(),
                actual.fields.len()
            ));
        }
        for expected_field in &expected.fields {
            trace!("typechecking record field {:?}", expected_field);
            let Some(matching_field) = actual.fields.iter().find(|f| f.name == expected_field.name)
            else {
                return Err(format!("expected record to have field {}", expected_field.name));
            };
            self.typecheck_types(expected_field.type_id, matching_field.type_id)?;
        }
        Ok(())
    }

    /// This implements 'duck-typing' for records, which is really cool
    /// but I do not want to do this by default since the codegen involves
    /// either v-tables or monomorphization of functions that accept records
    /// Maybe a <: syntax to opt-in to dynamic stuff like this, read as "conforms to"
    /// input <: {quack: () -> ()} means that it has at least a quack function
    /// fn takes_quacker = (input <: {quack: () -> ()}) -> ()
    ///
    /// "Conforms To" would mean that it has at least the same fields as the expected type, and
    /// it has them at least as strongly. If an optional is expected, actual can optional or required
    /// If a required is expected, actual must be required, etc. Basically TypeScripts structural typing
    #[allow(unused)]
    fn typecheck_record_duck(
        &self,
        expected: &RecordType,
        actual: &RecordType,
    ) -> Result<(), String> {
        for expected_field in &expected.fields {
            trace!("typechecking record field {:?}", expected_field);
            let Some(matching_field) = actual.fields.iter().find(|f| f.name == expected_field.name)
            else {
                return Err(format!("expected field {}", expected_field.name));
            };
            self.typecheck_types(matching_field.type_id, expected_field.type_id)?;
        }
        Ok(())
    }

    fn typecheck_types(&self, expected: TypeId, actual: TypeId) -> Result<(), String> {
        trace!(
            "typecheck expect {} actual {}",
            self.type_id_to_string(expected),
            self.type_id_to_string(actual)
        );
        if expected == actual {
            return Ok(());
        }
        match (self.get_type(expected), self.get_type(actual)) {
            (Type::Optional(o1), Type::Optional(o2)) => {
                self.typecheck_types(o1.inner_type, o2.inner_type)
            }
            (Type::Record(r1), Type::Record(r2)) => self.typecheck_record(r1, r2),
            (Type::Array(a1), Type::Array(a2)) => {
                self.typecheck_types(a1.element_type, a2.element_type)
            }
            (Type::TypeVariable(t1), Type::TypeVariable(t2)) => {
                // This is broken; should be more robust than just same identifier; we need to know the scope
                // so that we can resolve these type variables IF they point to something and typecheck those types
                if t1.identifier_id == t2.identifier_id {
                    Ok(())
                } else {
                    Err(format!(
                        "expected type variable {} but got {}",
                        &*self.get_ident_str(t1.identifier_id),
                        &*self.get_ident_str(t2.identifier_id)
                    ))
                }
            }
            (Type::Reference(o1), Type::Reference(o2)) => {
                self.typecheck_types(o1.inner_type, o2.inner_type)
            }
            (exp, got) => Err(format!(
                "Expected {} but got {}",
                self.type_to_string(exp),
                self.type_to_string(got)
            )),
        }
    }

    fn eval_const(&mut self, const_expr: &parse::ConstVal) -> TyperResult<VariableId> {
        let scope_id = 0;
        let type_id = self.eval_const_type_expr(&const_expr.ty)?;
        let expr = match &const_expr.value_expr.expr {
            ExpressionValue::Literal(Literal::Numeric(n, span)) => {
                let num = self.parse_numeric(n).map_err(|msg| make_err(msg, *span))?;
                TypedExpr::Int(num, const_expr.span)
            }
            ExpressionValue::Literal(Literal::Bool(b, span)) => TypedExpr::Bool(*b, *span),
            ExpressionValue::Literal(Literal::Char(c, span)) => TypedExpr::Char(*c, *span),
            _other => {
                return make_fail(
                    "Only literals are currently supported as constants",
                    const_expr.span,
                );
            }
        };
        let variable_id = self.add_variable(Variable {
            name: const_expr.name,
            type_id,
            is_mutable: false,
            owner_scope: None,
        });
        self.constants.push(Constant { variable_id, expr, ty: type_id, span: const_expr.span });
        self.scopes.add_variable(scope_id, const_expr.name, variable_id);
        Ok(variable_id)
    }

    fn get_stmt_expression_type(&self, stmt: &TypedStmt) -> TypeId {
        match stmt {
            TypedStmt::Expr(expr) => expr.get_type(),
            TypedStmt::ValDef(_) => UNIT_TYPE_ID,
            TypedStmt::Assignment(_) => UNIT_TYPE_ID,
            TypedStmt::WhileLoop(_) => UNIT_TYPE_ID,
        }
    }

    fn add_variable(&mut self, variable: Variable) -> VariableId {
        let id = self.variables.len();
        self.variables.push(variable);
        id as u32
    }

    pub fn get_variable(&self, id: VariableId) -> &Variable {
        &self.variables[id as usize]
    }

    fn add_function(&mut self, function: Function) -> FunctionId {
        let id = self.functions.len();
        self.functions.push(function);
        id as u32
    }

    fn add_namespace(&mut self, namespace: Namespace) -> NamespaceId {
        let id = self.namespaces.len();
        self.namespaces.push(namespace);
        id as u32
    }

    pub fn get_function(&self, function_id: FunctionId) -> &Function {
        &self.functions[function_id as usize]
    }

    pub fn get_function_mut(&mut self, function_id: FunctionId) -> &mut Function {
        &mut self.functions[function_id as usize]
    }

    fn parse_numeric(&self, s: &str) -> Result<i64, String> {
        // Eventually we need to find out what type of number literal this is.
        // For now we only support i64
        let num: i64 = s.parse().map_err(|_e| "Failed to parse signed numeric literal")?;
        Ok(num)
    }

    // If the expr is already a block, do nothing
    // If it is not, make a new block with just this expression inside.
    // Used main for if/else
    fn transform_expr_to_block(&mut self, expr: TypedExpr, block_scope: ScopeId) -> TypedBlock {
        match expr {
            TypedExpr::Block(b) => b,
            expr => {
                let ret_type = expr.get_type();
                let span = expr.get_span();
                let statement = TypedStmt::Expr(Box::new(expr));
                let statements = vec![statement];

                TypedBlock { expr_type: ret_type, scope_id: block_scope, statements, span }
            }
        }
    }

    fn coerce_block_to_unit_block(&mut self, block: &mut TypedBlock) {
        let span = block.statements.last().map(|s| s.get_span()).unwrap_or(block.span);
        let unit_literal = TypedExpr::unit_literal(span);
        block.statements.push(TypedStmt::Expr(Box::new(unit_literal)));
        block.expr_type = UNIT_TYPE_ID;
    }

    fn traverse_namespace_chain(
        &self,
        scope_id: ScopeId,
        namespaces: &[IdentifierId],
        span: Span,
    ) -> TyperResult<ScopeId> {
        trace!(
            "traverse_namespace_chain: {:?}",
            namespaces.iter().map(|id| self.get_ident_str(*id).to_string()).collect::<Vec<_>>()
        );
        let ns_iter = namespaces.iter();
        let mut cur_scope = scope_id;
        for ns in ns_iter {
            let namespace_id = self.scopes.find_namespace(cur_scope, *ns).ok_or(make_err(
                format!(
                    "Namespace not found: {} in scope: {:?}",
                    &*self.get_ident_str(*ns),
                    self.scopes.get_scope(scope_id)
                ),
                span,
            ))?;
            let namespace = self.get_namespace(namespace_id).unwrap();
            cur_scope = namespace.scope_id;
        }
        Ok(cur_scope)
    }

    fn eval_index_operation(
        &mut self,
        index_op: &IndexOperation,
        scope_id: ScopeId,
    ) -> TyperResult<TypedExpr> {
        let index_expr = self.eval_expr(&index_op.index_expr, scope_id, Some(INT_TYPE_ID))?;
        if index_expr.get_type() != INT_TYPE_ID {
            return make_fail("index type must be int", index_op.span);
        }

        let base_expr = self.eval_expr(&index_op.target, scope_id, None)?;
        let target_type = base_expr.get_type();
        match target_type {
            STRING_TYPE_ID => Ok(TypedExpr::StringIndex(IndexOp {
                base_expr: Box::new(base_expr),
                index_expr: Box::new(index_expr),
                result_type: CHAR_TYPE_ID,
                span: index_op.span,
            })),
            target_type_id => {
                let target_type = self.get_type(target_type_id);
                match target_type {
                    Type::Array(array_type) => Ok(TypedExpr::ArrayIndex(IndexOp {
                        base_expr: Box::new(base_expr),
                        index_expr: Box::new(index_expr),
                        result_type: array_type.element_type,
                        span: index_op.span,
                    })),
                    _ => make_fail("index base must be an array", index_op.span),
                }
            }
        }
    }

    fn eval_variable(
        &mut self,
        variable: &parse::Variable,
        scope_id: ScopeId,
        is_assignment_lhs: bool,
    ) -> TyperResult<TypedExpr> {
        let variable_id = self.scopes.find_variable(scope_id, variable.name).ok_or(make_err(
            format!("{} is not defined", &*self.get_ident_str(variable.name)),
            variable.span,
        ))?;
        let v = self.get_variable(variable_id);
        if is_assignment_lhs && !v.is_mutable {
            return make_fail(
                format!("Cannot assign to immutable variable {}", &*self.get_ident_str(v.name)),
                variable.span,
            );
        }
        let expr = TypedExpr::Variable(VariableExpr {
            type_id: v.type_id,
            variable_id,
            span: variable.span,
        });
        trace!(
            "variable {} had type {}",
            &*self.get_ident_str(variable.name),
            self.type_id_to_string(v.type_id)
        );
        Ok(expr)
    }

    fn eval_field_access(
        &mut self,
        field_access: &parse::FieldAccess,
        scope_id: ScopeId,
        is_assignment_lhs: bool,
    ) -> TyperResult<TypedExpr> {
        let mut base_expr = self.eval_expr(&field_access.base, scope_id, None)?;
        let type_id = base_expr.get_type();
        let maybe_record_type = match self.get_type(type_id) {
            Type::Reference(reference_type) => {
                // Auto de-reference records for  field access
                let maybe_record = self.get_type(reference_type.inner_type).as_record();
                if !is_assignment_lhs && maybe_record.is_some() {
                    // Dereference the base expression
                    base_expr = TypedExpr::UnaryOp(UnaryOp {
                        kind: UnaryOpKind::Dereference,
                        type_id: reference_type.inner_type,
                        span: base_expr.get_span(),
                        expr: Box::new(base_expr),
                    });
                }
                maybe_record
            }
            Type::Record(record_type) => Some(record_type),
            _ => None,
        };
        let Some(record_type) = maybe_record_type else {
            return make_fail(
                format!(
                    "Cannot access field {} on non-record type: {}",
                    &*self.get_ident_str(field_access.target),
                    self.type_id_to_string(base_expr.get_type())
                ),
                field_access.span,
            );
        };
        let (field_index, target_field) =
            record_type.find_field(field_access.target).ok_or(make_err(
                format!(
                    "Field {} not found on record type",
                    &*self.get_ident_str(field_access.target)
                ),
                field_access.span,
            ))?;
        Ok(TypedExpr::RecordFieldAccess(FieldAccess {
            base: Box::new(base_expr),
            target_field: field_access.target,
            target_field_index: field_index as u32,
            ty: target_field.type_id,
            span: field_access.span,
        }))
    }

    fn eval_assigment_lhs_expr(
        &mut self,
        expr: &Expression,
        scope_id: ScopeId,
        _expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        match &expr.expr {
            ExpressionValue::IndexOperation(index_op) => {
                self.eval_index_operation(index_op, scope_id)
            }
            ExpressionValue::Variable(variable) => self.eval_variable(variable, scope_id, true),
            ExpressionValue::FieldAccess(field_access) => {
                self.eval_field_access(field_access, scope_id, true)
            }
            other => {
                return make_fail(format!("Invalid assignment lhs: {:?}", other), other.get_span());
            }
        }
    }

    fn eval_expr(
        &mut self,
        expr: &Expression,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        let base_result = self.eval_expr_inner(expr, scope_id, expected_type)?;

        if let TypedExpr::None(_type_id, _span) = base_result {
            return Ok(base_result);
        }
        // Special case for some-boxing and auto-referencing
        if let Some(expected_type_id) = expected_type {
            match self.get_type(expected_type_id) {
                Type::Optional(optional_type) => {
                    trace!(
                        "some boxing: expected type is optional: {}",
                        self.type_id_to_string(expected_type_id)
                    );
                    trace!("some boxing: value is: {}", self.expr_to_string(&base_result));
                    trace!(
                        "some boxing: value type is: {}",
                        self.type_id_to_string(base_result.get_type())
                    );
                    match self.typecheck_types(optional_type.inner_type, base_result.get_type()) {
                        Ok(_) => Ok(TypedExpr::OptionalSome(OptionalSome {
                            inner_expr: Box::new(base_result),
                            type_id: expected_type_id,
                        })),
                        Err(_) => Ok(base_result),
                    }
                }
                Type::Reference(reference_type) => {
                    trace!(
                        "reference promotion: expected type is reference: {}",
                        self.type_id_to_string(expected_type_id)
                    );
                    trace!(
                        "reference promotion: value is: {}: {}",
                        self.expr_to_string(&base_result),
                        self.type_id_to_string(base_result.get_type())
                    );
                    match self.typecheck_types(reference_type.inner_type, base_result.get_type()) {
                        Ok(_) => Ok(TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::Reference,
                            type_id: expected_type_id,
                            expr: Box::new(base_result),
                            span: expr.get_span(),
                        })),
                        Err(_) => Ok(base_result),
                    }
                }
                _ => Ok(base_result),
            }
        } else {
            Ok(base_result)
        }
    }

    /// Passing `expected_type` is an optimization that can save us work.
    /// It does not guarantee that the returned expr always conforms
    /// to the given `expected_type`
    /// Although, maybe we re-think that because it would save
    /// a lot of code if we did a final check here before returning!
    fn eval_expr_inner(
        &mut self,
        expr: &Expression,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        trace!("eval_expr: {}: {:?}", expr, expected_type.map(|t| self.type_id_to_string(t)));
        let expected_type = match &expr.type_hint {
            None => expected_type,
            Some(type_hint_expr) => Some(self.eval_type_expr(type_hint_expr, scope_id)?),
        };
        match &expr.expr {
            ExpressionValue::Array(array_expr) => {
                let mut element_type: Option<TypeId> = match expected_type {
                    Some(type_id) => match self.get_type(type_id) {
                        Type::Array(arr) => Ok(Some(arr.element_type)),
                        _ => Ok(None),
                    },
                    None => Ok(None),
                }?;
                let elements: Vec<TypedExpr> = {
                    let mut elements = Vec::new();
                    for elem in &array_expr.elements {
                        let ir_expr = self.eval_expr(elem, scope_id, element_type)?;
                        if element_type.is_none() {
                            element_type = Some(ir_expr.get_type())
                        };
                        elements.push(ir_expr);
                    }
                    elements
                };

                let element_type = element_type.expect("By now this should be populated");
                // Technically we should not insert a new type here if we already have a type_id
                // representing an Array with this element type. But maybe we just make
                // the type internment do an equality check instead, so the 'consumer' code
                // throughout the compiler doesn't have to worry about creating or not creating
                // duplicate types; this is what Andrew Kelley just implemented with Zig's
                // intern pool that does full equality checking
                // https://github.com/ziglang/zig/pull/15569
                let type_id = match expected_type {
                    Some(t) => t,
                    None => {
                        let array_type = ArrayType { element_type, span: array_expr.span };
                        let type_id = self.add_type(Type::Array(array_type));
                        type_id
                    }
                };
                Ok(TypedExpr::Array(ArrayLiteral { elements, type_id, span: array_expr.span }))
            }
            ExpressionValue::IndexOperation(index_op) => {
                self.eval_index_operation(index_op, scope_id)
            }
            ExpressionValue::Record(ast_record) => {
                // FIXME: Let's factor out Structs and Records into separate things
                //        records can be created on the fly and are just hashmap literals
                //        Structs are structs
                let mut field_values = Vec::new();
                let mut field_defns = Vec::new();
                let expected_record = if let Some(expected_type) = expected_type {
                    match self.get_type(expected_type) {
                        Type::Record(record) => Some((expected_type, record.clone())),
                        Type::Optional(opt) => match self.get_type(opt.inner_type) {
                            Type::Record(record) => Some((opt.inner_type, record.clone())),
                            other_ty => {
                                return make_fail(
                                    format!(
                                        "Got record literal but expected {}",
                                        self.type_to_string(other_ty)
                                    ),
                                    ast_record.span,
                                );
                            }
                        },
                        Type::Reference(refer) => match self.get_type(refer.inner_type) {
                            Type::Record(record) => Some((refer.inner_type, record.clone())),
                            other_ty => {
                                return make_fail(
                                    format!(
                                        "Got record literal but expected {}",
                                        self.type_to_string(other_ty)
                                    ),
                                    ast_record.span,
                                );
                            }
                        },
                        other_ty => {
                            return make_fail(
                                format!(
                                    "Got record literal but expected {}",
                                    self.type_to_string(other_ty)
                                ),
                                ast_record.span,
                            );
                        }
                    }
                } else {
                    None
                };
                for (index, ast_field) in ast_record.fields.iter().enumerate() {
                    let expected_field = expected_record
                        .as_ref()
                        .map(|(_, rec)| rec.find_field(ast_field.name))
                        .flatten();
                    let expected_type_id = expected_field.map(|(_, f)| f.type_id);
                    let expr = self.eval_expr(&ast_field.expr, scope_id, expected_type_id)?;
                    field_defns.push(RecordTypeField {
                        name: ast_field.name,
                        type_id: expr.get_type(),
                        index,
                    });
                    field_values.push(RecordField { name: ast_field.name, expr });
                }
                // We can use 'expected type' here to just go ahead and typecheck or fail
                // rather than make a duplicate type
                let record_type_id = match expected_record {
                    None => {
                        let record_type = RecordType {
                            fields: field_defns,
                            name_if_named: None,
                            span: ast_record.span,
                        };
                        let anon_record_type_id = self.add_type(Type::Record(record_type));
                        Ok(anon_record_type_id)
                    }
                    Some((expected_type_id, expected_record)) => {
                        match self.typecheck_record(
                            &expected_record,
                            &RecordType {
                                fields: field_defns,
                                name_if_named: None,
                                span: ast_record.span,
                            },
                        ) {
                            Ok(_) => Ok(expected_type_id),
                            Err(s) => {
                                make_fail(format!("Invalid record type: {}", s), ast_record.span)
                            }
                        }
                    }
                }?;
                let typed_record =
                    Record { fields: field_values, span: ast_record.span, type_id: record_type_id };
                let expr = TypedExpr::Record(typed_record);
                trace!("generated record: {}", self.expr_to_string(&expr));
                Ok(expr)
            }
            ExpressionValue::If(if_expr) => self.eval_if_expr(if_expr, scope_id, expected_type),
            ExpressionValue::BinaryOp(binary_op) => {
                // Infer expected type to be type of operand1
                match binary_op.op_kind {
                    BinaryOpKind::Equals | BinaryOpKind::NotEquals => {
                        return self.eval_equality_expr(binary_op, scope_id, expected_type);
                    }
                    _ => {}
                };
                let lhs = self.eval_expr(&binary_op.lhs, scope_id, None)?;
                let rhs = self.eval_expr(&binary_op.rhs, scope_id, Some(lhs.get_type()))?;

                // FIXME: Typechecker We are not really typechecking binary operations at all.
                //        This is not enough; we need to check that the lhs is actually valid
                //        for this operation first
                if self.typecheck_types(lhs.get_type(), rhs.get_type()).is_err() {
                    return make_fail("operand types did not match", binary_op.span);
                }

                let kind = binary_op.op_kind;
                let result_type = match kind {
                    BinaryOpKind::Add => lhs.get_type(),
                    BinaryOpKind::Subtract => lhs.get_type(),
                    BinaryOpKind::Multiply => lhs.get_type(),
                    BinaryOpKind::Divide => lhs.get_type(),
                    BinaryOpKind::Less => BOOL_TYPE_ID,
                    BinaryOpKind::LessEqual => BOOL_TYPE_ID,
                    BinaryOpKind::Greater => BOOL_TYPE_ID,
                    BinaryOpKind::GreaterEqual => BOOL_TYPE_ID,
                    BinaryOpKind::And => lhs.get_type(),
                    BinaryOpKind::Or => lhs.get_type(),
                    BinaryOpKind::Equals => BOOL_TYPE_ID,
                    BinaryOpKind::NotEquals => BOOL_TYPE_ID,
                };
                let expr = TypedExpr::BinaryOp(BinaryOp {
                    kind,
                    ty: result_type,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span: binary_op.span,
                });
                Ok(expr)
            }
            ExpressionValue::UnaryOp(op) => {
                let base_expr = self.eval_expr(&op.expr, scope_id, None)?;
                match op.op_kind {
                    UnaryOpKind::Dereference => {
                        let reference_type =
                            self.get_type(base_expr.get_type()).as_reference().ok_or(make_err(
                                format!(
                                    "Cannot dereference non-reference type: {}",
                                    self.type_id_to_string(base_expr.get_type())
                                ),
                                op.span,
                            ))?;
                        eprintln!(
                            "DEREFERENCING: {}: {}",
                            self.expr_to_string(&base_expr),
                            self.type_id_to_string(base_expr.get_type())
                        );
                        Ok(TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::Dereference,
                            type_id: reference_type.inner_type,
                            expr: Box::new(base_expr),
                            span: op.span,
                        }))
                    }
                    UnaryOpKind::Reference => {
                        let type_id = self.add_type(Type::Reference(ReferenceType {
                            inner_type: base_expr.get_type(),
                        }));
                        Ok(TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::Reference,
                            type_id,
                            expr: Box::new(base_expr),
                            span: op.span,
                        }))
                    }
                    UnaryOpKind::BooleanNegation => {
                        self.typecheck_types(BOOL_TYPE_ID, base_expr.get_type())
                            .map_err(|s| make_err(s, op.span))?;
                        Ok(TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::BooleanNegation,
                            type_id: BOOL_TYPE_ID,
                            expr: Box::new(base_expr),
                            span: op.span,
                        }))
                    }
                }
            }
            ExpressionValue::Literal(Literal::Unit(span)) => Ok(TypedExpr::Unit(*span)),
            ExpressionValue::Literal(Literal::None(span)) => {
                // If we are expecting an Option, I need to reach inside it to get the inner type
                let expected_type = expected_type.ok_or(make_err(
                    "Cannot infer type of None literal without type hint",
                    *span,
                ))?;
                let expected_type = self.get_type(expected_type).as_optional().ok_or(make_err(
                    format!("Expected optional type for None literal but got {:?}", expected_type),
                    *span,
                ))?;
                let inner_type = expected_type.inner_type;
                let none_type = Type::Optional(OptionalType { inner_type });
                // FIXME: We'll re-create the type for optional int, bool, etc over and over. Instead of add_type it should be
                //        self.get_or_add_type()
                let type_id = self.add_type(none_type);
                Ok(TypedExpr::None(type_id, *span))
            }
            ExpressionValue::Literal(Literal::Char(byte, span)) => {
                Ok(TypedExpr::Char(*byte, *span))
            }
            ExpressionValue::Literal(Literal::Numeric(s, span)) => {
                let num = self.parse_numeric(s).map_err(|msg| make_err(msg, *span))?;
                Ok(TypedExpr::Int(num, *span))
            }
            ExpressionValue::Literal(Literal::Bool(b, span)) => {
                let expr = TypedExpr::Bool(*b, *span);
                Ok(expr)
            }
            ExpressionValue::Literal(Literal::String(s, span)) => {
                // So sad, we could just point to the source. BUT if we ever do escaping and things
                // then the source string is not the same as the string the user meant; perhaps here
                // is the place, or maybe in the parser, that we would actually do some work, which
                // would justify storing it separately. But then, post-transform, we should intern
                // these
                let expr = TypedExpr::Str(s.clone(), *span);
                Ok(expr)
            }
            ExpressionValue::Variable(variable) => self.eval_variable(variable, scope_id, false),
            ExpressionValue::FieldAccess(field_access) => {
                self.eval_field_access(field_access, scope_id, false)
            }
            ExpressionValue::Block(block) => {
                let block = self.eval_block(block, scope_id, expected_type)?;
                Ok(TypedExpr::Block(block))
            }
            ExpressionValue::MethodCall(m_call) => {
                let base_expr = self.eval_expr(&m_call.base, scope_id, None)?;
                let call =
                    self.eval_function_call(&m_call.call, Some(base_expr), None, scope_id)?;
                Ok(call)
            }
            ExpressionValue::FnCall(fn_call) => {
                let call = self.eval_function_call(fn_call, None, None, scope_id)?;
                Ok(call)
            }
            ExpressionValue::OptionalGet(optional_get) => {
                let base = self.eval_expr_inner(&optional_get.base, scope_id, expected_type)?;
                let Type::Optional(optional_type) = self.get_type(base.get_type()) else {
                    return make_fail(
                        format!(
                            "Cannot get value with ! from non-optional type: {}",
                            self.type_id_to_string(base.get_type())
                        ),
                        optional_get.span,
                    );
                };
                Ok(TypedExpr::OptionalGet(OptionalGet {
                    inner_expr: Box::new(base),
                    result_type_id: optional_type.inner_type,
                    span: optional_get.span,
                }))
            }
            ExpressionValue::For(for_expr) => self.eval_for_expr(for_expr, scope_id, expected_type),
        }
    }

    fn synth_variable_decl(
        &mut self,
        mut variable: Variable,
        span: Span,
        initializer: TypedExpr,
        no_mangle: bool,
    ) -> (VariableId, TypedStmt, TypedExpr) {
        let type_id = variable.type_id;
        let scope_id = variable.owner_scope.unwrap_or(self.scopes.get_root_scope_id());
        let new_ident = if no_mangle {
            variable.name
        } else {
            let new_ident_name =
                { format!("__{}_{}", &*self.ast.get_ident_str(variable.name), scope_id) };
            self.ast.ident_id(&new_ident_name)
        };
        variable.name = new_ident;
        let variable_id = self.add_variable(variable);
        let expr = VariableExpr { type_id, variable_id, span };
        let val_def =
            TypedStmt::ValDef(Box::new(ValDef { variable_id, ty: type_id, initializer, span }));
        self.scopes.add_variable(scope_id, new_ident, variable_id);
        (variable_id, val_def, TypedExpr::Variable(expr))
    }

    fn synth_variable_parsed_expr(&self, variable_id: VariableId, span: Span) -> parse::Expression {
        ExpressionValue::Variable(parse::Variable {
            name: self.get_variable(variable_id).name,
            namespaces: Vec::new(),
            span,
        })
        .into()
    }

    fn synth_function_call(
        &mut self,
        namespaces: Vec<IdentifierId>,
        name: IdentifierId,
        known_type_args: Option<Vec<TypeId>>,
        value_arg_exprs: Vec<Expression>,
        span: Span,
        scope_id: ScopeId,
    ) -> TyperResult<TypedExpr> {
        self.eval_function_call(
            &FnCall {
                name,
                type_args: None,
                args: value_arg_exprs
                    .into_iter()
                    .map(|expr| FnCallArg { name: None, value: expr })
                    .collect(),
                namespaces,
                span,
            },
            None,
            known_type_args,
            scope_id,
        )
    }

    fn eval_for_expr(
        &mut self,
        for_expr: &ForExpr,
        scope_id: ScopeId,
        _expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        let binding_ident = for_expr.binding.unwrap_or(self.ast.ident_id("it"));
        let iterable_expr = self.eval_expr(&for_expr.iterable_expr, scope_id, None)?;
        let iteree_type = iterable_expr.get_type();
        let is_string_iteree = iteree_type == STRING_TYPE_ID;
        let Some(item_type) = self.item_type_of_iterable(iteree_type) else {
            return make_fail(
                format!(
                    "Type {} is not iterable",
                    self.type_id_to_string(iterable_expr.get_type())
                ),
                for_expr.iterable_expr.get_span(),
            );
        };

        let is_do_block = for_expr.expr_type == ForExprType::Do;

        // We de-sugar the 'for ... do' expr into a typed while loop, synthesizing
        // a few local variables in order to achieve this.
        // I think these are broken scoping-wise and will collide with user code currently!

        let for_expr_scope = self.scopes.add_child_scope(scope_id);

        let (index_variable, index_defn_stmt, index_variable_expr) = self.synth_variable_decl(
            Variable {
                name: self.ast.ident_id("it_index"),
                type_id: INT_TYPE_ID,
                is_mutable: true,
                owner_scope: Some(for_expr_scope),
            },
            for_expr.body_block.span,
            TypedExpr::Int(0, for_expr.body_block.span),
            true,
        );
        let (iteree_variable_id, iteree_defn_stmt, iteree_variable_expr) = self
            .synth_variable_decl(
                Variable {
                    name: self.ast.ident_id("iteree"),
                    type_id: iteree_type,
                    is_mutable: false,
                    owner_scope: Some(for_expr_scope),
                },
                for_expr.iterable_expr.get_span(),
                iterable_expr,
                false,
            );
        let iteree_length_call = if is_string_iteree {
            self.synth_function_call(
                vec![self.ast.ident_id("string")],
                self.ast.ident_id("length"),
                None,
                vec![self.synth_variable_parsed_expr(
                    iteree_variable_id,
                    for_expr.iterable_expr.get_span(),
                )],
                for_expr.iterable_expr.get_span(),
                for_expr_scope,
            )?
        } else {
            self.synth_function_call(
                vec![self.ast.ident_id("Array")],
                self.ast.ident_id("length"),
                Some(vec![item_type]),
                vec![self.synth_variable_parsed_expr(
                    iteree_variable_id,
                    for_expr.iterable_expr.get_span(),
                )],
                for_expr.iterable_expr.get_span(),
                for_expr_scope,
            )?
        };
        let (iteree_length_variable_id, iteree_length_defn_stmt, iteree_length_variable_expr) =
            self.synth_variable_decl(
                Variable {
                    name: self.ast.ident_id("iteree_length"),
                    type_id: INT_TYPE_ID,
                    is_mutable: false,
                    owner_scope: Some(for_expr_scope),
                },
                for_expr.iterable_expr.get_span(),
                iteree_length_call,
                false,
            );

        let while_scope_id = self.scopes.add_child_scope(for_expr_scope);
        let binding_variable_id = self.add_variable(Variable {
            name: binding_ident,
            type_id: item_type,
            is_mutable: false,
            owner_scope: Some(while_scope_id),
        });
        self.scopes.add_variable(while_scope_id, binding_ident, binding_variable_id);
        let iteration_element_val_def = TypedStmt::ValDef(Box::new(ValDef {
            variable_id: binding_variable_id,
            ty: item_type,
            initializer: if is_string_iteree {
                TypedExpr::StringIndex(IndexOp {
                    base_expr: Box::new(iteree_variable_expr.clone()),
                    index_expr: Box::new(TypedExpr::Variable(VariableExpr {
                        type_id: INT_TYPE_ID,
                        variable_id: index_variable,
                        span: for_expr.body_block.span,
                    })),
                    result_type: item_type,
                    span: for_expr.body_block.span,
                })
            } else {
                TypedExpr::ArrayIndex(IndexOp {
                    base_expr: Box::new(iteree_variable_expr.clone()),
                    index_expr: Box::new(TypedExpr::Variable(VariableExpr {
                        type_id: INT_TYPE_ID,
                        variable_id: index_variable,
                        span: for_expr.body_block.span,
                    })),
                    result_type: item_type,
                    span: for_expr.body_block.span,
                })
            },
            span: for_expr.body_block.span,
        }));

        // TODO: we can hint to the block based on the expected type of the entire for expr
        let body_scope_id = self.scopes.add_child_scope(while_scope_id);
        let body_block = self.eval_block(&for_expr.body_block, body_scope_id, None)?;
        let body_block_result_type = body_block.expr_type;

        let resulting_type = if is_do_block {
            UNIT_TYPE_ID
        } else {
            match self.get_type(iteree_type) {
                Type::Optional(_opt) => {
                    let new_optional =
                        Type::Optional(OptionalType { inner_type: body_block.expr_type });
                    self.add_type(new_optional)
                }
                Type::Array(_arr) => {
                    let new_array = Type::Array(ArrayType {
                        element_type: body_block.expr_type,
                        span: for_expr.span,
                    });
                    self.add_type(new_array)
                }
                Type::String => {
                    let new_array = Type::Array(ArrayType {
                        element_type: body_block.expr_type,
                        span: for_expr.span,
                    });
                    self.add_type(new_array)
                }
                other => todo!("Unsupported iteree type: {}", self.type_to_string(other)),
            }
        };
        let yield_decls = if !is_do_block {
            let yield_variable = Variable {
                name: self.ast.ident_id("yielded_coll"),
                type_id: resulting_type,
                is_mutable: false,
                owner_scope: Some(for_expr_scope),
            };

            let yield_initializer = match self.get_type(resulting_type) {
                Type::Array(_array_type) => self.synth_function_call(
                    vec![self.ast.ident_id("Array")],
                    self.ast.ident_id("new"),
                    Some(vec![body_block_result_type]),
                    vec![self.synth_variable_parsed_expr(
                        iteree_length_variable_id,
                        for_expr.iterable_expr.get_span(),
                    )],
                    for_expr.body_block.span,
                    for_expr_scope,
                )?,
                _ => {
                    return make_fail("unsupported resulting_type in for_expr yield", for_expr.span)
                }
            };
            Some(self.synth_variable_decl(
                yield_variable,
                for_expr.body_block.span,
                yield_initializer,
                false,
            ))
        } else {
            None
        };
        let mut while_block = TypedBlock {
            expr_type: UNIT_TYPE_ID,
            scope_id: while_scope_id,
            statements: Vec::new(),
            span: for_expr.body_block.span,
        };
        // Prepend the val def to the body block
        while_block.statements.push(iteration_element_val_def);
        let (_user_block_val_id, user_block_val_def, user_block_val_expr) = self
            .synth_variable_decl(
                Variable {
                    name: self.ast.ident_id("block_expr_val"),
                    type_id: body_block.expr_type,
                    is_mutable: false,
                    owner_scope: Some(while_scope_id),
                },
                for_expr.body_block.span,
                TypedExpr::Block(body_block),
                false,
            );
        while_block.statements.push(user_block_val_def);

        // Assign element to yielded array
        if let Some((_yield_coll_variable_id, _yield_def, yielded_coll_expr)) = &yield_decls {
            match self.get_type(resulting_type) {
                Type::Array(_) => {
                    // yielded_coll[index] = block_expr_val;
                    let element_assign = TypedStmt::Assignment(Box::new(Assignment {
                        destination: Box::new(TypedExpr::ArrayIndex(IndexOp {
                            base_expr: Box::new(yielded_coll_expr.clone()),
                            index_expr: Box::new(index_variable_expr.clone()),
                            result_type: body_block_result_type,
                            span: for_expr.body_block.span,
                        })),
                        value: Box::new(user_block_val_expr),
                        span: for_expr.body_block.span,
                    }));
                    while_block.statements.push(element_assign);
                }
                _ => {}
            }
        }

        // Append the index increment to the body block
        let index_increment_statement = TypedStmt::Assignment(Box::new(Assignment {
            destination: Box::new(index_variable_expr.clone()),
            value: Box::new(TypedExpr::BinaryOp(BinaryOp {
                kind: BinaryOpKind::Add,
                ty: INT_TYPE_ID,
                lhs: Box::new(TypedExpr::Variable(VariableExpr {
                    type_id: INT_TYPE_ID,
                    variable_id: index_variable,
                    span: for_expr.iterable_expr.get_span(),
                })),
                rhs: Box::new(TypedExpr::Int(1, for_expr.iterable_expr.get_span())),
                span: for_expr.iterable_expr.get_span(),
            })),
            span: for_expr.iterable_expr.get_span(),
        }));
        while_block.statements.push(index_increment_statement);

        let while_stmt = TypedStmt::WhileLoop(Box::new(TypedWhileLoop {
            cond: TypedExpr::BinaryOp(BinaryOp {
                kind: BinaryOpKind::Less,
                ty: BOOL_TYPE_ID,
                lhs: Box::new(index_variable_expr.clone()),
                rhs: Box::new(iteree_length_variable_expr),
                span: for_expr.iterable_expr.get_span(),
            }),
            block: while_block,
            span: for_expr.span,
        }));

        let mut for_expr_initial_statements = Vec::with_capacity(4);
        for_expr_initial_statements.push(index_defn_stmt);
        for_expr_initial_statements.push(iteree_defn_stmt);
        for_expr_initial_statements.push(iteree_length_defn_stmt);
        if let Some((_yield_id, yield_def, _yield_expr)) = &yield_decls {
            for_expr_initial_statements.push(yield_def.clone());
        }
        let mut for_expr_block = TypedBlock {
            expr_type: resulting_type,
            scope_id: for_expr_scope,
            statements: for_expr_initial_statements,
            span: for_expr.body_block.span,
        };

        for_expr_block.statements.push(while_stmt);
        if let Some((yielded_variable_id, _, _)) = &yield_decls {
            let yield_expr = TypedExpr::Variable(VariableExpr {
                type_id: resulting_type,
                variable_id: *yielded_variable_id,
                span: for_expr.body_block.span,
            });
            for_expr_block.statements.push(TypedStmt::Expr(Box::new(yield_expr)));
        }

        let final_expr = TypedExpr::Block(for_expr_block);
        // eprintln!("{}", self.expr_to_string(&final_expr));
        Ok(final_expr)
    }

    fn eval_equality_expr(
        &mut self,
        binary_op: &parse::BinaryOp,
        scope_id: ScopeId,
        _expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        assert!(
            binary_op.op_kind == BinaryOpKind::Equals
                || binary_op.op_kind == BinaryOpKind::NotEquals
        );
        let lhs = self.eval_expr(&binary_op.lhs, scope_id, None)?;
        let equals_expr = match self.get_type(lhs.get_type()) {
            Type::Unit | Type::Char | Type::Int | Type::Bool => {
                // All these scalar types treated as IntValue s in codegen
                let rhs = self.eval_expr(&binary_op.rhs, scope_id, Some(lhs.get_type()))?;
                if rhs.get_type() == lhs.get_type() {
                    Ok(TypedExpr::BinaryOp(BinaryOp {
                        kind: BinaryOpKind::Equals,
                        ty: BOOL_TYPE_ID,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        span: binary_op.span,
                    }))
                } else {
                    make_fail("Bad rhs type", binary_op.span)
                }
            }
            Type::String => {
                let rhs = self.eval_expr(&binary_op.rhs, scope_id, Some(lhs.get_type()))?;
                if rhs.get_type() != STRING_TYPE_ID {
                    make_fail("Expected string on rhs", binary_op.span)
                } else {
                    let string_scope =
                        self.get_namespace_scope_for_ident(scope_id, self.ast.ident_id("string"));
                    let string_equality_function = string_scope
                        .find_function(self.ast.ident_id("equals"))
                        .ok_or(make_err("Missing equality function for string", binary_op.span))?;
                    let call_expr = TypedExpr::FunctionCall(Call {
                        callee_function_id: string_equality_function,
                        args: vec![lhs, rhs],
                        ret_type: BOOL_TYPE_ID,
                        span: binary_op.span,
                    });
                    Ok(call_expr)
                }
            }
            Type::Record(_record_defn) => {
                todo!("record equality")
            }
            Type::Array(array) => {
                // We need a cleaner way to generate _generic_ calls in typer
                // known_type_args is a good start, but I think we need to be able to
                // pass in pre-evaluated value args as well and pick up from there with specialization
                let array_equality_call = self.synth_function_call(
                    vec![self.ast.ident_id("Array")],
                    self.ast.ident_id("equals"),
                    Some(vec![array.element_type]),
                    vec![*binary_op.lhs.clone(), *binary_op.rhs.clone()],
                    binary_op.span,
                    scope_id,
                )?;
                Ok(array_equality_call)
            }
            Type::Optional(_opt) => {
                todo!("optional equality")
            }
            Type::Reference(_refer) => {
                todo!("reference equality")
            }
            Type::TypeVariable(_type_var) => {
                let rhs = self.eval_expr(&binary_op.rhs, scope_id, Some(lhs.get_type()))?;
                if let Err(msg) = self.typecheck_types(lhs.get_type(), rhs.get_type()) {
                    return make_fail(
                        format!("Type mismatch on equality of 2 generic variables: {}", msg),
                        binary_op.span,
                    );
                }
                Ok(TypedExpr::BinaryOp(BinaryOp {
                    kind: BinaryOpKind::Equals,
                    ty: BOOL_TYPE_ID,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span: binary_op.span,
                }))
            }
        }?;
        if binary_op.op_kind == BinaryOpKind::Equals {
            Ok(equals_expr)
        } else {
            Ok(TypedExpr::UnaryOp(UnaryOp {
                kind: UnaryOpKind::BooleanNegation,
                type_id: BOOL_TYPE_ID,
                expr: Box::new(equals_expr),
                span: binary_op.span,
            }))
        }
    }

    fn eval_if_expr(
        &mut self,
        if_expr: &IfExpr,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        let mut condition = self.eval_expr(&if_expr.cond, scope_id, None)?;
        let consequent_scope_id = self.scopes.add_child_scope(scope_id);
        let mut consequent = if if_expr.optional_ident.is_some() {
            let condition_optional_type = match self.get_type(condition.get_type()) {
                Type::Optional(opt) => opt,
                _other => {
                    return make_fail(
                        "Condition type for if with binding must be an optional",
                        if_expr.cond.get_span(),
                    );
                }
            };
            let inner_type = condition_optional_type.inner_type;
            let (binding, binding_span) = if_expr.optional_ident.expect("We already checked this");
            // Make a variable with the identifier binding from the expr
            // That is the non-optional type of the condition's type
            let narrowed_variable = Variable {
                name: binding,
                type_id: inner_type,
                is_mutable: false,
                // This should be the scope of the consequent expr
                owner_scope: Some(scope_id),
            };
            let narrowed_variable_id = self.add_variable(narrowed_variable);
            let consequent_scope = self.scopes.get_scope_mut(consequent_scope_id);
            consequent_scope.add_variable(binding, narrowed_variable_id);
            let original_condition = condition.clone();
            condition = TypedExpr::OptionalHasValue(Box::new(condition));
            let consequent_expr =
                self.eval_expr(&if_expr.cons, consequent_scope_id, expected_type)?;
            let mut consequent = self.transform_expr_to_block(consequent_expr, consequent_scope_id);
            consequent.statements.insert(
                0,
                TypedStmt::ValDef(Box::new(ValDef {
                    variable_id: narrowed_variable_id,
                    ty: inner_type,
                    initializer: TypedExpr::OptionalGet(OptionalGet {
                        inner_expr: Box::new(original_condition),
                        result_type_id: inner_type,
                        span: binding_span,
                    }),
                    span: binding_span,
                })),
            );
            consequent
        } else {
            // If there is no binding, the condition must be a boolean
            if let Err(msg) = self.typecheck_types(BOOL_TYPE_ID, condition.get_type()) {
                return make_fail(
                    format!("Invalid if condition type: {}. If you intended to use a binding optional if, you must supply a binding using |<ident>|", msg),
                    if_expr.cond.get_span(),
                );
            }
            let consequent_expr =
                self.eval_expr(&if_expr.cons, consequent_scope_id, expected_type)?;
            self.transform_expr_to_block(consequent_expr, consequent_scope_id)
        };
        let consequent_type = consequent.expr_type;
        // De-sugar if without else:
        // If there is no alternate, we coerce the consequent to return Unit, so both
        // branches have a matching type, making codegen simpler
        if if_expr.alt.is_none() {
            self.coerce_block_to_unit_block(&mut consequent);
        };
        let alternate_scope = self.scopes.add_child_scope(scope_id);
        let alternate = if let Some(alt) = &if_expr.alt {
            let expr = self.eval_expr(alt, alternate_scope, Some(consequent_type))?;
            self.transform_expr_to_block(expr, alternate_scope)
        } else {
            TypedBlock {
                expr_type: UNIT_TYPE_ID,
                scope_id: alternate_scope,
                statements: vec![TypedStmt::Expr(Box::new(TypedExpr::unit_literal(if_expr.span)))],
                span: if_expr.span,
            }
        };
        if let Err(msg) = self.typecheck_types(consequent.expr_type, alternate.expr_type) {
            return make_fail(
                format!("else branch type did not match then branch type: {}", msg),
                alternate.span,
            );
        }
        let overall_type = consequent.expr_type;
        Ok(TypedExpr::If(Box::new(TypedIf {
            condition,
            consequent,
            alternate,
            ty: overall_type,
            span: if_expr.span,
        })))
    }

    fn get_namespace_scope_for_ident(
        &self,
        scope_id: ScopeId,
        identifier_id: IdentifierId,
    ) -> &Scope {
        let namespace_id = self.scopes.find_namespace(scope_id, identifier_id).unwrap();
        let namespace = self.get_namespace(namespace_id).unwrap();
        let scope = self.scopes.get_scope(namespace.scope_id);
        scope
    }

    /// Can 'shortcircuit' with Left if the function call to resolve
    /// is actually a builtin
    fn resolve_function_call(
        &self,
        fn_call: &FnCall,
        this_expr: Option<&TypedExpr>,
        calling_scope: ScopeId,
    ) -> TyperResult<Either<TypedExpr, FunctionId>> {
        let function_id = match this_expr {
            Some(base_expr) => {
                // Resolve a method call
                let type_id = base_expr.get_type();
                let function_id = match self.get_type_dereferenced(type_id) {
                    Type::String => {
                        let string_ident_id = self.ast.ident_id("string");
                        let string_scope =
                            self.get_namespace_scope_for_ident(calling_scope, string_ident_id);
                        string_scope.find_function(fn_call.name)
                    }
                    Type::Char => {
                        let char_ident_id = self.ast.ident_id("char");
                        let char_scope =
                            self.get_namespace_scope_for_ident(calling_scope, char_ident_id);
                        char_scope.find_function(fn_call.name)
                    }
                    Type::Array(_array_type) => {
                        let array_ident_id = self.ast.ident_id("Array");
                        let array_scope =
                            self.get_namespace_scope_for_ident(calling_scope, array_ident_id);
                        array_scope.find_function(fn_call.name)
                    }
                    Type::Optional(_optional_type) => {
                        if fn_call.name == self.ast.ident_id("hasValue")
                            && fn_call.args.is_empty()
                            && fn_call.type_args.is_none()
                        {
                            return Ok(Either::Left(TypedExpr::OptionalHasValue(Box::new(
                                base_expr.clone(),
                            ))));
                        } else {
                            None
                        }
                    }
                    Type::Record(record) => {
                        // Need to distinguish between instances of 'named'
                        // records and anonymous ones
                        let Some(record_type_name) = record.name_if_named else {
                            return make_fail(
                                "Anonymous records currently have no methods",
                                record.span,
                            );
                        };
                        let record_scope =
                            self.get_namespace_scope_for_ident(calling_scope, record_type_name);
                        record_scope.find_function(fn_call.name)
                    }
                    _ => None,
                };
                match function_id {
                    Some(function_id) => function_id,
                    None => {
                        return make_fail(
                            format!(
                                "Method {} does not exist on type {:?}",
                                &*self.get_ident_str(fn_call.name),
                                self.type_id_to_string(type_id),
                            ),
                            fn_call.span,
                        );
                    }
                }
            }
            None => {
                // Resolve a non-method call
                let scope_to_search = self.traverse_namespace_chain(
                    calling_scope,
                    &fn_call.namespaces,
                    fn_call.span,
                )?;
                let function_id =
                    self.scopes.find_function(scope_to_search, fn_call.name).ok_or(make_err(
                        format!(
                            "Function not found: {} in scope: {:?}",
                            &*self.get_ident_str(fn_call.name),
                            self.scopes.get_scope(calling_scope)
                        ),
                        fn_call.span,
                    ))?;
                function_id
            }
        };
        return Ok(Either::Right(function_id));
    }

    // I actually just want this to handle the 'self' order and zipping thing because I sometimes
    // don't want these checked just yet because I'm using them for inference
    fn typecheck_call_arguments(
        &mut self,
        fn_call: &FnCall,
        this_expr: Option<TypedExpr>,
        params: &Vec<FnArgDefn>,
        calling_scope: ScopeId,
        skip_typecheck: bool,
    ) -> TyperResult<Vec<TypedExpr>> {
        let mut final_args: Vec<TypedExpr> = Vec::new();
        // We have to deal with `self` arguments outside of the loop because
        // we can't 'move' out of this_expr more than once
        let mut skip_first = false;
        if let Some(first) = params.get(0) {
            let is_self = first.name == self.ast.ident_id("self");
            if is_self {
                if let Some(this) = this_expr {
                    if !skip_typecheck {
                        if let Err(e) = self.typecheck_types(first.type_id, this.get_type()) {
                            return make_fail(
                                format!(
                                    "Invalid parameter type for 'self' to function {}: {}",
                                    &*self.ast.get_ident_str(fn_call.name),
                                    e
                                ),
                                fn_call.span,
                            );
                        }
                    }
                    final_args.push(this);
                    skip_first = true;
                }
            }
        }
        let start: u32 = if skip_first { 1 } else { 0 };
        for fn_param in &params[start as usize..] {
            let matching_param_by_name =
                fn_call.args.iter().find(|arg| arg.name == Some(fn_param.name));
            // If we skipped 'self', we need to subtract 1 from the offset we index into fn_call.args with
            let matching_idx = fn_param.position - start;
            let matching_param = matching_param_by_name.or(fn_call.args.get(matching_idx as usize));
            if let Some(param) = matching_param {
                let expected_type_for_param =
                    if skip_typecheck { None } else { Some(fn_param.type_id) };
                let expr = self.eval_expr(&param.value, calling_scope, expected_type_for_param)?;
                if !skip_typecheck {
                    if let Err(e) = self.typecheck_types(fn_param.type_id, expr.get_type()) {
                        return make_fail(
                            format!(
                                "Invalid parameter type passed to function {}: {}",
                                &*self.ast.get_ident_str(fn_call.name),
                                e
                            ),
                            param.value.get_span(),
                        );
                    }
                }
                final_args.push(expr);
            } else {
                return make_fail(
                    format!(
                        "Missing argument to function {}: {}",
                        &*self.ast.get_ident_str(fn_call.name),
                        &*self.get_ident_str(fn_param.name)
                    ),
                    fn_call.span,
                );
            }
        }
        Ok(final_args)
    }

    fn eval_function_call(
        &mut self,
        fn_call: &FnCall,
        this_expr: Option<TypedExpr>,
        known_type_args: Option<Vec<TypeId>>,
        scope_id: ScopeId,
    ) -> TyperResult<TypedExpr> {
        // Special case for Some() because it is parsed as a function call
        // but should result in a special expression
        if fn_call.name == self.ast.ident_id("Some") {
            if fn_call.args.len() != 1 {
                return make_fail("Some() must have exactly one argument", fn_call.span);
            }
            let arg = self.eval_expr_inner(&fn_call.args[0].value, scope_id, None)?;
            let type_id = arg.get_type();
            let optional_type = Type::Optional(OptionalType { inner_type: type_id });
            let type_id = self.add_type(optional_type);
            return Ok(TypedExpr::OptionalSome(OptionalSome {
                inner_expr: Box::new(arg),
                type_id,
            }));
        }

        let function_id = match self.resolve_function_call(fn_call, this_expr.as_ref(), scope_id)? {
            Either::Left(expr) => return Ok(expr),
            Either::Right(function_id) => function_id,
        };

        // Now that we have resolved to a function id, we need to specialize it if generic
        let original_function = self.get_function(function_id);
        let original_params = original_function.params.clone();

        let (function_to_call, typechecked_arguments) = if let Some(type_params) =
            &original_function.type_params
        {
            let intrinsic_type = original_function.intrinsic_type;

            // We infer the type arguments, or evaluate them if the user has supplied them
            let type_args = match known_type_args {
                Some(ta) => {
                    // Need the ident
                    ta.into_iter()
                        .enumerate()
                        .map(|(idx, type_id)| TypeParam { ident: type_params[idx].ident, type_id })
                        .collect()
                }
                None => {
                    self.infer_call_type_args(fn_call, function_id, this_expr.as_ref(), scope_id)?
                }
            };

            // We skip specialization if any of the type arguments are type variables: `any_type_vars`
            // because we could just be evaluating a generic function that calls another generic function,
            // in which case we don't want to generate a 'specialized' function where we haven't
            // actually specialized everything
            let mut fully_concrete = true;
            for TypeParam { type_id, .. } in type_args.iter() {
                if self.is_generic(*type_id) {
                    fully_concrete = false
                }
            }
            if !fully_concrete {
                (
                    function_id,
                    self.typecheck_call_arguments(
                        fn_call,
                        this_expr,
                        &original_params,
                        scope_id,
                        false,
                    )?,
                )
            } else {
                self.get_specialized_function_for_call(
                    fn_call,
                    type_args,
                    function_id,
                    intrinsic_type,
                    this_expr,
                    scope_id,
                )?
            }
        } else {
            (
                function_id,
                self.typecheck_call_arguments(
                    fn_call,
                    this_expr,
                    &original_params,
                    scope_id,
                    false,
                )?,
            )
        };

        let function_ret_type = self.get_function(function_to_call).ret_type;
        let call = Call {
            callee_function_id: function_to_call,
            args: typechecked_arguments,
            ret_type: function_ret_type,
            span: fn_call.span,
        };
        Ok(TypedExpr::FunctionCall(call))
    }

    fn infer_call_type_args(
        &mut self,
        fn_call: &FnCall,
        generic_function_id: FunctionId,
        this_expr: Option<&TypedExpr>,
        calling_scope: ScopeId,
    ) -> TyperResult<Vec<TypeParam>> {
        let generic_function = self.get_function(generic_function_id);
        let generic_type_params = generic_function
            .type_params
            .as_ref()
            .cloned()
            .expect("expected function to be generic");
        let generic_name = generic_function.name;
        let generic_params = generic_function.params.clone();
        let type_params = match &fn_call.type_args {
            Some(type_args) => {
                if type_args.len() != generic_type_params.len() {
                    return make_fail(
                        format!(
                            "Expected {} type arguments but got {}",
                            generic_type_params.len(),
                            type_args.len()
                        ),
                        fn_call.span,
                    );
                }
                let mut checked_params = Vec::new();
                for (idx, type_arg) in type_args.iter().enumerate() {
                    let param = &generic_type_params[idx];
                    // TODO: This is where we'd apply constraints
                    let type_id = self.eval_type_expr(&type_arg.type_expr, calling_scope)?;
                    checked_params.push(TypeParam { ident: param.ident, type_id });
                }
                checked_params
            }
            None => {
                let exprs = self.typecheck_call_arguments(
                    fn_call,
                    this_expr.cloned(),
                    &generic_params,
                    calling_scope,
                    true,
                )?;

                let mut solved_params: Vec<TypeParam> = Vec::new();

                for (idx, expr) in exprs.iter().enumerate() {
                    let param = &generic_params[idx];

                    // This 'match' is where we would ideally implement some actual
                    // recursive type unification algorithm rather than hardcode 2 cases
                    let maybe_solved_param = match self.get_type(param.type_id) {
                        Type::TypeVariable(tv) => {
                            // If the type param is used in the type of the argument, we can infer
                            // the type param from the type of the argument
                            Some(TypeParam { ident: tv.identifier_id, type_id: expr.get_type() })
                        }
                        Type::Array(array_type) => {
                            // If the type param is used in the element type of an array, we can infer
                            // the type param from the type of the array
                            if let Type::TypeVariable(tv) = self.get_type(array_type.element_type) {
                                if let Type::Array(at) = self.get_type(expr.get_type()) {
                                    Some(TypeParam {
                                        ident: tv.identifier_id,
                                        type_id: at.element_type,
                                    })
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(solved_param) = maybe_solved_param {
                        let existing_solution =
                            solved_params.iter().find(|p| p.ident == solved_param.ident);
                        if let Some(existing_solution) = existing_solution {
                            if let Err(msg) = self
                                .typecheck_types(existing_solution.type_id, solved_param.type_id)
                            {
                                return make_fail(
                                    format!(
                                        "Conflicting type parameters for type param {} in call to {}: {}",
                                        &*self.get_ident_str(solved_param.ident),
                                        &*self.get_ident_str(generic_name),
                                        msg
                                    ),
                                    fn_call.span,
                                );
                            }
                        }
                        solved_params.push(solved_param);
                    }
                }
                if solved_params.len() < generic_type_params.len() {
                    return make_fail(
                        format!(
                            "Could not infer all type parameters for function call to {}",
                            &*self.get_ident_str(generic_name)
                        ),
                        fn_call.span,
                    );
                } else {
                    debug!("Solved params: {:?}", solved_params);
                    solved_params
                }
            }
        };
        Ok(type_params)
    }

    fn get_specialized_function_for_call(
        &mut self,
        fn_call: &FnCall,
        inferred_or_passed_type_args: Vec<TypeParam>,
        generic_function_id: FunctionId,
        intrinsic_type: Option<IntrinsicFunctionType>,
        this_expr: Option<TypedExpr>,
        calling_scope: ScopeId,
    ) -> TyperResult<(FunctionId, Vec<TypedExpr>)> {
        let spec_fn_scope_id = self.scopes.add_scope_to_root();
        let generic_function = self.get_function(generic_function_id);
        let generic_function_ast_id = generic_function.ast_id;
        let specializations = generic_function.specializations.clone();
        let name = String::from(&*self.get_ident_str(generic_function.name));
        let mut new_name = name.clone();
        // Add type_args to scope and typecheck them against the actual params
        for type_param in inferred_or_passed_type_args.iter() {
            if let Type::TypeVariable(tv) = self.get_type(type_param.type_id) {
                return make_fail(
                    format!(
                        "Cannot specialize function with type variable: {}",
                        &*self.get_ident_str(tv.identifier_id)
                    ),
                    fn_call.span,
                );
            };
            debug!(
                "Adding type param {}: {} to scope for specialized function {}",
                &*self.get_ident_str(type_param.ident),
                self.type_id_to_string(type_param.type_id),
                name
            );
            self.scopes
                .get_scope_mut(spec_fn_scope_id)
                .add_type(type_param.ident, type_param.type_id);
        }
        let type_ids = inferred_or_passed_type_args
            .iter()
            .map(|type_param| type_param.type_id)
            .collect::<Vec<_>>();
        new_name.push_str("_");
        new_name.push_str(
            &type_ids.iter().map(|type_id| type_id.to_string()).collect::<Vec<_>>().join("_"),
        );

        for (i, existing_specialization) in specializations.iter().enumerate() {
            // For now, naive comparison that all type ids are identical
            // There may be some scenarios where they are _equivalent_ but not identical
            // But I'm not sure
            let types_stringified = existing_specialization
                .specialized_type_params
                .iter()
                .map(|type_id| self.type_id_to_string(*type_id))
                .collect::<Vec<_>>()
                .join("_");
            debug!("existing specialization for {} {}: {}", name, i, types_stringified);
            if existing_specialization.specialized_type_params == type_ids {
                debug!(
                    "Found existing specialization for function {} with types: {}",
                    name, types_stringified
                );
                let exprs = self.typecheck_call_arguments(
                    fn_call,
                    this_expr,
                    &existing_specialization.specialized_params,
                    calling_scope,
                    false,
                )?;
                return Ok((existing_specialization.specialized_function_id, exprs));
            }
        }

        let ast = self.ast.clone();
        let Definition::FnDef(ast_def) = ast.get_defn(generic_function_ast_id) else {
            self.internal_compiler_error(
                "failed to get AST node for function specialization",
                fn_call.span,
            )
        };
        let specialized_function_id = self.eval_function(
            ast_def,
            self.scopes.get_root_scope_id(),
            Some(spec_fn_scope_id),
            true,
            Some(self.ast.ident_id(&new_name)),
            intrinsic_type,
        )?;
        let specialized_params = self.get_function(specialized_function_id).params.clone();
        self.get_function_mut(generic_function_id).specializations.push(SpecializationRecord {
            specialized_function_id,
            specialized_type_params: type_ids,
            specialized_params: specialized_params.clone(),
        });

        let typechecked_exprs = self.typecheck_call_arguments(
            fn_call,
            this_expr,
            &specialized_params,
            calling_scope,
            false,
        )?;
        Ok((specialized_function_id, typechecked_exprs))
    }

    fn eval_block_stmt(
        &mut self,
        stmt: &BlockStmt,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedStmt> {
        match stmt {
            BlockStmt::ValDef(val_def) => {
                let provided_type = match val_def.type_id.as_ref() {
                    None => None,
                    Some(type_expr) => Some(self.eval_type_expr(type_expr, scope_id)?),
                };
                let value_expr = self.eval_expr(&val_def.value, scope_id, provided_type)?;
                let actual_type = value_expr.get_type();
                let variable_type = if let Some(expected_type) = provided_type {
                    if let Err(msg) = self.typecheck_types(expected_type, actual_type) {
                        return make_fail(
                            format!("Local variable type mismatch: {}", msg),
                            val_def.span,
                        );
                    }
                    expected_type
                } else {
                    actual_type
                };

                let variable_id = self.add_variable(Variable {
                    is_mutable: val_def.is_mutable,
                    name: val_def.name,
                    type_id: variable_type,
                    owner_scope: Some(scope_id),
                });
                let val_def_stmt = TypedStmt::ValDef(Box::new(ValDef {
                    ty: variable_type,
                    variable_id,
                    initializer: value_expr,
                    span: val_def.span,
                }));
                self.scopes.add_variable(scope_id, val_def.name, variable_id);
                Ok(val_def_stmt)
            }
            BlockStmt::Assignment(assignment) => {
                // let lhs = self.eval_expr(&assignment.lhs, scope_id, None)?;
                let lhs = self.eval_assigment_lhs_expr(&assignment.lhs, scope_id, None)?;
                let rhs = self.eval_expr(&assignment.rhs, scope_id, Some(lhs.get_type()))?;
                if let Err(msg) = self.typecheck_types(lhs.get_type(), rhs.get_type()) {
                    return make_fail(
                        format!("Invalid types for assignment: {}", msg),
                        assignment.span,
                    );
                }
                let expr = TypedStmt::Assignment(Box::new(Assignment {
                    destination: Box::new(lhs),
                    value: Box::new(rhs),
                    span: assignment.span,
                }));
                Ok(expr)
            }
            BlockStmt::LoneExpression(expression) => {
                let expr = self.eval_expr(expression, scope_id, expected_type)?;
                Ok(TypedStmt::Expr(Box::new(expr)))
            }
            BlockStmt::While(while_stmt) => {
                let cond = self.eval_expr(&while_stmt.cond, scope_id, Some(BOOL_TYPE_ID))?;
                if let Err(e) = self.typecheck_types(BOOL_TYPE_ID, cond.get_type()) {
                    return make_fail(
                        format!("Invalid while condition type: {}", e),
                        cond.get_span(),
                    );
                }
                let block = self.eval_block(&while_stmt.block, scope_id, None)?;
                Ok(TypedStmt::WhileLoop(Box::new(TypedWhileLoop {
                    cond,
                    block,
                    span: while_stmt.span,
                })))
            }
        }
    }
    fn eval_block(
        &mut self,
        block: &Block,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedBlock> {
        let mut statements = Vec::with_capacity(block.stmts.len());
        let mut expr_type_of_block: TypeId = UNIT_TYPE_ID;
        for (index, stmt) in block.stmts.iter().enumerate() {
            let is_last = index == block.stmts.len() - 1;
            let expected_type = if is_last { expected_type } else { None };
            let stmt = self.eval_block_stmt(stmt, scope_id, expected_type)?;
            if is_last {
                expr_type_of_block = self.get_stmt_expression_type(&stmt);
            }
            statements.push(stmt);
        }

        let typed_block =
            TypedBlock { expr_type: expr_type_of_block, scope_id, statements, span: block.span };
        Ok(typed_block)
    }

    fn _get_scope_for_namespace(&self, namespace_ident: IdentifierId) -> ScopeId {
        self.namespaces.iter().find(|ns| ns.name == namespace_ident).unwrap().scope_id
    }

    fn resolve_intrinsic_function_type(
        &self,
        fn_def: &FnDef,
        scope_id: ScopeId,
    ) -> IntrinsicFunctionType {
        trace!("resolve_intrinsic_function_type for {}", &*self.get_ident_str(fn_def.name));
        let Some(current_namespace) = self.namespaces.iter().find(|ns| ns.scope_id == scope_id)
        else {
            println!("{:?}", fn_def);
            panic!(
                "Functions must be defined within a namespace scope: {:?}",
                &*self.get_ident_str(fn_def.name)
            )
        };
        let result = if current_namespace.name == self.ast.ident_id("string") {
            if fn_def.name == self.ast.ident_id("length") {
                Some(IntrinsicFunctionType::StringLength)
            } else if fn_def.name == self.ast.ident_id("fromChars") {
                Some(IntrinsicFunctionType::StringFromCharArray)
            } else if fn_def.name == self.ast.ident_id("equals") {
                Some(IntrinsicFunctionType::StringEquals)
            } else {
                None
            }
        } else if current_namespace.name == self.ast.ident_id("Array") {
            if fn_def.name == self.ast.ident_id("length") {
                Some(IntrinsicFunctionType::ArrayLength)
            } else if fn_def.name == self.ast.ident_id("capacity") {
                Some(IntrinsicFunctionType::ArrayCapacity)
            } else if fn_def.name == self.ast.ident_id("grow") {
                Some(IntrinsicFunctionType::ArrayGrow)
            } else if fn_def.name == self.ast.ident_id("new") {
                Some(IntrinsicFunctionType::ArrayNew)
            } else if fn_def.name == self.ast.ident_id("set_length") {
                Some(IntrinsicFunctionType::ArraySetLength)
            } else {
                None
            }
        } else if current_namespace.name == self.ast.ident_id("char") {
            // Future Char intrinsics
            None
        } else if current_namespace.name == self.ast.ident_id("_root") {
            let function_name = &*self.get_ident_str(fn_def.name);
            IntrinsicFunctionType::from_function_name(function_name)
        } else {
            None
        };
        match result {
            Some(result) => result,
            None => panic!(
                "Could not resolve intrinsic function type for function: {} in namespace: {}",
                &*self.get_ident_str(fn_def.name),
                &*self.get_ident_str(current_namespace.name)
            ),
        }
    }

    fn eval_function(
        &mut self,
        fn_def: &FnDef,
        parent_scope_id: ScopeId,
        fn_scope_id: Option<ScopeId>,
        specialize: bool,
        new_name: Option<IdentifierId>,
        // Used only during specialization; we already know the intrinsic type
        // from the generic version so we just pass it in
        known_intrinsic: Option<IntrinsicFunctionType>,
    ) -> TyperResult<FunctionId> {
        let fn_scope_id = match fn_scope_id {
            None => self.scopes.add_child_scope(parent_scope_id),
            Some(fn_scope_id) => fn_scope_id,
        };

        // Instantiate type arguments
        let is_generic =
            !specialize && fn_def.type_args.as_ref().map(|args| !args.is_empty()).unwrap_or(false);
        trace!(
            "eval_function {} is_generic: {}, specialize: {} in scope: {}",
            &*self.get_ident_str(fn_def.name),
            is_generic,
            specialize,
            parent_scope_id
        );
        let mut type_params: Option<Vec<TypeParam>> = None;
        if is_generic {
            let mut the_type_params = Vec::new();
            for type_parameter in fn_def.type_args.as_ref().unwrap().iter() {
                let type_variable =
                    TypeVariable { identifier_id: type_parameter.ident, _constraints: None };
                let type_variable_id = self.add_type(Type::TypeVariable(type_variable));
                let fn_scope = self.scopes.get_scope_mut(fn_scope_id);
                let type_param =
                    TypeParam { ident: type_parameter.ident, type_id: type_variable_id };
                the_type_params.push(type_param);
                fn_scope.add_type(type_parameter.ident, type_variable_id)
            }
            type_params = Some(the_type_params);
            trace!(
                "Added type arguments to function {} scope {:?}",
                &*self.get_ident_str(fn_def.name),
                self.scopes.get_scope(fn_scope_id)
            );
        }

        // Typecheck arguments
        let mut params = Vec::new();
        for (idx, fn_arg) in fn_def.args.iter().enumerate() {
            let type_id = self.eval_type_expr(&fn_arg.ty, fn_scope_id)?;
            if specialize {
                trace!(
                    "Specializing argument: {} got {}",
                    &*self.get_ident_str(fn_arg.name),
                    self.type_id_to_string(type_id)
                );
            }
            let variable = Variable {
                name: fn_arg.name,
                type_id,
                is_mutable: false,
                owner_scope: Some(fn_scope_id),
            };
            let variable_id = self.add_variable(variable);
            params.push(FnArgDefn {
                name: fn_arg.name,
                variable_id,
                position: idx as u32,
                type_id,
                span: fn_arg.span,
            });
            self.scopes.add_variable(fn_scope_id, fn_arg.name, variable_id);
        }

        let intrinsic_type = if specialize && known_intrinsic.is_some() {
            known_intrinsic
        } else if fn_def.linkage == Linkage::Intrinsic {
            Some(self.resolve_intrinsic_function_type(fn_def, parent_scope_id))
        } else {
            None
        };
        let given_ret_type = match &fn_def.ret_type {
            None => UNIT_TYPE_ID,
            Some(type_expr) => self.eval_type_expr(type_expr, fn_scope_id)?,
        };
        let function = Function {
            name: new_name.unwrap_or(fn_def.name),
            scope: fn_scope_id,
            ret_type: given_ret_type,
            params,
            type_params,
            block: None,
            intrinsic_type,
            linkage: fn_def.linkage,
            specializations: Vec::new(),
            ast_id: fn_def.ast_id,
            span: fn_def.span,
        };
        let is_extern = function.linkage == Linkage::External;
        let function_id = self.add_function(function);

        // We do not want to resolve specialized functions by name!
        // So don't add them to any scope.
        // They all have the same name but different types!!!
        if !specialize {
            self.scopes.add_function(parent_scope_id, fn_def.name, function_id);
        }
        let is_intrinsic = intrinsic_type.is_some();
        let body_block = match &fn_def.block {
            Some(block_ast) => {
                let block = self.eval_block(block_ast, fn_scope_id, Some(given_ret_type))?;
                if let Err(msg) = self.typecheck_types(given_ret_type, block.expr_type) {
                    return make_fail(
                        format!(
                            "Function {} return type mismatch: {}",
                            &*self.get_ident_str(fn_def.name),
                            msg
                        ),
                        fn_def.span,
                    );
                } else {
                    Some(block)
                }
            }
            None if is_intrinsic || is_extern => None,
            None => return make_fail("function is missing implementation", fn_def.span),
        };
        // Add the body now
        self.get_function_mut(function_id).block = body_block;
        Ok(function_id)
    }
    fn eval_namespace(
        &mut self,
        ast_namespace: &ParsedNamespace,
        scope_id: ScopeId,
    ) -> TyperResult<NamespaceId> {
        // We add the new namespace's scope as a child of the current scope
        let ns_scope_id = self.scopes.add_child_scope(scope_id);
        let namespace = Namespace { name: ast_namespace.name, scope_id: ns_scope_id };
        let namespace_id = self.add_namespace(namespace);
        // We add the new namespace to the current scope
        let scope = self.scopes.get_scope_mut(scope_id);
        scope.add_namespace(ast_namespace.name, namespace_id);
        for defn in &ast_namespace.definitions {
            if let Definition::FnDef(fn_def) = defn {
                self.eval_function(fn_def, ns_scope_id, None, false, None, None)?;
            } else if let Definition::Namespace(ns) = defn {
                self.eval_namespace(ns, ns_scope_id)?;
            } else {
                panic!("Unsupported definition type inside namespace: {:?}", defn)
            }
        }
        Ok(namespace_id)
    }
    fn eval_definition(&mut self, def: &Definition, scope_id: ScopeId) -> TyperResult<()> {
        match def {
            Definition::Namespace(namespace) => {
                self.eval_namespace(namespace, scope_id)?;
                Ok(())
            }
            Definition::Const(const_val) => {
                let _variable_id: VariableId = self.eval_const(const_val)?;
                Ok(())
            }
            Definition::FnDef(fn_def) => {
                self.eval_function(fn_def, scope_id, None, false, None, None)?;
                Ok(())
            }
            Definition::TypeDef(type_defn) => {
                self.eval_type_defn(type_defn, scope_id)?;
                let _typ = self.eval_type_expr(&type_defn.value_expr, scope_id)?;
                Ok(())
            }
        }
    }
    pub fn run(&mut self) -> anyhow::Result<()> {
        let mut errors: Vec<TyperError> = Vec::new();
        // TODO: 'Declare' everything first, will allow modules
        //        to declare their API without full typechecking
        //        will also allow recursion without hacks

        let scope_id = self.scopes.get_root_scope_id();
        for defn in self.ast.clone().defns_iter() {
            let result = self.eval_definition(defn, scope_id);
            if let Err(e) = result {
                self.print_error(&e.message, e.span);
                errors.push(e);
            }
        }
        if !errors.is_empty() {
            if log::log_enabled!(Level::Debug) {
                debug!("{}", self);
            }
            bail!("{} failed typechecking with {} errors", self.ast.source.filename, errors.len())
        }
        Ok(())
    }
}

// lower impl
impl TypedModule {
    // pub fn lower(&mut self) -> ParseResult<()> {
    //     // Question: do we only need to lower concrete code?
    //     for function_id in 0..self.function_iter() {
    //         self.lower_function(function_id)?;
    //     }
    //     Ok(())
    // }
    //
    // fn lower_function(&mut self, function_id: FunctionId) -> TyperResult<()> {
    //     let function = self.get_function(function_id);
    //
    //     // Lower the block of the function
    //     let Some(block) = &function.block else {
    //         return Ok(());
    //     };
    //     if let Some(lowered_block) = self.lower_block(block)? {
    //         let function = self.get_function_mut(function_id);
    //         function.block = Some(lowered_block);
    //     };
    //
    //     // Maybe lower other things one day?
    //
    //     Ok(())
    // }
    //
    // // If lowering is not a no-op, returns Some with the lowered block
    // // This is to avoid unnecessary cloning
    // fn lower_block(&mut self, block: &mut TypedBlock) -> TyperResult<()> {
    //     let mut new_stmts: Vec<TypedStmt> = vec![];
    //     for stmt in &block.statements {
    //         if let Some(lowered_stmts) = self.lower_stmt(stmt)? {
    //             updates.push()
    //             let block = self.get_block_mut(block);
    //             block.statements(lowered_stmt);
    //         };
    //     }
    //     Ok(())
    // }
    //
    // fn lower_stmt(&mut self, stmt: &TypedStmt) -> TyperResult<Vec<TypedStmt>> {
    //     match stmt {
    //         TypedStmt::Expr(expr) => {
    //             self.lower_expr(expr)?;
    //         }
    //         TypedStmt::ValDef(val_def) => {
    //             self.lower_expr(&mut val_def.initializer)?;
    //         }
    //         TypedStmt::Assignment(assignment) => {
    //             self.lower_expr(&mut assignment.destination)?;
    //             self.lower_expr(&mut assignment.value)?;
    //         }
    //         TypedStmt::WhileLoop(while_loop) => {
    //             self.lower_expr(&mut while_loop.cond)?;
    //             self.lower_block(&mut while_loop.block)?;
    //         }
    //     }
    // }
}

impl Display for TypedModule {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("Module ")?;
        f.write_str(&self.ast.name)?;
        f.write_str("\n")?;
        f.write_str("--- TYPES ---\n")?;
        for (id, ty) in self.types.iter().enumerate() {
            f.write_fmt(format_args!("{} ", id))?;
            self.write_type(ty, f)?;
            f.write_str("\n")?;
        }
        f.write_str("--- Namespaces ---\n")?;
        for (id, namespace) in self.namespaces.iter().enumerate() {
            f.write_fmt(format_args!("{} ", id))?;
            f.write_str(&self.get_ident_str(namespace.name))?;
            f.write_str("\n")?;
        }
        f.write_str("--- Variables ---\n")?;
        for (id, variable) in self.variables.iter().enumerate() {
            f.write_fmt(format_args!("{id:02} "))?;
            self.display_variable(variable, f)?;
            f.write_str("\n")?;
        }
        f.write_str("--- Functions ---\n")?;
        for (_, func) in self.functions.iter().enumerate() {
            self.display_function(func, f, false)?;
            f.write_str("\n")?;
        }
        Ok(())
    }
}

// Dumping
impl TypedModule {
    fn display_variable(&self, var: &Variable, writ: &mut impl Write) -> std::fmt::Result {
        if var.is_mutable {
            writ.write_str("mut ")?;
        }
        writ.write_str(&self.get_ident_str(var.name))?;
        writ.write_str(": ")?;
        self.display_type_id(var.type_id, writ)
    }

    fn display_type_id(&self, ty: TypeId, writ: &mut impl Write) -> std::fmt::Result {
        match ty {
            UNIT_TYPE_ID => writ.write_str("()"),
            CHAR_TYPE_ID => writ.write_str("char"),
            INT_TYPE_ID => writ.write_str("int"),
            BOOL_TYPE_ID => writ.write_str("bool"),
            STRING_TYPE_ID => writ.write_str("string"),
            type_id => {
                let ty = self.get_type(type_id);
                self.write_type(ty, writ)
            }
        }
    }

    pub fn type_id_to_string(&self, type_id: TypeId) -> String {
        let ty = self.get_type(type_id);
        self.type_to_string(ty)
    }

    pub fn type_to_string(&self, ty: &Type) -> String {
        let mut s = String::new();
        self.write_type(ty, &mut s).unwrap();
        s
    }

    fn write_type(&self, ty: &Type, writ: &mut impl Write) -> std::fmt::Result {
        match ty {
            Type::Unit => writ.write_str("()"),
            Type::Char => writ.write_str("char"),
            Type::Int => writ.write_str("int"),
            Type::Bool => writ.write_str("bool"),
            Type::String => writ.write_str("string"),
            Type::Record(record) => {
                writ.write_str("{")?;
                for (index, field) in record.fields.iter().enumerate() {
                    if index > 0 {
                        writ.write_str(", ")?;
                    }
                    writ.write_str(&self.get_ident_str(field.name))?;
                    writ.write_str(": ")?;
                    self.display_type_id(field.type_id, writ)?;
                }
                writ.write_str("}")
            }
            Type::Array(array) => {
                writ.write_str("Array<")?;
                self.display_type_id(array.element_type, writ)?;
                writ.write_str(">")
            }
            Type::TypeVariable(tv) => {
                writ.write_str("$")?;
                writ.write_str(&self.get_ident_str(tv.identifier_id))
            }
            Type::Optional(opt) => {
                self.display_type_id(opt.inner_type, writ)?;
                writ.write_char('?')
            }
            Type::Reference(r) => {
                self.display_type_id(r.inner_type, writ)?;
                writ.write_char('*')
            }
        }
    }

    pub fn display_function(
        &self,
        function: &Function,
        writ: &mut impl Write,
        display_block: bool,
    ) -> std::fmt::Result {
        if function.linkage == Linkage::External {
            writ.write_str("extern ")?;
        }
        if function.linkage == Linkage::Intrinsic {
            writ.write_str("intern ")?;
        }

        writ.write_str("fn ")?;
        writ.write_str(&self.get_ident_str(function.name))?;
        writ.write_str("(")?;
        for (idx, param) in function.params.iter().enumerate() {
            if idx > 0 {
                writ.write_str(", ")?;
            }
            writ.write_str(&self.get_ident_str(param.name))?;
            writ.write_str(": ")?;
            self.display_type_id(param.type_id, writ)?;
        }
        writ.write_str(")")?;
        writ.write_str(": ")?;
        self.display_type_id(function.ret_type, writ)?;
        if display_block {
            if let Some(block) = &function.block {
                self.display_block(block, writ, 0)?;
            }
        }
        Ok(())
    }

    fn display_block(
        &self,
        block: &TypedBlock,
        writ: &mut impl Write,
        indentation: usize,
    ) -> std::fmt::Result {
        writ.write_str("{\n")?;
        for (idx, stmt) in block.statements.iter().enumerate() {
            self.display_stmt(stmt, writ, indentation + 1)?;
            if idx < block.statements.len() - 1 {
                writ.write_str(";")?;
            }
            writ.write_str("\n")?;
        }
        writ.write_str(&" ".repeat(indentation))?;
        writ.write_str("}")
    }

    fn display_stmt(
        &self,
        stmt: &TypedStmt,
        writ: &mut impl Write,
        indentation: usize,
    ) -> std::fmt::Result {
        writ.write_str(&" ".repeat(indentation))?;
        match stmt {
            TypedStmt::Expr(expr) => self.display_expr(expr, writ, indentation),
            TypedStmt::ValDef(val_def) => {
                writ.write_str("val ")?;
                self.display_variable(self.get_variable(val_def.variable_id), writ)?;
                writ.write_str(" = ")?;
                self.display_expr(&val_def.initializer, writ, indentation)
            }
            TypedStmt::Assignment(assignment) => {
                self.display_expr(&assignment.destination, writ, 0)?;
                writ.write_str(" = ")?;
                self.display_expr(&assignment.value, writ, 0)
            }
            TypedStmt::WhileLoop(while_loop) => {
                writ.write_str("while ")?;
                self.display_expr(&while_loop.cond, writ, 0)?;
                writ.write_str(" ")?;
                self.display_block(&while_loop.block, writ, indentation + 1)
            }
        }
    }

    pub fn expr_to_string(&self, expr: &TypedExpr) -> String {
        let mut s = String::new();
        self.display_expr(expr, &mut s, 0).unwrap();
        s
    }

    pub fn display_expr(
        &self,
        expr: &TypedExpr,
        writ: &mut impl Write,
        indentation: usize,
    ) -> std::fmt::Result {
        match expr {
            TypedExpr::Unit(_) => writ.write_str("()"),
            TypedExpr::Char(c, _) => writ.write_fmt(format_args!("'{}'", c)),
            TypedExpr::Int(i, _) => writ.write_fmt(format_args!("{}", i)),
            TypedExpr::Bool(b, _) => writ.write_fmt(format_args!("{}", b)),
            TypedExpr::Str(s, _) => writ.write_fmt(format_args!("\"{}\"", s)),
            TypedExpr::None(typ, _) => {
                writ.write_str("None<")?;
                self.display_type_id(*typ, writ)?;
                writ.write_str(">")
            }
            TypedExpr::Array(array) => {
                writ.write_str("[")?;
                for (idx, expr) in array.elements.iter().enumerate() {
                    if idx > 0 {
                        writ.write_str(", ")?;
                    }
                    self.display_expr(expr, writ, indentation)?;
                }
                writ.write_str("]")
            }
            TypedExpr::Record(record) => {
                writ.write_str("{")?;
                for (idx, field) in record.fields.iter().enumerate() {
                    if idx > 0 {
                        writ.write_str(",\n")?;
                        writ.write_str(&" ".repeat(indentation + 1))?;
                    }
                    writ.write_str(&self.get_ident_str(field.name))?;
                    writ.write_str(": ")?;
                    self.display_expr(&field.expr, writ, indentation)?;
                }
                writ.write_str(&" ".repeat(indentation))?;
                writ.write_str("}")
            }
            TypedExpr::Variable(v) => {
                let variable = self.get_variable(v.variable_id);
                writ.write_str(&self.get_ident_str(variable.name))
            }
            TypedExpr::RecordFieldAccess(field_access) => {
                self.display_expr(&field_access.base, writ, indentation)?;
                writ.write_str(".")?;
                writ.write_str(&self.get_ident_str(field_access.target_field))
            }
            TypedExpr::ArrayIndex(array_index) => {
                self.display_expr(&array_index.base_expr, writ, indentation)?;
                writ.write_str("[")?;
                self.display_expr(&array_index.index_expr, writ, indentation)?;
                writ.write_str("]")
            }
            TypedExpr::StringIndex(string_index) => {
                self.display_expr(&string_index.base_expr, writ, indentation)?;
                writ.write_str("[")?;
                self.display_expr(&string_index.index_expr, writ, indentation)?;
                writ.write_str("]")
            }
            TypedExpr::FunctionCall(fn_call) => {
                let function = self.get_function(fn_call.callee_function_id);
                writ.write_str(&self.get_ident_str(function.name))?;
                writ.write_str("(")?;
                for (idx, arg) in fn_call.args.iter().enumerate() {
                    if idx > 0 {
                        writ.write_str(", ")?;
                    }
                    self.display_expr(arg, writ, indentation)?;
                }
                writ.write_str(")")
            }
            TypedExpr::Block(block) => self.display_block(block, writ, indentation),
            TypedExpr::If(if_expr) => {
                writ.write_str("if ")?;
                self.display_expr(&if_expr.condition, writ, 0)?;
                writ.write_str(" ")?;
                self.display_block(&if_expr.consequent, writ, 0)?;
                writ.write_str(" else ")?;
                self.display_block(&if_expr.alternate, writ, 0)?;
                Ok(())
            }
            TypedExpr::UnaryOp(unary_op) => {
                writ.write_fmt(format_args!("{}", unary_op.kind))?;
                self.display_expr(&unary_op.expr, writ, 0)
            }
            TypedExpr::BinaryOp(binary_op) => {
                self.display_expr(&binary_op.lhs, writ, 0)?;
                writ.write_fmt(format_args!(" {} ", binary_op.kind))?;
                self.display_expr(&binary_op.rhs, writ, 0)
            }
            TypedExpr::OptionalSome(opt) => {
                writ.write_str("Some(")?;
                self.display_expr(&opt.inner_expr, writ, 0)?;
                writ.write_str(")")
            }
            TypedExpr::OptionalHasValue(opt) => {
                self.display_expr(&opt, writ, 0)?;
                writ.write_str(".hasValue()")
            }
            TypedExpr::OptionalGet(opt) => {
                self.display_expr(&opt.inner_expr, writ, 0)?;
                writ.write_str("!")
            }
            TypedExpr::For(for_expr) => {
                writ.write_str("for ")?;
                writ.write_str(&self.get_ident_str(for_expr.binding))?;
                writ.write_str(" in ")?;
                self.display_expr(&for_expr.iterable_expr, writ, 0)?;
                writ.write_str(" ")?;
                writ.write_str(match for_expr.for_expr_type {
                    ForExprType::Yield => "yield ",
                    ForExprType::Do => "do ",
                })?;
                self.display_block(&for_expr.body_block, writ, indentation + 1)
            }
        }
    }

    pub fn function_id_to_string(&self, function_id: FunctionId, display_block: bool) -> String {
        let func = self.get_function(function_id);
        let mut s = String::new();
        self.display_function(func, &mut s, display_block).unwrap();
        s
    }
}

#[cfg(test)]
mod test {
    use crate::parse::{parse_text, ParseResult};
    use crate::typer::*;

    fn setup(src: &str, test_name: &str) -> ParseResult<ParsedModule> {
        parse_text(src.to_string(), ".".to_string(), test_name.to_string(), false)
    }

    #[test]
    fn const_definition_1() -> anyhow::Result<()> {
        let src = r"val x: int = 420;";
        let module = setup(src, "const_definition_1.nx")?;
        let mut ir = TypedModule::new(Rc::new(module));
        ir.run()?;
        let i1 = &ir.constants[0];
        if let TypedExpr::Int(i, span) = i1.expr {
            assert_eq!(i, 420);
            assert_eq!(span.end, 16);
            assert_eq!(span.start, 0);
            Ok(())
        } else {
            panic!("{i1:?} was not an int")
        }
    }

    #[test]
    fn fn_definition_1() -> anyhow::Result<()> {
        let src = r#"
        fn foo(): int {
          1
        }
        fn basic(x: int, y: int): int {
          val x: int = 0; mut y: int = 1;
          y = { 1; 2; 3 };
          y = 42 + 42;
          foo()
        }"#;
        let module = setup(src, "basic_fn.nx")?;
        let mut ir = TypedModule::new(Rc::new(module));
        ir.run()?;
        println!("{:?}", ir.functions);
        Ok(())
    }
}
