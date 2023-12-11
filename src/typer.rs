#![allow(clippy::match_like_matches_macro)]

use crate::lex::{Span, TokenKind};
use crate::parse::{self, IfExpr, ParsedNamespace};
use crate::parse::{
    AstId, AstModule, Block, BlockStmt, Definition, Expression, FnCall, FnDef, IdentifierId,
    Literal,
};
use anyhow::{bail, Result};
use colored::Colorize;
use log::{error, trace, warn};
use std::collections::HashMap;
use std::error::Error;

use std::fmt::{Display, Formatter, Write};
use std::rc::Rc;

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
pub struct RecordDefnField {
    pub name: IdentifierId,
    pub type_id: TypeId,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct RecordDefn {
    pub fields: Vec<RecordDefnField>,
    pub name_if_named: Option<IdentifierId>,
    pub span: Span,
}

impl RecordDefn {
    pub fn find_field(&self, field_name: IdentifierId) -> Option<(usize, &RecordDefnField)> {
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
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeVariable {
    identifier_id: IdentifierId,
    /// This is where trait bounds would go
    constraints: Option<Vec<()>>,
}

#[derive(Debug, Clone)]
pub struct OptionalType {
    pub inner_type: TypeId,
}

#[derive(Debug, Clone)]
pub enum Type {
    Unit,
    Char,
    Int,
    Bool,
    String,
    Record(RecordDefn),
    Array(ArrayType),
    TypeVariable(TypeVariable),
    Optional(OptionalType),
}

impl Type {
    pub fn as_optional_type(&self) -> Option<&OptionalType> {
        match self {
            Type::Optional(opt) => Some(opt),
            _ => None,
        }
    }
    pub fn expect_optional_type(&self) -> &OptionalType {
        match self {
            Type::Optional(opt) => opt,
            _ => panic!("expect_optional called on: {:?}", self),
        }
    }
    pub fn expect_array_type(&self) -> &ArrayType {
        match self {
            Type::Array(array) => array,
            _ => panic!("expect_array called on: {:?}", self),
        }
    }
    pub fn expect_record_type(&self) -> &RecordDefn {
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
    pub type_params: Vec<TypeId>,
    pub specialized_function_id: FunctionId,
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
        }
    }
}

impl BinaryOpKind {
    pub fn precedence(&self) -> usize {
        use BinaryOpKind as B;
        match self {
            B::Multiply | B::Divide => 100,
            B::Add | B::Subtract => 90,
            B::Less | B::LessEqual | B::Greater | B::GreaterEqual | B::Equals => 80,
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
            B::Equals => true,
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
}

impl Display for UnaryOpKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UnaryOpKind::BooleanNegation => f.write_char('!'),
        }
    }
}

impl UnaryOpKind {}

#[derive(Debug, Clone)]
pub struct UnaryOp {
    pub kind: UnaryOpKind,
    pub ty: TypeId,
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
    FieldAccess(FieldAccess),
    BinaryOp(BinaryOp),
    UnaryOp(UnaryOp),
    Block(TypedBlock),
    FunctionCall(Call),
    If(Box<TypedIf>),
    ArrayIndex(IndexOp),
    StringIndex(IndexOp),
    OptionalSome(OptionalSome),
    OptionalHasValue(Box<TypedExpr>),
    OptionalGet(OptionalGet),
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
            TypedExpr::FieldAccess(field_access) => field_access.ty,
            TypedExpr::BinaryOp(binary_op) => binary_op.ty,
            TypedExpr::UnaryOp(unary_op) => unary_op.ty,
            TypedExpr::Block(b) => b.expr_type,
            TypedExpr::FunctionCall(call) => call.ret_type,
            TypedExpr::If(ir_if) => ir_if.ty,
            TypedExpr::ArrayIndex(op) => op.result_type,
            TypedExpr::StringIndex(op) => op.result_type,
            TypedExpr::OptionalSome(opt) => opt.type_id,
            TypedExpr::OptionalHasValue(_opt) => BOOL_TYPE_ID,
            TypedExpr::OptionalGet(opt_get) => opt_get.result_type_id,
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
            TypedExpr::FieldAccess(field_access) => field_access.span,
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

// TODO: When do we 'clone' a whole TypedStmt?
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

fn make_err<T: AsRef<str>>(s: T, span: Span) -> TyperError {
    TyperError::make(s.as_ref(), span)
}

fn make_fail<A, T: AsRef<str>>(s: T, span: Span) -> TyperResult<A> {
    Err(make_err(s, span))
}

pub struct TypedModule {
    pub ast: Rc<AstModule>,
    functions: Vec<Function>,
    pub variables: Vec<Variable>,
    pub types: Vec<Type>,
    pub constants: Vec<Constant>,
    pub scopes: Scopes,
    pub errors: Vec<TyperError>,
    pub namespaces: Vec<Namespace>,
}

impl TypedModule {
    pub fn new(parsed_module: Rc<AstModule>) -> TypedModule {
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

    fn report_error(&mut self, span: Span, message: String) {
        self.errors.push(TyperError { span, message })
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
    // fn is_generic(&self, ty: TypeId) -> bool {
    //     match ty {
    //         TypeId::TypeId(type_id) => match self.get_type(type_id) {
    //             Type::TypeVariable(_) => true,
    //             Type::Record(record) => record.fields.iter().any(|f| self.is_generic(f.ty)),
    //             Type::Array(arr) => self.is_generic(arr.element_type),
    //         },
    //         _ => false,
    //     }
    // }

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
        expr: &parse::TypeExpression,
        scope_id: ScopeId,
    ) -> TyperResult<TypeId> {
        let mut base = match expr {
            parse::TypeExpression::Unit(_) => Ok(UNIT_TYPE_ID),
            parse::TypeExpression::Char(_) => Ok(CHAR_TYPE_ID),
            parse::TypeExpression::Int(_) => Ok(INT_TYPE_ID),
            parse::TypeExpression::Bool(_) => Ok(BOOL_TYPE_ID),
            parse::TypeExpression::String(_) => Ok(STRING_TYPE_ID),
            parse::TypeExpression::Record(record_defn) => {
                let mut fields: Vec<RecordDefnField> = Vec::new();
                for (index, ast_field) in record_defn.fields.iter().enumerate() {
                    let ty = self.eval_type_expr(&ast_field.ty, scope_id)?;
                    fields.push(RecordDefnField { name: ast_field.name, type_id: ty, index })
                }
                let record_defn =
                    RecordDefn { fields, name_if_named: None, span: record_defn.span };
                let type_id = self.add_type(Type::Record(record_defn));
                Ok(type_id)
            }
            parse::TypeExpression::Name(ident, span) => {
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
            parse::TypeExpression::TypeApplication(ty_app) => {
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
            parse::TypeExpression::Optional(opt) => {
                let inner_ty = self.eval_type_expr(&opt.base, scope_id)?;
                let optional_type = Type::Optional(OptionalType { inner_type: inner_ty });
                let type_id = self.add_type(optional_type);
                Ok(type_id)
            }
        }?;
        // Attempt to fully resolve type variables before returning
        // loop {
        //     match self.get_type(base) {
        //         Type::TypeVariable(type_variable) => {
        //             let type_id = self.scopes.find_type(scope_id, type_variable.identifier_id);
        //             match type_id {
        //                 None => {
        //                     break;
        //                 }
        //                 Some(type_id) => {
        //                     trace!(
        //                         "eval_type_expr attempt resolve of TypeVariable {} got {:?}",
        //                         &*self.get_ident_str(type_variable.identifier_id),
        //                         self.type_id_to_string(type_id)
        //                     );
        //                     base = type_id;
        //                 }
        //             }
        //         }
        //         _other_type => break,
        //     }
        // }
        Ok(base)
    }

    fn eval_const_type_expr(&mut self, expr: &parse::TypeExpression) -> TyperResult<TypeId> {
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

    fn typecheck_record(&self, expected: &RecordDefn, actual: &RecordDefn) -> Result<(), String> {
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
        expected: &RecordDefn,
        actual: &RecordDefn,
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
        let expr = match &const_expr.value_expr {
            Expression::Literal(Literal::Numeric(n, span)) => {
                let num = self.parse_numeric(n).map_err(|msg| make_err(msg, *span))?;
                TypedExpr::Int(num, const_expr.span)
            }
            Expression::Literal(Literal::Bool(b, span)) => TypedExpr::Bool(*b, *span),
            Expression::Literal(Literal::Char(c, span)) => TypedExpr::Char(*c, *span),
            _other => {
                return make_fail(
                    "Only literals are currently supported as constants",
                    const_expr.span,
                )
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
        log::trace!(
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
        if let Some(expected_type_id) = expected_type {
            if let Type::Optional(optional_type) = self.get_type(expected_type_id) {
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
                    Err(msg) => make_fail(
                        format!("Typecheck failed when expecting optional: {}", msg),
                        expr.get_span(),
                    ),
                }
            } else {
                Ok(base_result)
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
        trace!(
            "eval_expr: {} expected type: {:?}",
            expr,
            expected_type.map(|t| self.type_id_to_string(t))
        );
        match expr {
            Expression::Array(array_expr) => {
                let mut element_type: Option<TypeId> = match expected_type {
                    Some(type_id) => match self.get_type(type_id) {
                        Type::Array(arr) => Ok(Some(arr.element_type)),
                        t => make_fail(format!("Expected {:?} but got Array", t), array_expr.span),
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
            Expression::IndexOperation(index_op) => {
                let index_expr =
                    self.eval_expr(&index_op.index_expr, scope_id, Some(INT_TYPE_ID))?;
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
            Expression::Record(ast_record) => {
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
                                )
                            }
                        },
                        other_ty => {
                            return make_fail(
                                format!(
                                    "Got record literal but expected {}",
                                    self.type_to_string(other_ty)
                                ),
                                ast_record.span,
                            )
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
                    field_defns.push(RecordDefnField {
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
                        let record_type = RecordDefn {
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
                            &RecordDefn {
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
            Expression::If(if_expr) => self.eval_if_expr(if_expr, scope_id),
            Expression::BinaryOp(binary_op) => {
                // Infer expected type to be type of operand1
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
            Expression::UnaryOp(op) => {
                let base_expr = self.eval_expr(&op.expr, scope_id, None)?;
                match op.op_kind {
                    UnaryOpKind::BooleanNegation => {
                        self.typecheck_types(BOOL_TYPE_ID, base_expr.get_type())
                            .map_err(|s| make_err(s, op.span))?;
                        Ok(TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::BooleanNegation,
                            ty: BOOL_TYPE_ID,
                            expr: Box::new(base_expr),
                            span: op.span,
                        }))
                    }
                }
            }
            Expression::Literal(Literal::Unit(span)) => Ok(TypedExpr::Unit(*span)),
            Expression::Literal(Literal::None(span)) => {
                // If we are expecting an Option, I need to reach inside it to get the inner type
                let expected_type = expected_type.ok_or(make_err(
                    "Cannot infer type of None literal without type hint",
                    *span,
                ))?;
                let expected_type =
                    self.get_type(expected_type).as_optional_type().ok_or(make_err(
                        format!(
                            "Expected optional type for None literal but got {:?}",
                            expected_type
                        ),
                        *span,
                    ))?;
                let inner_type = expected_type.inner_type;
                let none_type = Type::Optional(OptionalType { inner_type });
                // FIXME: We'll re-create the type for optional int, bool, etc over and over. Instead of add_type it should be
                //        self.get_or_add_type()
                let type_id = self.add_type(none_type);
                Ok(TypedExpr::None(type_id, *span))
            }
            Expression::Literal(Literal::Char(byte, span)) => Ok(TypedExpr::Char(*byte, *span)),
            Expression::Literal(Literal::Numeric(s, span)) => {
                let num = self.parse_numeric(s).map_err(|msg| make_err(msg, *span))?;
                Ok(TypedExpr::Int(num, *span))
            }
            Expression::Literal(Literal::Bool(b, span)) => {
                let expr = TypedExpr::Bool(*b, *span);
                Ok(expr)
            }
            Expression::Literal(Literal::String(s, span)) => {
                // So sad, we could just point to the source. BUT if we ever do escaping and things
                // then the source string is not the same as the string the user meant; perhaps here
                // is the place, or maybe in the parser, that we would actually do some work, which
                // would justify storing it separately. But then, post-transform, we should intern
                // these
                let expr = TypedExpr::Str(s.clone(), *span);
                Ok(expr)
            }
            Expression::Variable(variable) => {
                let variable_id =
                    self.scopes.find_variable(scope_id, variable.ident).ok_or(make_err(
                        format!("{} is not defined", &*self.get_ident_str(variable.ident)),
                        variable.span,
                    ))?;
                let v = self.get_variable(variable_id);
                let expr = TypedExpr::Variable(VariableExpr {
                    type_id: v.type_id,
                    variable_id,
                    span: variable.span,
                });
                trace!(
                    "variable {} had type {}",
                    &*self.get_ident_str(variable.ident),
                    self.type_id_to_string(v.type_id)
                );
                Ok(expr)
            }
            Expression::FieldAccess(field_access) => {
                let base_expr = self.eval_expr(&field_access.base, scope_id, None)?;
                let type_id = base_expr.get_type();
                let ret_type = match self.get_type(type_id) {
                    Type::Record(record_type) => {
                        let (_idx, target_field) =
                            record_type.find_field(field_access.target).ok_or(make_err(
                                format!(
                                    "Field {} not found on record type",
                                    &*self.get_ident_str(field_access.target)
                                ),
                                field_access.span,
                            ))?;
                        Ok(target_field.type_id)
                    }
                    ty => make_fail(
                        format!(
                            "Cannot access field {} on non-record type: {:?}",
                            &*self.get_ident_str(field_access.target),
                            self.type_to_string(ty)
                        ),
                        field_access.span,
                    ),
                }?;
                Ok(TypedExpr::FieldAccess(FieldAccess {
                    base: Box::new(base_expr),
                    target_field: field_access.target,
                    ty: ret_type,
                    span: field_access.span,
                }))
            }
            Expression::Block(block) => {
                let block = self.eval_block(block, scope_id)?;
                Ok(TypedExpr::Block(block))
            }
            Expression::MethodCall(m_call) => {
                let base_expr = self.eval_expr(&m_call.base, scope_id, None)?;
                if m_call.call.name == self.ast.ident_id("has_value") {
                    let Type::Optional(_opt) = self.get_type(base_expr.get_type()) else {
                        return make_fail(
                            format!(
                                "Cannot call has_value() on non-optional type: {}",
                                self.type_id_to_string(base_expr.get_type())
                            ),
                            m_call.call.span,
                        );
                    };
                    return Ok(TypedExpr::OptionalHasValue(Box::new(base_expr)));
                }
                let call = self.eval_function_call(&m_call.call, Some(base_expr), scope_id)?;
                Ok(call)
            }
            Expression::FnCall(fn_call) => {
                let call = self.eval_function_call(fn_call, None, scope_id)?;
                Ok(call)
            }
            Expression::OptionalGet(optional_get) => {
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
        }
    }

    fn eval_if_expr(&mut self, if_expr: &IfExpr, scope_id: ScopeId) -> TyperResult<TypedExpr> {
        // Ensure boolean condition (or optional which isn't built yet)
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
            let consequent_expr = self.eval_expr(&if_expr.cons, consequent_scope_id, None)?;
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
            let consequent_expr = self.eval_expr(&if_expr.cons, consequent_scope_id, None)?;
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

    fn eval_function_call(
        &mut self,
        fn_call: &FnCall,
        this_expr: Option<TypedExpr>,
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
        // This block is all about method or resolution
        // We are trying to find out if this method or function
        // exists, and returning its id if so
        let function_id = match this_expr.as_ref() {
            Some(base_expr) => {
                // Resolve a method call
                let type_id = base_expr.get_type();
                let function_id = match self.get_type(type_id) {
                    Type::String => {
                        // TODO: Abstract out a way to go from identifier to scope
                        //       (name -> ident id -> namespace id -> namespace -> scope id -> scope
                        let string_ident_id = self.ast.ident_id("string");
                        let string_namespace_id =
                            self.scopes.find_namespace(scope_id, string_ident_id).unwrap();
                        let string_namespace = self.get_namespace(string_namespace_id).unwrap();
                        let string_scope = self.scopes.get_scope(string_namespace.scope_id);
                        string_scope.find_function(fn_call.name)
                    }
                    Type::Char => {
                        let char_ident_id = self.ast.ident_id("char");
                        let char_namespace_id =
                            self.scopes.find_namespace(scope_id, char_ident_id).unwrap();
                        let char_namespace = self.get_namespace(char_namespace_id).unwrap();
                        let char_scope = self.scopes.get_scope(char_namespace.scope_id);
                        char_scope.find_function(fn_call.name)
                    }
                    Type::Array(_array_type) => {
                        let array_ident_id = self.ast.ident_id("Array");
                        let array_namespace_id =
                            self.scopes.find_namespace(scope_id, array_ident_id).unwrap();
                        let array_namespace = self.get_namespace(array_namespace_id).unwrap();
                        let array_scope = self.scopes.get_scope(array_namespace.scope_id);
                        array_scope.find_function(fn_call.name)
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
                        let record_namespace_id =
                            self.scopes.find_namespace(scope_id, record_type_name).unwrap();
                        let record_namespace = self.get_namespace(record_namespace_id).unwrap();
                        let record_scope = self.scopes.get_scope(record_namespace.scope_id);
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
                        )
                    }
                }
            }
            None => {
                // Resolve a non-method call
                let scope_to_search =
                    self.traverse_namespace_chain(scope_id, &fn_call.namespaces, fn_call.span)?;
                let function_id =
                    self.scopes.find_function(scope_to_search, fn_call.name).ok_or(make_err(
                        format!(
                            "Function not found: {} in scope: {:?}",
                            &*self.get_ident_str(fn_call.name),
                            self.scopes.get_scope(scope_id)
                        ),
                        fn_call.span,
                    ))?;
                function_id
            }
        };

        // Now that we have resolved to a function id, we need to specialize it if generic
        let original_function = self.get_function(function_id);
        // We should only specialize if we are in a concrete context, meaning
        // we have no unresolved type variables.
        // We could just be evaluating a generic function that calls another generic function,
        // in which case we don't want to generate a 'specialized' function where we haven't
        // actually specialized anything
        let function_to_call = if original_function.is_generic() {
            let intrinsic_type = original_function.intrinsic_type;
            let Some(type_args) = &fn_call.type_args else {
                return make_fail(
                    format!(
                        "Generic function {} must be called with type arguments",
                        &*self.get_ident_str(original_function.name)
                    ),
                    fn_call.span,
                );
            };
            // We skip specialization if any of the type arguments are type variables
            let mut any_type_vars = false;
            for type_arg in type_args.iter() {
                let type_id = self.eval_type_expr(&type_arg.value, scope_id)?;
                if let Type::TypeVariable(_tv) = self.get_type(type_id) {
                    any_type_vars = true;
                }
            }
            if any_type_vars {
                function_id
            } else {
                self.get_specialized_function_for_call(
                    fn_call,
                    function_id,
                    intrinsic_type,
                    scope_id,
                )?
            }
        } else {
            function_id
        };
        let mut final_args: Vec<TypedExpr> = Vec::new();
        let params_cloned = self.get_function(function_to_call).params.clone();
        // We have to deal with this outside of the loop because
        // we can't 'move' out of this_expr more than once
        let mut skip_first = false;
        if let Some(first) = params_cloned.get(0) {
            let is_self = first.name == self.ast.ident_id("self");
            if is_self {
                if let Some(this) = this_expr {
                    trace!("Method call; self = {}", self.expr_to_string(&this));
                    final_args.push(this);
                    skip_first = true;
                }
            }
        }
        let start: u32 = if skip_first { 1 } else { 0 };
        for fn_param in &params_cloned[start as usize..] {
            let matching_param_by_name =
                fn_call.args.iter().find(|arg| arg.name == Some(fn_param.name));
            // If we skipped 'self', we need to subtract 1 from the offset we index into fn_call.args with
            let matching_idx = fn_param.position - start;
            let matching_param = matching_param_by_name.or(fn_call.args.get(matching_idx as usize));
            if let Some(param) = matching_param {
                let expr = self.eval_expr(&param.value, scope_id, Some(fn_param.type_id))?;
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
                final_args.push(expr);
            } else {
                return make_fail(
                    format!(
                        "Could not find match for parameter {}",
                        &*self.get_ident_str(fn_param.name)
                    ),
                    fn_call.span,
                );
            }
        }
        let function_ret_type = self.get_function(function_to_call).ret_type;
        let call = Call {
            callee_function_id: function_to_call,
            args: final_args,
            ret_type: function_ret_type,
            span: fn_call.span,
        };
        Ok(TypedExpr::FunctionCall(call))
    }

    fn get_specialized_function_for_call(
        &mut self,
        fn_call: &FnCall,
        generic_function_id: FunctionId,
        intrinsic_type: Option<IntrinsicFunctionType>,
        calling_scope: ScopeId,
    ) -> TyperResult<FunctionId> {
        // TODO: Implement full generic type inference. This could get slow!
        //       Cases like [T](t: T) are easier but [T](x: ComplexType[A, B, T]) and solving for
        //       T in that case is hard. Requires recursive search.
        //       I wonder if we could infer in simple cases and refuse to infer
        //       in complex cases that would be slow.
        //       Inference algorithm:
        //       1. Find arguments that include a type param
        //       2. Find the actual value passed for each, find where the type variable appears within
        //          that type expression, and assign it to the concrete type

        // FIXME: Can we avoid this clone of the whole function
        let generic_function = self.get_function(generic_function_id).clone();
        trace!(
            "Specializing function call: {}, {}, astid {}",
            &*self.get_ident_str(fn_call.name),
            &*self.get_ident_str(generic_function.name),
            generic_function.ast_id
        );
        let type_params =
            generic_function.type_params.as_ref().expect("expected function to be generic");
        let type_args = fn_call
            .type_args
            .as_ref()
            .ok_or(make_err("fn call missing type args", fn_call.span))?;
        let mut new_name = self.get_ident_str(fn_call.name).to_string();

        // The specialized function lives in the root of the module because
        // we never look it up by name; we look up the generic version then use a cached
        // specialized function anyway

        // The only real difference is the scope: it has substitutions for the type variables
        let spec_fn_scope_id = self.scopes.add_scope_to_root();
        let evaluated_type_args = type_args
            .iter()
            .map(|type_arg| self.eval_type_expr(&type_arg.value, calling_scope))
            .collect::<TyperResult<Vec<TypeId>>>()?;
        for (i, existing_specialization) in generic_function.specializations.iter().enumerate() {
            // For now, naive comparison that all type ids are identical
            // There may be some scenarios where they are _equivalent_ but not identical
            // But I'm not sure
            let x = existing_specialization
                .type_params
                .iter()
                .map(|type_id| self.type_id_to_string(*type_id))
                .collect::<Vec<_>>()
                .join(",");
            warn!("existing specialization for {} {}: {}", new_name, i, x);
            if existing_specialization.type_params == evaluated_type_args {
                log::info!(
                    "Found existing specialization for function {} with types: {}",
                    &*self.get_ident_str(generic_function.name),
                    x
                );
                return Ok(existing_specialization.specialized_function_id);
            }
        }

        for (i, type_param) in type_params.iter().enumerate() {
            let type_arg = &type_args[i];
            let type_id = self.eval_type_expr(&type_arg.value, calling_scope)?;
            if let Type::TypeVariable(tv) = self.get_type(type_id) {
                return make_fail(
                    format!(
                        "Cannot specialize function with type variable: {}",
                        &*self.get_ident_str(tv.identifier_id)
                    ),
                    type_arg.value.get_span(),
                );
            };
            trace!(
                "Adding type param {} = {} to scope for specialized function {}",
                &*self.get_ident_str(type_param.ident),
                self.type_id_to_string(type_id),
                new_name
            );
            self.scopes.get_scope_mut(spec_fn_scope_id).add_type(type_param.ident, type_id);
        }

        let ast = self.ast.clone();
        let Definition::FnDef(ast_def) = ast.get_defn(generic_function.ast_id) else {
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
            intrinsic_type,
        )?;
        self.get_function_mut(generic_function_id).specializations.push(SpecializationRecord {
            specialized_function_id,
            type_params: evaluated_type_args,
        });
        Ok(specialized_function_id)
    }
    fn eval_block_stmt(&mut self, stmt: &BlockStmt, scope_id: ScopeId) -> TyperResult<TypedStmt> {
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
                let lhs = self.eval_expr(&assignment.lhs, scope_id, None)?;
                match &lhs {
                    TypedExpr::Variable(v) => {
                        let var = self.get_variable(v.variable_id);
                        if !var.is_mutable {
                            return make_fail(
                                "Cannot assign to immutable variable",
                                assignment.span,
                            );
                        }
                    }
                    TypedExpr::FieldAccess(_) => {
                        trace!("assignment to record member");
                    }
                    TypedExpr::ArrayIndex(_) => {
                        trace!("assignment to array index");
                    }
                    _ => {
                        return make_fail(
                            format!("Invalid assignment lhs: {:?}", lhs),
                            lhs.get_span(),
                        )
                    }
                };
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
                let expr = self.eval_expr(expression, scope_id, None)?;
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
                let block = self.eval_block(&while_stmt.block, scope_id)?;
                Ok(TypedStmt::WhileLoop(Box::new(TypedWhileLoop {
                    cond,
                    block,
                    span: while_stmt.span,
                })))
            }
        }
    }
    fn eval_block(&mut self, block: &Block, scope_id: ScopeId) -> TyperResult<TypedBlock> {
        let mut statements = Vec::new();
        for stmt in &block.stmts {
            let stmt = self.eval_block_stmt(stmt, scope_id)?;
            statements.push(stmt);
        }

        let expr_type = if let Some(stmt) = statements.last() {
            self.get_stmt_expression_type(stmt)
        } else {
            UNIT_TYPE_ID
        };

        let ir_block = TypedBlock { expr_type, scope_id: 0, statements, span: block.span };
        Ok(ir_block)
    }

    fn get_scope_for_namespace(&self, namespace_ident: IdentifierId) -> ScopeId {
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
        // Used only during specialization; we already know the intrinsic type
        // from the generic version so we just pass it in
        known_intrinsic: Option<IntrinsicFunctionType>,
    ) -> TyperResult<FunctionId> {
        let mut params = Vec::new();
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
                    TypeVariable { identifier_id: type_parameter.ident, constraints: None };
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
            name: fn_def.name,
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
                let block = self.eval_block(block_ast, fn_scope_id)?;
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
        let function = self.get_function_mut(function_id);
        function.block = body_block;
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
        for fn_def in &ast_namespace.definitions {
            if let Definition::FnDef(fn_def) = fn_def {
                self.eval_function(fn_def, ns_scope_id, None, false, None)?;
            } else {
                panic!("Unsupported definition type inside namespace: {:?}", fn_def)
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
                self.eval_function(fn_def, scope_id, None, false, None)?;
                Ok(())
            }
            Definition::TypeDef(type_defn) => {
                self.eval_type_defn(type_defn, scope_id)?;
                let _typ = self.eval_type_expr(&type_defn.value_expr, scope_id)?;
                Ok(())
            }
        }
    }
    pub fn run(&mut self) -> Result<()> {
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
            println!("{}", self);
            println!("{:?}", errors);
            bail!("Typechecking failed")
        }
        Ok(())
    }
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
    fn display_variable(
        &self,
        var: &Variable,
        writ: &mut impl std::fmt::Write,
    ) -> std::fmt::Result {
        if var.is_mutable {
            writ.write_str("mut ")?;
        }
        writ.write_str(&self.get_ident_str(var.name))?;
        writ.write_str(": ")?;
        self.display_type_id(var.type_id, writ)
    }

    fn display_type_id(&self, ty: TypeId, writ: &mut impl std::fmt::Write) -> std::fmt::Result {
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

    fn write_type(&self, ty: &Type, writ: &mut impl std::fmt::Write) -> std::fmt::Result {
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
                writ.write_str("array<")?;
                self.display_type_id(array.element_type, writ)?;
                writ.write_str(">")
            }
            Type::TypeVariable(tv) => {
                writ.write_str("tvar#")?;
                writ.write_str(&self.get_ident_str(tv.identifier_id))
            }
            Type::Optional(opt) => {
                self.display_type_id(opt.inner_type, writ)?;
                writ.write_char('?')
            }
        }
    }

    pub fn display_function(
        &self,
        function: &Function,
        writ: &mut impl std::fmt::Write,
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
                self.display_block(block, writ)?;
            }
        }
        Ok(())
    }

    fn display_block(
        &self,
        block: &TypedBlock,
        writ: &mut impl std::fmt::Write,
    ) -> std::fmt::Result {
        writ.write_str("{\n")?;
        for stmt in &block.statements {
            self.display_stmt(stmt, writ)?;
            writ.write_str("\n")?;
        }
        writ.write_str("}")
    }

    fn display_stmt(&self, stmt: &TypedStmt, writ: &mut impl std::fmt::Write) -> std::fmt::Result {
        match stmt {
            TypedStmt::Expr(expr) => self.display_expr(expr, writ),
            TypedStmt::ValDef(val_def) => {
                writ.write_str("val ")?;
                self.display_variable(self.get_variable(val_def.variable_id), writ)?;
                writ.write_str(" = ")?;
                self.display_expr(&val_def.initializer, writ)
            }
            TypedStmt::Assignment(assignment) => {
                self.display_expr(&assignment.destination, writ)?;
                writ.write_str(" = ")?;
                self.display_expr(&assignment.value, writ)
            }
            TypedStmt::WhileLoop(while_loop) => {
                writ.write_str("while ")?;
                self.display_expr(&while_loop.cond, writ)?;
                writ.write_str(" ")?;
                self.display_block(&while_loop.block, writ)
            }
        }
    }

    pub fn expr_to_string(&self, expr: &TypedExpr) -> String {
        let mut s = String::new();
        self.display_expr(expr, &mut s).unwrap();
        s
    }

    pub fn display_expr(
        &self,
        expr: &TypedExpr,
        writ: &mut impl std::fmt::Write,
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
                    self.display_expr(expr, writ)?;
                }
                writ.write_str("]")
            }
            TypedExpr::Record(record) => {
                writ.write_str("{")?;
                for (idx, field) in record.fields.iter().enumerate() {
                    if idx > 0 {
                        writ.write_str(", ")?;
                    }
                    writ.write_str(&self.get_ident_str(field.name))?;
                    writ.write_str(": ")?;
                    self.display_expr(&field.expr, writ)?;
                }
                writ.write_str("}")
            }
            TypedExpr::Variable(v) => {
                let variable = self.get_variable(v.variable_id);
                writ.write_str(&self.get_ident_str(variable.name))
            }
            TypedExpr::FieldAccess(field_access) => {
                self.display_expr(&field_access.base, writ)?;
                writ.write_str(".")?;
                writ.write_str(&self.get_ident_str(field_access.target_field))
            }
            TypedExpr::ArrayIndex(array_index) => {
                self.display_expr(&array_index.base_expr, writ)?;
                writ.write_str("[")?;
                self.display_expr(&array_index.index_expr, writ)?;
                writ.write_str("]")
            }
            TypedExpr::StringIndex(string_index) => {
                self.display_expr(&string_index.base_expr, writ)?;
                writ.write_str("[")?;
                self.display_expr(&string_index.index_expr, writ)?;
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
                    self.display_expr(arg, writ)?;
                }
                writ.write_str(")")
            }
            TypedExpr::Block(block) => self.display_block(block, writ),
            TypedExpr::If(if_expr) => {
                writ.write_str("if ")?;
                self.display_expr(&if_expr.condition, writ)?;
                writ.write_str(" ")?;
                self.display_block(&if_expr.consequent, writ)?;
                writ.write_str(" else ")?;
                self.display_block(&if_expr.alternate, writ)?;
                Ok(())
            }
            TypedExpr::UnaryOp(unary_op) => {
                writ.write_fmt(format_args!("{}", unary_op.kind))?;
                self.display_expr(&unary_op.expr, writ)
            }
            TypedExpr::BinaryOp(binary_op) => {
                self.display_expr(&binary_op.lhs, writ)?;
                writ.write_fmt(format_args!(" {} ", binary_op.kind))?;
                self.display_expr(&binary_op.rhs, writ)
            }
            TypedExpr::OptionalSome(opt) => {
                writ.write_str("Some(")?;
                self.display_expr(&opt.inner_expr, writ)?;
                writ.write_str(")")
            }
            TypedExpr::OptionalHasValue(opt) => {
                self.display_expr(&opt, writ)?;
                writ.write_str(".has_value()")
            }
            TypedExpr::OptionalGet(opt) => {
                self.display_expr(&opt.inner_expr, writ)?;
                writ.write_str("!")
            }
        }
    }

    pub fn function_to_string(&self, function: &Function, display_block: bool) -> String {
        let mut s = String::new();
        self.display_function(function, &mut s, display_block).unwrap();
        s
    }
}

#[cfg(test)]
mod test {
    use crate::parse::{parse_text, ParseResult};
    use crate::typer::*;

    fn setup(src: &str, test_name: &str) -> ParseResult<AstModule> {
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
