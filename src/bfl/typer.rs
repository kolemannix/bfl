#![allow(clippy::match_like_matches_macro)]
pub mod derive;
pub mod dump;
pub mod scopes;
pub mod types;

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt::{Display, Formatter, Write};

use anyhow::bail;
use colored::Colorize;
use either::Either;
use log::{debug, trace};

use scopes::*;
use types::*;

use crate::lex::{SpanId, Spans, TokenKind};
use crate::parse::{
    self, FnCallArg, ForExpr, ForExprType, IfExpr, IndexOperation, NumericWidth, ParsedAbilityId,
    ParsedAbilityImplId, ParsedConstantId, ParsedExpressionId, ParsedFunctionId, ParsedId,
    ParsedNamespaceId, ParsedPattern, ParsedPatternId, ParsedTypeDefnId, ParsedTypeExpression,
    ParsedTypeExpressionId, ParsedUnaryOpKind, Sources,
};
use crate::parse::{
    Block, FnCall, IdentifierId, Literal, ParsedExpression, ParsedModule, ParsedStmt,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VariableId(u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NamespaceId(u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AbilityId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AbilityImplId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    Standard,
    External,
    Intrinsic,
}

#[derive(Debug, Clone)]
pub struct TypedAbilityFunctionRef {
    pub function_name: IdentifierId,
    pub function_id: FunctionId,
}

pub const EQUALS_ABILITY_ID: AbilityId = AbilityId(0);
pub const BITWISE_ABILITY_ID: AbilityId = AbilityId(1);

#[derive(Debug, Clone)]
pub struct TypedAbility {
    pub name: IdentifierId,
    pub functions: Vec<TypedAbilityFunctionRef>,
    pub scope_id: ScopeId,
    pub ast_id: ParsedAbilityId,
}

impl TypedAbility {
    pub fn find_function_by_name(
        &self,
        name: IdentifierId,
    ) -> Option<(usize, &TypedAbilityFunctionRef)> {
        self.functions.iter().enumerate().find(|(_, f)| f.function_name == name)
    }
}

#[derive(Debug, Clone)]
pub struct TypedEnumPattern {
    pub enum_type_id: TypeId,
    pub variant_tag_name: IdentifierId,
    pub variant_index: u32,
    pub payload: Option<Box<TypedPattern>>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypedStructPatternField {
    pub field_name: IdentifierId,
    pub field_pattern: TypedPattern,
    pub field_index: u32,
    pub field_type_id: TypeId,
}

#[derive(Debug, Clone)]
pub struct TypedStructPattern {
    pub struct_type_id: TypeId,
    pub fields: Vec<TypedStructPatternField>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypedSomePattern {
    pub optional_type_id: TypeId,
    pub inner_pattern: Box<TypedPattern>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct VariablePattern {
    pub ident: IdentifierId,
    pub span: SpanId,
}

// <pattern> ::= <literal> | <variable> | <enum> | <struc>
// <literal> ::= "(" ")" | "\"" <ident> "\"" | [0-9]+ | "'" [a-z] "'" | "None"
// <variable> ::= <ident>
// <ident> ::= [a-z]*
// <enum> ::= "." <ident> ( "(" <pattern> ")" )?
// <struc> ::= "{" ( <ident> ": " <pattern> ","? )* "}"
#[derive(Debug, Clone)]
pub enum TypedPattern {
    LiteralUnit(SpanId),
    LiteralChar(u8, SpanId),
    LiteralInteger(TypedIntegerValue, SpanId),
    LiteralBool(bool, SpanId),
    LiteralString(String, SpanId),
    LiteralNone(SpanId),
    Some(TypedSomePattern),
    Variable(VariablePattern),
    Enum(TypedEnumPattern),
    Struct(TypedStructPattern),
    Wildcard(SpanId),
}

#[derive(Debug, Clone)]
pub struct TypedBlock {
    pub expr_type: TypeId,
    pub scope_id: ScopeId,
    pub statements: Vec<TypedStmt>,
    pub span: SpanId,
}

impl TypedBlock {
    fn push_stmt(&mut self, stmt: TypedStmt) {
        self.expr_type = stmt.get_type();
        self.statements.push(stmt);
    }

    fn push_expr(&mut self, expr: TypedExpr) {
        self.push_stmt(TypedStmt::Expr(Box::new(expr)))
    }

    pub fn is_single_unit_block(&self) -> bool {
        self.statements.len() == 1 && self.statements[0].is_unit_expr()
    }

    pub fn control_flow_type(&self) -> ControlFlowType {
        self.statements
            .last()
            .map_or(ControlFlowType::Value(UNIT_TYPE_ID), |s| s.control_flow_type())
    }
}

#[derive(Debug, Clone)]
pub struct FnArgDefn {
    pub name: IdentifierId,
    pub variable_id: VariableId,
    pub position: u32,
    pub type_id: TypeId,
    pub span: SpanId,
}

#[derive(Debug)]
pub struct SpecializationParams {
    pub fn_scope_id: ScopeId,
    pub new_name: IdentifierId,
    pub known_intrinsic: Option<IntrinsicFunction>,
    pub generic_parent_function: FunctionId,
    pub ability_impl_info: Option<(AbilityId, TypeId)>,
}

#[derive(Debug, Clone)]
pub struct SpecializationStruct {
    pub specialized_type_params: Vec<TypeId>,
    pub specialized_function_id: FunctionId,
    pub specialized_params: Vec<FnArgDefn>,
}

#[derive(Debug, Clone, Copy)]
pub enum TypedFunctionMetadata {
    Standard { parsed_function_id: ParsedFunctionId, is_method_of: Option<TypeId> },
    Specialization { generic_parsed_function_id: ParsedFunctionId, generic_defn: FunctionId },
    AbilityDefn(ParsedFunctionId),
    AbilityImpl { generic_defn: FunctionId, impl_parsed_function_id: ParsedFunctionId },
}
impl TypedFunctionMetadata {
    /// If this function has a corresponding parsed_function_id, return it.
    /// In the case of a specialized generic function, it will not.
    fn parsed_function_id(&self) -> ParsedFunctionId {
        match self {
            TypedFunctionMetadata::Standard { parsed_function_id, .. } => *parsed_function_id,
            TypedFunctionMetadata::Specialization { generic_parsed_function_id, .. } => {
                *generic_parsed_function_id
            }
            TypedFunctionMetadata::AbilityDefn(pfi) => *pfi,
            TypedFunctionMetadata::AbilityImpl {
                impl_parsed_function_id: parsed_function_id,
                ..
            } => *parsed_function_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypedFunction {
    pub name: IdentifierId,
    pub scope: ScopeId,
    pub ret_type: TypeId,
    pub params: Vec<FnArgDefn>,
    pub type_params: Vec<TypeParam>,
    pub block: Option<TypedBlock>,
    pub intrinsic_type: Option<IntrinsicFunction>,
    pub linkage: Linkage,
    pub specializations: Vec<SpecializationStruct>,
    pub metadata: TypedFunctionMetadata,
    pub span: SpanId,
}

impl TypedFunction {
    pub fn should_codegen_function(&self) -> bool {
        match self.intrinsic_type {
            Some(intrinsic) if intrinsic.is_inlined() => false,
            _ => match self.metadata {
                TypedFunctionMetadata::Standard { .. } => !self.is_generic(),
                TypedFunctionMetadata::Specialization { .. } => true,
                TypedFunctionMetadata::AbilityDefn(_) => false,
                TypedFunctionMetadata::AbilityImpl { .. } => true,
            },
        }
    }
    pub fn is_generic(&self) -> bool {
        !self.type_params.is_empty()
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
    pub span: SpanId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOpKind {
    // Integer operations
    Add,
    Subtract,
    Multiply,
    Divide,
    Rem,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,

    // Boolean operations
    And,
    Or,

    // Equality
    Equals,
    NotEquals,

    // Other
    OptionalElse,
}

impl Display for BinaryOpKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryOpKind::Add => f.write_char('+'),
            BinaryOpKind::Subtract => f.write_char('-'),
            BinaryOpKind::Multiply => f.write_char('*'),
            BinaryOpKind::Divide => f.write_char('/'),
            BinaryOpKind::Rem => f.write_char('%'),
            BinaryOpKind::Less => f.write_char('<'),
            BinaryOpKind::Greater => f.write_char('>'),
            BinaryOpKind::LessEqual => f.write_str("<="),
            BinaryOpKind::GreaterEqual => f.write_str(">="),
            BinaryOpKind::And => f.write_str("and"),
            BinaryOpKind::Or => f.write_str("or"),
            BinaryOpKind::Equals => f.write_str("=="),
            BinaryOpKind::NotEquals => f.write_str("!="),
            BinaryOpKind::OptionalElse => f.write_str("?"),
        }
    }
}

impl BinaryOpKind {
    pub fn precedence(&self) -> usize {
        use BinaryOpKind as B;
        match self {
            B::Rem => 101,
            B::Multiply | B::Divide => 100,
            B::Add | B::Subtract => 90,
            B::Less | B::LessEqual | B::Greater | B::GreaterEqual | B::Equals | B::NotEquals => 80,
            B::And => 70,
            B::Or => 66,
            B::OptionalElse => 65,
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
            TokenKind::LessThanEqual => Some(BinaryOpKind::LessEqual),
            TokenKind::GreaterThanEqual => Some(BinaryOpKind::GreaterEqual),
            TokenKind::KeywordAnd => Some(BinaryOpKind::And),
            TokenKind::KeywordOr => Some(BinaryOpKind::Or),
            TokenKind::EqualsEquals => Some(BinaryOpKind::Equals),
            TokenKind::BangEquals => Some(BinaryOpKind::NotEquals),
            TokenKind::QuestionMark => Some(BinaryOpKind::OptionalElse),
            TokenKind::Percent => Some(BinaryOpKind::Rem),
            _ => None,
        }
    }

    fn is_symmetric_binop(&self) -> bool {
        match self {
            BinaryOpKind::Add => true,
            BinaryOpKind::Subtract => true,
            BinaryOpKind::Multiply => true,
            BinaryOpKind::Divide => true,
            BinaryOpKind::Rem => true,
            BinaryOpKind::Less => true,
            BinaryOpKind::LessEqual => true,
            BinaryOpKind::Greater => true,
            BinaryOpKind::GreaterEqual => true,
            BinaryOpKind::And => true,
            BinaryOpKind::Or => true,
            BinaryOpKind::Equals => true,
            BinaryOpKind::NotEquals => true,
            BinaryOpKind::OptionalElse => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BinaryOp {
    pub kind: BinaryOpKind,
    pub ty: TypeId,
    pub lhs: Box<TypedExpr>,
    pub rhs: Box<TypedExpr>,
    pub span: SpanId,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum UnaryOpKind {
    BooleanNegation,
    Reference,
    Dereference,
    ReferenceToInt,
}

impl Display for UnaryOpKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UnaryOpKind::BooleanNegation => f.write_str("not "),
            UnaryOpKind::Reference => f.write_char('&'),
            UnaryOpKind::Dereference => f.write_char('*'),
            UnaryOpKind::ReferenceToInt => f.write_str("(*int)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnaryOp {
    pub kind: UnaryOpKind,
    pub type_id: TypeId,
    pub expr: Box<TypedExpr>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub callee_function_id: FunctionId,
    pub args: Vec<TypedExpr>,
    pub type_args: Vec<TypeParam>,
    pub ret_type: TypeId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: IdentifierId,
    pub expr: TypedExpr,
}

#[derive(Debug, Clone)]
pub struct Struct {
    pub fields: Vec<StructField>,
    pub type_id: TypeId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct ArrayLiteral {
    pub elements: Vec<TypedExpr>,
    pub type_id: TypeId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypedIf {
    pub condition: TypedExpr,
    pub consequent: TypedBlock,
    pub alternate: TypedBlock,
    pub ty: TypeId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct FieldAccess {
    pub base: Box<TypedExpr>,
    pub target_field: IdentifierId,
    pub target_field_index: u32,
    pub ty: TypeId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct IndexOp {
    pub base_expr: Box<TypedExpr>,
    pub index_expr: Box<TypedExpr>,
    pub result_type: TypeId,
    pub span: SpanId,
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
    pub span: SpanId,
    pub checked: bool,
}

#[derive(Debug, Clone)]
pub struct TypedTagExpr {
    pub name: IdentifierId,
    pub type_id: TypeId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypedEnumConstructor {
    pub type_id: TypeId,
    pub variant_name: IdentifierId,
    pub variant_index: u32,
    pub payload: Option<Box<TypedExpr>>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct GetEnumPayload {
    pub target_expr: Box<TypedExpr>,
    pub payload_type_id: TypeId,
    pub variant_name: IdentifierId,
    pub variant_index: u32,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypedEnumIsVariantExpr {
    pub target_expr: Box<TypedExpr>,
    pub variant_name: IdentifierId,
    pub variant_index: u32,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypedEnumCast {
    pub base: Box<TypedExpr>,
    pub variant_type_id: TypeId,
    pub variant_name: IdentifierId,
    pub variant_index: u32,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct NoOpCast {
    pub base: Box<TypedExpr>,
    pub type_id: TypeId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
struct TypedMatchCase {
    // Will be used when we do exhaustive matching
    #[allow(unused)]
    pattern: TypedPattern,
    pre_stmts: Vec<TypedStmt>,
    condition: TypedExpr,
    arm_block: TypedBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedIntegerValue {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
}

impl TypedIntegerValue {
    pub fn as_u64(&self) -> u64 {
        match self {
            TypedIntegerValue::U8(v) => *v as u64,
            TypedIntegerValue::U16(v) => *v as u64,
            TypedIntegerValue::U32(v) => *v as u64,
            TypedIntegerValue::U64(v) => *v,
            TypedIntegerValue::I8(v) => *v as u64,
            TypedIntegerValue::I16(v) => *v as u64,
            TypedIntegerValue::I32(v) => *v as u64,
            TypedIntegerValue::I64(v) => *v as u64,
        }
    }
}

impl Display for TypedIntegerValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TypedIntegerValue::U8(v) => write!(f, "{}u8", v),
            TypedIntegerValue::U16(v) => write!(f, "{}u16", v),
            TypedIntegerValue::U32(v) => write!(f, "{}u32", v),
            TypedIntegerValue::U64(v) => write!(f, "{}u64", v),
            TypedIntegerValue::I8(v) => write!(f, "{}i8", v),
            TypedIntegerValue::I16(v) => write!(f, "{}i16", v),
            TypedIntegerValue::I32(v) => write!(f, "{}i32", v),
            TypedIntegerValue::I64(v) => write!(f, "{}i64", v),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypedIntegerExpr {
    pub value: TypedIntegerValue,
    pub span: SpanId,
}

impl TypedIntegerExpr {
    pub fn type_id(&self) -> TypeId {
        match self.value {
            TypedIntegerValue::U8(_) => U8_TYPE_ID,
            TypedIntegerValue::U16(_) => U16_TYPE_ID,
            TypedIntegerValue::U32(_) => U32_TYPE_ID,
            TypedIntegerValue::U64(_) => U64_TYPE_ID,
            TypedIntegerValue::I8(_) => I8_TYPE_ID,
            TypedIntegerValue::I16(_) => I16_TYPE_ID,
            TypedIntegerValue::I32(_) => I32_TYPE_ID,
            TypedIntegerValue::I64(_) => I64_TYPE_ID,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CastType {
    IntegerExtend,
    IntegerTruncate,
    Integer8ToChar,
    IntegerExtendFromChar,
    EnumVariant,
    KnownNoOp,
}

impl Display for CastType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CastType::IntegerExtend => write!(f, "iext"),
            CastType::IntegerTruncate => write!(f, "itrunc"),
            CastType::Integer8ToChar => write!(f, "i8tochar"),
            CastType::IntegerExtendFromChar => write!(f, "iextfromchar"),
            CastType::EnumVariant => write!(f, "enum"),
            CastType::KnownNoOp => write!(f, "noop"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypedCast {
    pub cast_type: CastType,
    pub base_expr: Box<TypedExpr>,
    pub target_type_id: TypeId,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypedReturn {
    pub value: Box<TypedExpr>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub enum ControlFlowType {
    Returns(TypeId),
    Value(TypeId),
    //Breaks,
    //Continues,
}

#[derive(Debug, Clone)]
pub enum TypedExpr {
    Unit(SpanId),
    Char(u8, SpanId),
    Bool(bool, SpanId),
    Integer(TypedIntegerExpr),
    Str(String, SpanId),
    None(TypeId, SpanId),
    Struct(Struct),
    Array(ArrayLiteral),
    Variable(VariableExpr),
    StructFieldAccess(FieldAccess),
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
    Tag(TypedTagExpr),
    EnumConstructor(TypedEnumConstructor),
    EnumIsVariant(TypedEnumIsVariantExpr),
    EnumGetPayload(GetEnumPayload),
    Cast(TypedCast),
    Return(TypedReturn),
}

impl From<VariableExpr> for TypedExpr {
    fn from(value: VariableExpr) -> Self {
        TypedExpr::Variable(value)
    }
}

impl TypedExpr {
    #[inline]
    pub fn get_type(&self) -> TypeId {
        match self {
            TypedExpr::None(type_id, _) => *type_id,
            TypedExpr::Unit(_) => UNIT_TYPE_ID,
            TypedExpr::Char(_, _) => CHAR_TYPE_ID,
            TypedExpr::Str(_, _) => STRING_TYPE_ID,
            TypedExpr::Integer(integer) => integer.type_id(),
            TypedExpr::Bool(_, _) => BOOL_TYPE_ID,
            TypedExpr::Struct(struc) => struc.type_id,
            TypedExpr::Array(arr) => arr.type_id,
            TypedExpr::Variable(var) => var.type_id,
            TypedExpr::StructFieldAccess(field_access) => field_access.ty,
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
            TypedExpr::Tag(tag_expr) => tag_expr.type_id,
            TypedExpr::EnumConstructor(enum_cons) => enum_cons.type_id,
            TypedExpr::EnumIsVariant(_is_variant) => BOOL_TYPE_ID,
            TypedExpr::EnumGetPayload(as_variant) => as_variant.payload_type_id,
            TypedExpr::Cast(c) => c.target_type_id,
            TypedExpr::Return(_ret) => NEVER_TYPE_ID,
        }
    }
    #[inline]
    pub fn get_span(&self) -> SpanId {
        match self {
            TypedExpr::Unit(span) => *span,
            TypedExpr::Char(_, span) => *span,
            TypedExpr::Bool(_, span) => *span,
            TypedExpr::Integer(int) => int.span,
            TypedExpr::Str(_, span) => *span,
            TypedExpr::None(_, span) => *span,
            TypedExpr::Struct(struc) => struc.span,
            TypedExpr::Array(array) => array.span,
            TypedExpr::Variable(var) => var.span,
            TypedExpr::StructFieldAccess(field_access) => field_access.span,
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
            TypedExpr::Tag(tag_expr) => tag_expr.span,
            TypedExpr::EnumConstructor(e) => e.span,
            TypedExpr::EnumIsVariant(is_variant) => is_variant.span,
            TypedExpr::EnumGetPayload(as_variant) => as_variant.span,
            TypedExpr::Cast(c) => c.span,
            TypedExpr::Return(ret) => ret.span,
        }
    }

    pub fn control_flow_type(&self) -> ControlFlowType {
        match self {
            TypedExpr::Return(ret) => ControlFlowType::Returns(ret.value.get_type()),
            TypedExpr::Block(block) => block.control_flow_type(),
            _ => ControlFlowType::Value(self.get_type()),
        }
    }

    pub fn expect_variable(self) -> VariableExpr {
        if let Self::Variable(v) = self {
            v
        } else {
            panic!("Expected variable expression")
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValDef {
    pub variable_id: VariableId,
    pub ty: TypeId,
    pub initializer: TypedExpr,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub destination: Box<TypedExpr>,
    pub value: Box<TypedExpr>,
    pub span: SpanId,
}

#[derive(Debug, Clone)]
pub struct TypedWhileLoop {
    pub cond: TypedExpr,
    pub block: TypedBlock,
    pub span: SpanId,
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
    pub fn get_span(&self) -> SpanId {
        match self {
            TypedStmt::Expr(e) => e.get_span(),
            TypedStmt::ValDef(v) => v.span,
            TypedStmt::Assignment(ass) => ass.span,
            TypedStmt::WhileLoop(w) => w.span,
        }
    }

    pub fn get_type(&self) -> TypeId {
        match self {
            TypedStmt::Expr(expr) => expr.get_type(),
            TypedStmt::ValDef(_) => UNIT_TYPE_ID,
            TypedStmt::Assignment(_) => UNIT_TYPE_ID,
            TypedStmt::WhileLoop(_) => UNIT_TYPE_ID,
        }
    }

    pub fn is_unit_expr(&self) -> bool {
        if let TypedStmt::Expr(expr) = self {
            if let TypedExpr::Unit(_) = **expr {
                return true;
            }
        }
        return false;
    }

    pub fn control_flow_type(&self) -> ControlFlowType {
        match self {
            TypedStmt::Expr(expr) => expr.control_flow_type(),
            TypedStmt::ValDef(_) => ControlFlowType::Value(UNIT_TYPE_ID),
            TypedStmt::Assignment(_) => ControlFlowType::Value(UNIT_TYPE_ID),
            TypedStmt::WhileLoop(_) => ControlFlowType::Value(UNIT_TYPE_ID),
        }
    }
}

#[derive(Debug)]
pub struct TyperError {
    message: String,
    span: SpanId,
}

impl TyperError {
    fn make(message: impl AsRef<str>, span: SpanId) -> TyperError {
        TyperError { message: message.as_ref().to_owned(), span }
    }
}

impl Display for TyperError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("typer error {}: {:?}", self.message, self))
    }
}

impl Error for TyperError {}

pub type TyperResult<A> = Result<A, TyperError>;

#[derive(Debug)]
pub struct Variable {
    pub name: IdentifierId,
    pub type_id: TypeId,
    pub is_mutable: bool,
    pub owner_scope: ScopeId,
}

#[derive(Debug)]
pub struct Constant {
    pub variable_id: VariableId,
    pub expr: TypedExpr,
    pub ty: TypeId,
    pub span: SpanId,
}

#[derive(Debug)]
pub enum NamespaceType {
    User,
    Ability,
    Root,
}

#[derive(Debug)]
pub struct Namespace {
    pub name: IdentifierId,
    pub scope_id: ScopeId,
    pub namespace_type: NamespaceType,
    pub companion_type_id: Option<TypeId>,
    pub parent_id: Option<NamespaceId>,
}

pub struct Namespaces {
    pub namespaces: Vec<Namespace>,
}

impl Namespaces {
    pub fn get(&self, id: NamespaceId) -> &Namespace {
        &self.namespaces[id.0 as usize]
    }

    pub fn get_mut(&mut self, id: NamespaceId) -> &mut Namespace {
        &mut self.namespaces[id.0 as usize]
    }

    pub fn add(&mut self, namespace: Namespace) -> NamespaceId {
        let id = NamespaceId(self.namespaces.len() as u32);
        self.namespaces.push(namespace);
        id
    }

    pub fn iter(&self) -> impl Iterator<Item = (NamespaceId, &Namespace)> {
        self.namespaces
            .iter()
            .enumerate()
            .map(|(index, namespace)| (NamespaceId(index as u32), namespace))
    }

    pub fn name_chain(&self, id: NamespaceId) -> VecDeque<IdentifierId> {
        let mut chain = VecDeque::with_capacity(8);
        let mut id = id;
        loop {
            let namespace = &self.get(id);
            chain.push_front(namespace.name);
            if let Some(parent_id) = namespace.parent_id {
                id = parent_id;
            } else {
                break;
            }
        }
        chain
    }

    pub fn get_scope(&self, namespace_id: NamespaceId) -> ScopeId {
        self.get(namespace_id).scope_id
    }

    pub fn namespace_for_scope(&self, scope_id: ScopeId) -> Option<NamespaceId> {
        self.namespaces
            .iter()
            .position(|namespace| namespace.scope_id == scope_id)
            .map(|id| NamespaceId(id as u32))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntrinsicFunction {
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
    RawPointerToReference,
    SizeOf,
    AlignOf,
    BitNot,
    BitAnd,
    BitOr,
    BitXor,
    BitShiftLeft,
    BitShiftRight,
}

impl IntrinsicFunction {
    fn is_inlined(self: Self) -> bool {
        match self {
            IntrinsicFunction::Exit => false,
            IntrinsicFunction::PrintInt => false,
            IntrinsicFunction::PrintString => false,
            IntrinsicFunction::StringLength => false,
            IntrinsicFunction::ArrayLength => false,
            IntrinsicFunction::ArrayNew => false,
            IntrinsicFunction::ArrayGrow => false,
            IntrinsicFunction::ArraySetLength => false,
            IntrinsicFunction::ArrayCapacity => false,
            IntrinsicFunction::StringFromCharArray => false,
            IntrinsicFunction::StringEquals => false,
            IntrinsicFunction::RawPointerToReference => false,
            IntrinsicFunction::SizeOf => true,
            IntrinsicFunction::AlignOf => true,
            IntrinsicFunction::BitNot => true,
            IntrinsicFunction::BitAnd => true,
            IntrinsicFunction::BitOr => true,
            IntrinsicFunction::BitXor => true,
            IntrinsicFunction::BitShiftLeft => true,
            IntrinsicFunction::BitShiftRight => true,
        }
    }
}

fn make_error<T: AsRef<str>>(message: T, span: SpanId) -> TyperError {
    TyperError::make(message.as_ref(), span)
}

fn make_fail_span<A, T: AsRef<str>>(message: T, span: SpanId) -> TyperResult<A> {
    Err(make_error(message, span))
}

/// thanks heather
macro_rules! panic_at_disco {
    ($($format_args:expr),*) => {
        {
            let s: String = format!($($format_args),*);
            panic!("Panic!! at disco!!!: {}", s)
        }
    };
}

macro_rules! ferr {
    ($span:expr, $($format_args:expr),*) => {
        {
            let s: String = format!($($format_args),*);
            make_error(&s, $span)
        }
    };
}

macro_rules! ffail {
    ($span:expr, $($format_args:expr),*) => {
        {
            let s: String = format!($($format_args),*);
            make_fail_span(&s, $span)
        }
    };
}

fn make_fail_ast_id<A, T: AsRef<str>>(
    ast: &ParsedModule,
    message: T,
    parsed_id: ParsedId,
) -> TyperResult<A> {
    let span = ast.get_span_for_id(parsed_id);
    Err(make_error(message, span))
}

fn print_error(spans: &Spans, sources: &Sources, message: impl AsRef<str>, span: SpanId) {
    parse::print_error_location(spans, sources, span);
    eprintln!("\t{}", message.as_ref());
}

#[derive(Debug, Clone)]
pub struct TypedAbilityImpl {
    pub type_id: TypeId,
    pub ability_id: AbilityId,
    /// In order they are defined in the ability. If None, predecl only
    pub functions: Vec<FunctionId>,
    pub span: SpanId,
}

impl TypedAbilityImpl {
    pub fn function_at_index(&self, index: usize) -> FunctionId {
        self.functions[index]
    }
}

#[derive(Default, Debug)]
pub struct Variables {
    variables: Vec<Variable>,
}

impl Variables {
    fn add_variable(&mut self, typ: Variable) -> VariableId {
        let index = self.variables.len();
        self.variables.push(typ);
        VariableId(index as u32)
    }

    pub fn get_variable(&self, variable_id: VariableId) -> &Variable {
        &self.variables[variable_id.0 as usize]
    }

    pub fn iter(&self) -> impl Iterator<Item = (VariableId, &Variable)> {
        self.variables.iter().enumerate().map(|(i, v)| (VariableId(i as u32), v))
    }
}

// Not using this yet but probably need to be
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Symbol {
    pub namespace: NamespaceId,
    pub identifier: IdentifierId,
}

pub struct TypedModule {
    pub ast: ParsedModule,
    functions: Vec<TypedFunction>,
    pub variables: Variables,
    pub types: Types,
    pub constants: Vec<Constant>,
    pub scopes: Scopes,
    pub errors: Vec<TyperError>,
    pub namespaces: Namespaces,
    pub abilities: Vec<TypedAbility>,
    pub ability_impls: Vec<TypedAbilityImpl>,
    pub namespace_ast_mappings: HashMap<ParsedNamespaceId, NamespaceId>,
    pub function_ast_mappings: HashMap<ParsedFunctionId, FunctionId>,
    pub ability_impl_ast_mappings: HashMap<ParsedAbilityImplId, AbilityImplId>,
}

impl TypedModule {
    pub fn new(parsed_module: ParsedModule) -> TypedModule {
        let types = Types {
            types: vec![
                Type::Unit,
                Type::Char,
                Type::Bool,
                Type::String,
                Type::Never,
                Type::Integer(IntegerType::U8),
                Type::Integer(IntegerType::U16),
                Type::Integer(IntegerType::U32),
                Type::Integer(IntegerType::U64),
                Type::Integer(IntegerType::I8),
                Type::Integer(IntegerType::I16),
                Type::Integer(IntegerType::I32),
                Type::Integer(IntegerType::I64),
            ],
            type_defn_mapping: HashMap::new(),
        };
        debug_assert!(matches!(*types.get(UNIT_TYPE_ID), Type::Unit));
        debug_assert!(matches!(*types.get(CHAR_TYPE_ID), Type::Char));
        debug_assert!(matches!(*types.get(BOOL_TYPE_ID), Type::Bool));
        debug_assert!(matches!(*types.get(STRING_TYPE_ID), Type::String));
        debug_assert!(matches!(*types.get(NEVER_TYPE_ID), Type::Never));
        debug_assert!(matches!(*types.get(U8_TYPE_ID), Type::Integer(IntegerType::U8)));
        debug_assert!(matches!(*types.get(U16_TYPE_ID), Type::Integer(IntegerType::U16)));
        debug_assert!(matches!(*types.get(U32_TYPE_ID), Type::Integer(IntegerType::U32)));
        debug_assert!(matches!(*types.get(U64_TYPE_ID), Type::Integer(IntegerType::U64)));
        debug_assert!(matches!(*types.get(I8_TYPE_ID), Type::Integer(IntegerType::I8)));
        debug_assert!(matches!(*types.get(I16_TYPE_ID), Type::Integer(IntegerType::I16)));
        debug_assert!(matches!(*types.get(I32_TYPE_ID), Type::Integer(IntegerType::I32)));
        debug_assert!(matches!(*types.get(I64_TYPE_ID), Type::Integer(IntegerType::I64)));

        let scopes = Scopes::make();
        let namespaces = Namespaces { namespaces: Vec::new() };
        TypedModule {
            ast: parsed_module,
            functions: Vec::new(),
            variables: Variables::default(),
            types,
            constants: Vec::new(),
            scopes,
            errors: Vec::new(),
            namespaces,
            abilities: Vec::new(),
            ability_impls: Vec::new(),
            namespace_ast_mappings: HashMap::new(),
            function_ast_mappings: HashMap::new(),
            ability_impl_ast_mappings: HashMap::new(),
        }
    }

    pub fn function_iter(&self) -> impl Iterator<Item = (FunctionId, &TypedFunction)> {
        self.functions.iter().enumerate().map(|(idx, f)| (FunctionId(idx as u32), f))
    }

    pub fn name(&self) -> &str {
        &self.ast.name
    }

    pub fn get_ident_str(&self, id: IdentifierId) -> &str {
        self.ast.identifiers.get_name(id)
    }

    pub fn get_identifier(&self, name: &str) -> Option<IdentifierId> {
        self.ast.identifiers.get(name)
    }

    pub fn get_namespace_scope(&self, namespace_id: NamespaceId) -> &Scope {
        let scope_id = self.namespaces.get_scope(namespace_id);
        self.scopes.get_scope(scope_id)
    }

    pub fn get_main_function_id(&self) -> Option<FunctionId> {
        let main = self.get_identifier("main").unwrap();
        self.scopes.get_root_scope().find_function(main)
    }

    fn type_eq(type1: &Type, type2: &Type) -> bool {
        match (type1, type2) {
            (Type::Unit, Type::Unit) => true,
            (Type::Char, Type::Char) => true,
            (Type::Integer(int1), Type::Integer(int2)) => int1 == int2,
            (Type::Bool, Type::Bool) => true,
            (Type::String, Type::String) => true,
            (Type::Struct(r1), Type::Struct(r2)) => {
                if r1.is_named() || r2.is_named() {
                    return false;
                }
                if r1.fields.len() != r2.fields.len() {
                    return false;
                }
                for (index, f1) in r1.fields.iter().enumerate() {
                    let f2 = &r2.fields[index];
                    let mismatch = f1.name != f2.name || f1.type_id != f2.type_id;
                    if mismatch {
                        return false;
                    }
                }
                return true;
            }
            (Type::Array(a1), Type::Array(a2)) => a1.element_type == a2.element_type,
            (Type::Optional(o1), Type::Optional(o2)) => o1.inner_type == o2.inner_type,
            (Type::Reference(r1), Type::Reference(r2)) => r1.inner_type == r2.inner_type,
            (Type::TypeVariable(t1), Type::TypeVariable(t2)) => {
                t1.name == t2.name && t1.scope_id == t2.scope_id
            }
            (Type::TagInstance(t1), Type::TagInstance(t2)) => t1.ident == t2.ident,
            (Type::Enum(e1), Type::Enum(e2)) => {
                if e1.type_defn_info.is_some() || e2.type_defn_info.is_some() {
                    return false;
                }
                if e1.variants.len() != e2.variants.len() {
                    return false;
                }
                for (index, v1) in e1.variants.iter().enumerate() {
                    let v2 = &e2.variants[index];
                    let mismatch = v1.tag_name != v2.tag_name || v1.payload != v2.payload;
                    if mismatch {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn add_ability(&mut self, ability: TypedAbility) -> AbilityId {
        let ability_id = self.abilities.len();
        self.abilities.push(ability);
        AbilityId(ability_id as u32)
    }

    fn get_ability(&self, ability_id: AbilityId) -> &TypedAbility {
        &self.abilities[ability_id.0 as usize]
    }

    fn eval_type_defn(
        &mut self,
        parsed_type_defn_id: ParsedTypeDefnId,
        scope_id: ScopeId,
    ) -> TyperResult<TypeId> {
        let parsed_type_defn = self.ast.get_type_defn(parsed_type_defn_id).clone();
        let existing_defn = self.types.find_type_defn_mapping(parsed_type_defn_id);
        if let Some(existing_defn) = existing_defn {
            return Ok(existing_defn);
        }

        // Find companion namespace if exists and update type_defn_info
        let companion_namespace_id =
            self.scopes.get_scope(scope_id).find_namespace(parsed_type_defn.name);

        // The type defn info is about a lot more than just definition site.
        // It differentiates between simple names for shapes of structs and
        // structs with an actual named-based identity that can have methods, implement traits, etc.
        let type_defn_info = TypeDefnInfo {
            name: parsed_type_defn.name,
            scope: scope_id,
            companion_namespace: companion_namespace_id,
            generic_parent: None,
        };

        let has_type_params = !parsed_type_defn.type_params.is_empty();

        let passed_type_defn_info = if has_type_params
            || (parsed_type_defn.flags.is_alias() && !parsed_type_defn.flags.is_opaque())
        {
            None
        } else {
            Some(type_defn_info.clone())
        };

        let defn_scope_id = self.scopes.add_child_scope(
            scope_id,
            ScopeType::TypeDefn,
            None,
            Some(parsed_type_defn.name),
        );
        let our_scope = self.scopes.get_scope_mut(defn_scope_id);

        let mut type_params: Vec<GenericTypeParam> =
            Vec::with_capacity(parsed_type_defn.type_params.len());
        for type_param in parsed_type_defn.type_params.iter() {
            let type_variable = Type::TypeVariable(TypeVariable {
                name: type_param.ident,
                scope_id: defn_scope_id,
                _constraints: None,
            });
            let type_variable_id = self.types.add_type(type_variable);
            type_params
                .push(GenericTypeParam { name: type_param.ident, type_id: type_variable_id });
            let added = our_scope.add_type(type_param.ident, type_variable_id);
            if !added {
                return ffail!(
                    type_param.span,
                    "Type variable name '{}' is taken",
                    self.get_ident_str(type_param.ident).blue()
                );
            }
        }

        let rhs_type_id =
            self.eval_type_expr(parsed_type_defn.value_expr, defn_scope_id, passed_type_defn_info)?;
        let type_id = if has_type_params {
            eprintln!(
                "Generating a generic wrapper for a {} with companion namespace {:?}",
                self.type_id_to_string(rhs_type_id),
                type_defn_info.companion_namespace
            );
            let gen = GenericType {
                params: type_params,
                ast_id: parsed_type_defn_id.into(),
                inner: rhs_type_id,
                type_defn_info,
                specializations: HashMap::new(),
            };
            Ok(self.types.add_type(Type::Generic(gen)))
        } else if parsed_type_defn.flags.is_alias() {
            if parsed_type_defn.flags.is_opaque() {
                // Opaque alias
                eprintln!(
                    "Generating an opaque alias for a {} with companion namespace {:?}",
                    self.type_id_to_string(rhs_type_id),
                    type_defn_info.companion_namespace,
                );
                let alias = OpaqueTypeAlias {
                    ast_id: parsed_type_defn_id.into(),
                    aliasee: rhs_type_id,
                    type_defn_info,
                };
                Ok(self.types.add_type(Type::OpaqueAlias(alias)))
            } else {
                // Transparent alias
                match self.types.get(rhs_type_id) {
                    Type::Never => {
                        make_fail_span("Why would you alias 'never'", parsed_type_defn.span)
                    }
                    _ => {
                        eprintln!(
                            "Creating transparent alias type defn for type: {}",
                            self.type_id_to_string(rhs_type_id)
                        );
                        Ok(rhs_type_id)
                    }
                }
            }
        } else {
            // 'New type' territory; must be a named struct/enum OR a builtin
            match self.types.get(rhs_type_id) {
                Type::Unit | Type::Char | Type::Bool | Type::String | Type::Never => {
                    Ok(rhs_type_id)
                }
                Type::Struct(_s) => Ok(rhs_type_id),
                Type::Enum(_e) => Ok(rhs_type_id),
                _other => {
                    ffail!(parsed_type_defn.span, "Non-alias type definition must be a struct or enum; perhaps you meant to create an alias `type alias [opaque] <name> = <type>`")
                }
            }
        }?;
        self.types.add_type_defn_mapping(parsed_type_defn_id, type_id);

        let type_added = self.scopes.add_type(scope_id, parsed_type_defn.name, type_id);
        if !type_added {
            return ffail!(
                parsed_type_defn.span,
                "Type {} exists",
                self.get_ident_str(parsed_type_defn.name)
            );
        };
        if let Some(companion_namespace_id) = companion_namespace_id {
            self.namespaces.get_mut(companion_namespace_id).companion_type_id = Some(type_id);
            eprintln!(
                "Found and set companion namespace for {} to {}",
                self.get_ident_str(parsed_type_defn.name),
                type_id
            );
        }

        let removed = self.scopes.remove_pending_type_defn(scope_id, parsed_type_defn.name);
        if !removed {
            panic!("Failed to remove pending type defn");
        } else {
            eprintln!("Removed pending type defn {}", self.get_ident_str(parsed_type_defn.name))
        }

        Ok(type_id)
    }

    fn eval_type_expr(
        &mut self,
        type_expr_id: ParsedTypeExpressionId,
        scope_id: ScopeId,
        // If this is a type definition, this is the type definition info
        // that should be attached to the type
        type_defn_info: Option<TypeDefnInfo>,
    ) -> TyperResult<TypeId> {
        let base = match self.ast.type_expressions.get(type_expr_id) {
            ParsedTypeExpression::Builtin(span) => {
                let name = self
                    .get_ident_str(type_defn_info.expect("required defn info for builtin").name);
                match name {
                    "unit" => Ok(UNIT_TYPE_ID),
                    "char" => Ok(CHAR_TYPE_ID),
                    "bool" => Ok(BOOL_TYPE_ID),
                    "string" => Ok(STRING_TYPE_ID),
                    "never" => Ok(NEVER_TYPE_ID),
                    _ => ffail!(*span, "Unknown builtin type '{}'", name),
                }
            }
            ParsedTypeExpression::Integer(num_type) => match (num_type.width, num_type.signed) {
                (NumericWidth::B8, false) => Ok(U8_TYPE_ID),
                (NumericWidth::B16, false) => Ok(U16_TYPE_ID),
                (NumericWidth::B32, false) => Ok(U32_TYPE_ID),
                (NumericWidth::B64, false) => Ok(U64_TYPE_ID),
                (NumericWidth::B8, true) => Ok(I8_TYPE_ID),
                (NumericWidth::B16, true) => Ok(I16_TYPE_ID),
                (NumericWidth::B32, true) => Ok(I32_TYPE_ID),
                (NumericWidth::B64, true) => Ok(I64_TYPE_ID),
            },
            ParsedTypeExpression::Struct(struct_defn) => {
                let struct_defn = struct_defn.clone();
                let mut fields: Vec<StructTypeField> = Vec::new();
                for (index, ast_field) in struct_defn.fields.iter().enumerate() {
                    let ty = self.eval_type_expr(ast_field.ty, scope_id, None)?;
                    fields.push(StructTypeField {
                        name: ast_field.name,
                        type_id: ty,
                        index: index as u32,
                    })
                }
                let struct_defn =
                    StructType { fields, type_defn_info, ast_node: type_expr_id.into() };
                let type_id = self.types.add_type(Type::Struct(struct_defn));
                Ok(type_id)
            }
            ParsedTypeExpression::Name(name, span) => {
                if Some(*name) == self.get_identifier("never") {
                    Ok(NEVER_TYPE_ID)
                } else {
                    let name = *name;
                    match self.scopes.find_type(scope_id, name) {
                        Some(named_type) => Ok(named_type),
                        None => match self.scopes.find_pending_type_defn(scope_id, name) {
                            None => make_fail_span(
                                format!("No type named {} is in scope", self.get_ident_str(name)),
                                *span,
                            ),
                            Some((pending_defn_id, pending_defn_scope_id)) => {
                                eprintln!(
                                    "Recursing into pending type defn {}",
                                    self.get_ident_str(
                                        self.ast.get_type_defn(pending_defn_id).name
                                    )
                                );
                                let type_id =
                                    self.eval_type_defn(pending_defn_id, pending_defn_scope_id)?;
                                Ok(type_id)
                            }
                        },
                    }
                }
            }
            ParsedTypeExpression::TagName(ident, _span) => {
                // Make a type for this tag, if there isn't one
                let tag_type_id = self.types.get_type_for_tag(*ident);
                Ok(tag_type_id)
            }
            ParsedTypeExpression::TypeApplication(ty_app) => {
                if self.ast.identifiers.get_name(ty_app.base) == "Array" {
                    if ty_app.params.len() == 1 {
                        let element_ty = self.eval_type_expr(ty_app.params[0], scope_id, None)?;
                        let array_ty = ArrayType { element_type: element_ty };
                        let type_id = self.types.add_type(Type::Array(array_ty));
                        Ok(type_id)
                    } else {
                        make_fail_span("Expected 1 type parameter for Array", ty_app.span)
                    }
                } else {
                    self.eval_type_application(type_expr_id, scope_id)
                }
            }
            ParsedTypeExpression::Optional(opt) => {
                let inner_ty = self.eval_type_expr(opt.base, scope_id, None)?;
                let optional_type = Type::Optional(OptionalType { inner_type: inner_ty });
                let type_id = self.types.add_type(optional_type);
                Ok(type_id)
            }
            ParsedTypeExpression::Reference(r) => {
                let inner_ty = self.eval_type_expr(r.base, scope_id, None)?;
                let reference_type = Type::Reference(ReferenceType { inner_type: inner_ty });
                let type_id = self.types.add_type(reference_type);
                Ok(type_id)
            }
            ParsedTypeExpression::Enum(e) => {
                let e = e.clone();
                let mut variants = Vec::with_capacity(e.variants.len());
                for (index, v) in e.variants.iter().enumerate() {
                    // Hack: Ensure there's a type for this tag
                    self.types.get_type_for_tag(v.tag_name);

                    let payload_type_id = match &v.payload_expression {
                        None => None,
                        Some(payload_type_expr) => {
                            let type_id =
                                self.eval_type_expr(*payload_type_expr, scope_id, None)?;
                            Some(type_id)
                        }
                    };
                    let variant = TypedEnumVariant {
                        enum_type_id: TypeId::PENDING,
                        my_type_id: TypeId::PENDING,
                        tag_name: v.tag_name,
                        index: index as u32,
                        payload: payload_type_id,
                    };
                    variants.push(variant);
                }
                let enum_type = Type::Enum(TypedEnum {
                    variants,
                    type_defn_info,
                    ast_node: type_expr_id.into(),
                });
                let enum_type_id = self.types.add_type(enum_type);
                Ok(enum_type_id)
            }
            ParsedTypeExpression::DotMemberAccess(dot_acc) => {
                let dot_acc = dot_acc.clone();
                let base_type = self.eval_type_expr(dot_acc.base, scope_id, None)?;
                match self.types.get(base_type) {
                    // You can do dot access on enums to get their variants
                    Type::Enum(e) => {
                        let Some(matching_variant) =
                            e.variants.iter().find(|v| v.tag_name == dot_acc.member_name)
                        else {
                            return make_fail_ast_id(
                                &self.ast,
                                "Variant <todo> does not exist on Enum <todo>",
                                type_expr_id.into(),
                            );
                        };
                        let variant_type = matching_variant.my_type_id;
                        Ok(variant_type)
                    }
                    // You can do dot access on structs to get their members!
                    Type::Struct(_s) => todo!("struct member access"),
                    // You can do dot access on Array to get its element type
                    Type::Array(_a) => todo!("array member access"),
                    // You can do dot access on Optionals to get out their 'inner' types
                    Type::Optional(_o) => todo!("optional dot access for inner type"),
                    // You can do dot access on References to get out their 'inner' types
                    Type::Reference(_r) => todo!("reference dot access for referenced type"),
                    _other => {
                        return make_fail_ast_id(
                            &self.ast,
                            "Invalid type for '.' access",
                            type_expr_id.into(),
                        )
                    } // You can do dot access on function names (which are not type expressions yet)
                      // to talk about their return types (and argument types?)
                }
            }
        }?;
        Ok(base)
    }

    fn eval_type_application(
        &mut self,
        ty_app_id: ParsedTypeExpressionId,
        scope_id: ScopeId,
    ) -> TyperResult<TypeId> {
        let ParsedTypeExpression::TypeApplication(ty_app) =
            self.ast.type_expressions.get(ty_app_id)
        else {
            panic!("Expected TypeApplication")
        };
        let ty_app = ty_app.clone();
        match self.scopes.find_type(scope_id, ty_app.base) {
            None => {
                return ffail!(
                    ty_app.span,
                    "No type named '{}' is in scope",
                    self.get_ident_str(ty_app.base).blue()
                )
            }
            Some(type_id) => {
                let mut evaled_type_params: Vec<TypeId> = Vec::with_capacity(ty_app.params.len());
                let Type::Generic(_) = self.types.get(type_id) else {
                    return ffail!(
                        ty_app.span,
                        "Type '{}' does not take type parameters",
                        self.get_ident_str(ty_app.base)
                    );
                };
                for parsed_param in ty_app.params.clone().iter() {
                    let param_type_id = self.eval_type_expr(*parsed_param, scope_id, None)?;
                    evaled_type_params.push(param_type_id);
                }
                let gen = self.types.get(type_id).expect_generic().clone();
                let mut type_defn_info = gen.type_defn_info.clone();
                type_defn_info.generic_parent = Some(type_id);
                let specialized_type = match gen.specializations.get(&evaled_type_params) {
                    Some(existing) => *existing,
                    None => {
                        let specialized_type = self.substitute_in_type(
                            gen.inner,
                            Some(type_defn_info.clone()),
                            &evaled_type_params,
                            &gen.params,
                            ty_app_id,
                        );
                        match self.types.get_mut(type_id) {
                            Type::Generic(gen) => {
                                gen.specializations.insert(evaled_type_params, specialized_type);
                            }
                            _ => {}
                        };
                        specialized_type
                    }
                };
                Ok(specialized_type)
            }
        }
    }

    fn substitute_in_type(
        &mut self,
        type_id: TypeId,
        defn_info: Option<TypeDefnInfo>,
        passed_params: &[TypeId],
        generic_params: &[GenericTypeParam],
        parsed_expression_id: ParsedTypeExpressionId,
    ) -> TypeId {
        let typ = self.types.get(type_id);
        match typ {
            Type::Struct(struc) => {
                let mut new_fields = struc.fields.clone();
                let mut any_change = false;
                for field in new_fields.iter_mut() {
                    let new_field_type_id = self.substitute_in_type(
                        field.type_id,
                        None,
                        passed_params,
                        generic_params,
                        parsed_expression_id,
                    );
                    if new_field_type_id != field.type_id {
                        any_change = true;
                    }
                    field.type_id = new_field_type_id;
                }
                if any_change {
                    let specialized_struct = StructType {
                        fields: new_fields,
                        type_defn_info: defn_info,
                        ast_node: parsed_expression_id.into(),
                    };
                    self.types.add_type(Type::Struct(specialized_struct))
                } else {
                    type_id
                }
            }
            Type::Optional(opt) => {
                let opt_inner = opt.inner_type;
                let new_inner = self.substitute_in_type(
                    opt_inner,
                    None,
                    passed_params,
                    generic_params,
                    parsed_expression_id,
                );
                if new_inner != opt_inner {
                    let specialized_optional = OptionalType { inner_type: new_inner };
                    self.types.add_type(Type::Optional(specialized_optional))
                } else {
                    type_id
                }
            }
            Type::Reference(reference) => {
                let ref_inner = reference.inner_type;
                let new_inner = self.substitute_in_type(
                    ref_inner,
                    None,
                    passed_params,
                    generic_params,
                    parsed_expression_id,
                );
                if new_inner != ref_inner {
                    let specialized_reference = ReferenceType { inner_type: new_inner };
                    self.types.add_type(Type::Reference(specialized_reference))
                } else {
                    type_id
                }
            }
            Type::Enum(e) => {
                let mut new_variants = e.variants.clone();
                let mut any_changed = false;
                for variant in new_variants.iter_mut() {
                    let new_payload_id = match variant.payload {
                        None => None,
                        Some(payload_type_id) => Some(self.substitute_in_type(
                            payload_type_id,
                            None,
                            passed_params,
                            generic_params,
                            parsed_expression_id,
                        )),
                    };
                    if new_payload_id != variant.payload {
                        any_changed = true;
                        variant.payload = new_payload_id;
                    }
                }
                if any_changed {
                    let new_enum = TypedEnum {
                        variants: new_variants,
                        ast_node: parsed_expression_id.into(),
                        type_defn_info: defn_info,
                    };
                    let new_enum_id = self.types.add_type(Type::Enum(new_enum));
                    new_enum_id
                } else {
                    type_id
                }
            }
            Type::TypeVariable(_t) => {
                let generic_param = generic_params
                    .iter()
                    .enumerate()
                    .find(|(_, gen_param)| gen_param.type_id == type_id);
                match generic_param {
                    None => type_id,
                    Some((param_index, _generic_param)) => {
                        let corresponding_type = passed_params[param_index];
                        eprintln!(
                            "SUBSTITUTING {} for {}",
                            self.type_id_to_string(corresponding_type),
                            self.type_id_to_string(type_id)
                        );
                        corresponding_type
                    }
                }
            }
            _ => todo!("Weird generic type"),
        }
    }

    fn eval_const_type_expr(
        &mut self,
        parsed_type_expr: ParsedTypeExpressionId,
    ) -> TyperResult<TypeId> {
        let ty = self.eval_type_expr(parsed_type_expr, self.scopes.get_root_scope_id(), None)?;
        match ty {
            UNIT_TYPE_ID => Ok(ty),
            CHAR_TYPE_ID => Ok(ty),
            I64_TYPE_ID => Ok(ty),
            BOOL_TYPE_ID => Ok(ty),
            STRING_TYPE_ID => Ok(ty),
            _ => make_fail_span(
                "Only scalar types allowed in constants",
                self.ast.get_type_expression_span(parsed_type_expr),
            ),
        }
    }

    fn eval_pattern(
        &self,
        pat_expr: ParsedPatternId,
        target_type_id: TypeId,
        scope_id: ScopeId,
    ) -> TyperResult<TypedPattern> {
        let parsed_pattern_expr = self.ast.patterns.get_pattern(pat_expr);
        match &*parsed_pattern_expr {
            ParsedPattern::Wildcard(span) => Ok(TypedPattern::Wildcard(*span)),
            ParsedPattern::Literal(literal_expr_id) => {
                match self.ast.expressions.get(*literal_expr_id).expect_literal() {
                    Literal::None(span) => match self.types.get(target_type_id) {
                        Type::Optional(_) => Ok(TypedPattern::LiteralNone(*span)),
                        _ => make_fail_span(
                            "unrelated type will never match none",
                            self.ast.get_pattern_span(pat_expr),
                        ),
                    },
                    Literal::Unit(span) => match self.types.get(target_type_id) {
                        Type::Unit => Ok(TypedPattern::LiteralUnit(*span)),
                        _ => make_fail_span(
                            "unrelated type will never match unit",
                            self.ast.get_pattern_span(pat_expr),
                        ),
                    },
                    Literal::Char(c, span) => match self.types.get(target_type_id) {
                        Type::Char => Ok(TypedPattern::LiteralChar(*c, *span)),
                        _ => make_fail_span(
                            "unrelated type will never match char",
                            self.ast.get_pattern_span(pat_expr),
                        ),
                    },
                    Literal::Integer(int) => {
                        let value = self.eval_integer_value(
                            &int.text,
                            int.span,
                            scope_id,
                            Some(target_type_id),
                        )?;
                        match self.types.get(target_type_id) {
                            Type::Integer(_integer_type) => {
                                Ok(TypedPattern::LiteralInteger(value, int.span))
                            }
                            _ => make_fail_span(
                                "unrelated type will never match int",
                                self.ast.get_pattern_span(pat_expr),
                            ),
                        }
                    }
                    Literal::Bool(b, span) => match self.types.get(target_type_id) {
                        Type::Bool => Ok(TypedPattern::LiteralBool(*b, *span)),
                        _ => make_fail_span(
                            "unrelated type will never match bool",
                            self.ast.get_pattern_span(pat_expr),
                        ),
                    },
                    // Clone would go away if we intern string literals
                    // But I think this is where we'd interpolate and handle escapes and stuff so maybe there's always going
                    // to be an allocation here. Should be same handling as non-pattern string literals
                    Literal::String(s, span) => match self.types.get(target_type_id) {
                        Type::String => Ok(TypedPattern::LiteralString(s.clone(), *span)),
                        _ => make_fail_span(
                            "unrelated type will never match string",
                            self.ast.get_pattern_span(pat_expr),
                        ),
                    },
                }
            }
            ParsedPattern::Variable(ident_id, span) => {
                Ok(TypedPattern::Variable(VariablePattern { ident: *ident_id, span: *span }))
            }
            ParsedPattern::Some(some_pattern) => {
                let Some(optional_type_id) = self.types.get(target_type_id).as_optional() else {
                    return make_fail_span(
                        "Impossible pattern: Expected optional type",
                        some_pattern.span,
                    );
                };
                let inner_pattern = self.eval_pattern(
                    some_pattern.inner_pattern,
                    optional_type_id.inner_type,
                    scope_id,
                )?;
                Ok(TypedPattern::Some(TypedSomePattern {
                    inner_pattern: Box::new(inner_pattern),
                    optional_type_id: target_type_id,
                    span: some_pattern.span,
                }))
            }
            ParsedPattern::Enum(enum_pattern) => {
                let Some(enum_type) = self.types.get(target_type_id).as_enum() else {
                    return make_fail_span(
                        &format!(
                            "Impossible pattern: Expected an enum type; but got {}",
                            self.type_id_to_string(target_type_id)
                        ),
                        enum_pattern.span,
                    );
                };
                let Some(matching_variant) =
                    enum_type.variants.iter().find(|v| v.tag_name == enum_pattern.variant_tag)
                else {
                    return make_fail_span(
                        &format!(
                            "Impossible pattern: No variant named '{}'",
                            self.get_ident_str(enum_pattern.variant_tag).blue()
                        ),
                        enum_pattern.span,
                    );
                };

                let payload_pattern = match &enum_pattern.payload_pattern {
                    None => None,
                    Some(payload_expr) => {
                        let payload_type_id = matching_variant.payload.ok_or(make_error(
                            "Impossible pattern: Enum variant has no payload",
                            enum_pattern.span,
                        ))?;
                        let payload_pattern =
                            self.eval_pattern(*payload_expr, payload_type_id, scope_id)?;
                        Some(Box::new(payload_pattern))
                    }
                };

                let enum_pattern = TypedEnumPattern {
                    enum_type_id: target_type_id,
                    variant_index: matching_variant.index,
                    variant_tag_name: matching_variant.tag_name,
                    payload: payload_pattern,
                    span: enum_pattern.span,
                };
                Ok(TypedPattern::Enum(enum_pattern))
            }
            ParsedPattern::Struct(struct_pattern) => {
                let target_type = self.types.get(target_type_id);
                let expected_struct = target_type.as_struct().ok_or_else(|| {
                    make_error(
                        &format!(
                            "Impossible pattern: Match target '{}' is not a struct",
                            self.type_to_string(target_type)
                        ),
                        struct_pattern.span,
                    )
                })?;
                let mut fields = Vec::with_capacity(struct_pattern.fields.len());
                for (field_name, field_parsed_pattern_id) in &struct_pattern.fields {
                    let expected_field = expected_struct
                        .fields
                        .iter()
                        .find(|f| f.name == *field_name)
                        .ok_or(make_error(
                            format!("Impossible pattern: Struct has no field named {}", field_name),
                            self.ast.get_pattern_span(*field_parsed_pattern_id),
                        ))?;
                    let field_type_id = expected_field.type_id;
                    let field_pattern =
                        self.eval_pattern(*field_parsed_pattern_id, field_type_id, scope_id)?;
                    fields.push(TypedStructPatternField {
                        field_name: *field_name,
                        field_pattern,
                        field_index: expected_field.index,
                        field_type_id: expected_field.type_id,
                    });
                }
                let struct_pattern = TypedStructPattern {
                    struct_type_id: target_type_id,
                    fields,
                    span: struct_pattern.span,
                };
                Ok(TypedPattern::Struct(struct_pattern))
            }
        }
    }

    fn typecheck_struct(
        &self,
        expected: &StructType,
        actual: &StructType,
        scope_id: ScopeId,
    ) -> Result<(), String> {
        if expected.fields.len() != actual.fields.len() {
            return Err(format!(
                "expected struc with {} fields, got {}",
                expected.fields.len(),
                actual.fields.len()
            ));
        }
        for expected_field in &expected.fields {
            trace!("typechecking struc field {:?}", expected_field);
            let Some(matching_field) = actual.fields.iter().find(|f| f.name == expected_field.name)
            else {
                return Err(format!("expected struc to have field {}", expected_field.name));
            };
            self.check_types(expected_field.type_id, matching_field.type_id, scope_id)?;
        }
        Ok(())
    }

    /// This implements 'duck-typing' for structs, which is really cool
    /// but I do not want to do this by default since the codegen involves
    /// either v-tables or monomorphization of functions that accept structs
    /// Maybe a <: syntax to opt-in to dynamic stuff like this, read as "conforms to"
    /// input <: {quack: () -> ()} means that it has at least a quack function
    /// fn takes_quacker = (input <: {quack: () -> ()}) -> ()
    ///
    /// "Conforms To" would mean that it has at least the same fields as the expected type, and
    /// it has them at least as strongly. If an optional is expected, actual can optional or required
    /// If a required is expected, actual must be required, etc. Basically TypeScripts structural typing
    #[allow(unused)]
    fn typecheck_struct_duck(
        &self,
        expected: &StructType,
        actual: &StructType,
        scope_id: ScopeId,
    ) -> Result<(), String> {
        for expected_field in &expected.fields {
            trace!("typechecking struc field {:?}", expected_field);
            let Some(matching_field) = actual.fields.iter().find(|f| f.name == expected_field.name)
            else {
                return Err(format!("expected field {}", expected_field.name));
            };
            self.check_types(matching_field.type_id, expected_field.type_id, scope_id)?;
        }
        Ok(())
    }

    fn check_types(
        &self,
        expected: TypeId,
        actual: TypeId,
        scope_id: ScopeId,
    ) -> Result<(), String> {
        trace!(
            "typecheck expect {} actual {}",
            self.type_id_to_string(expected),
            self.type_id_to_string(actual)
        );
        if expected == actual {
            return Ok(());
        }
        match (self.types.get(expected), self.types.get(actual)) {
            (Type::Optional(o1), Type::Optional(o2)) => {
                self.check_types(o1.inner_type, o2.inner_type, scope_id)
            }
            (Type::Struct(r1), Type::Struct(r2)) => self.typecheck_struct(r1, r2, scope_id),
            (Type::Array(a1), Type::Array(a2)) => {
                self.check_types(a1.element_type, a2.element_type, scope_id)
            }
            (Type::TypeVariable(t1), Type::TypeVariable(t2)) => {
                if t1.name == t2.name {
                    Ok(())
                } else {
                    Err(format!(
                        "expected type variable {} but got {}",
                        &self.ast.identifiers.get_name(t1.name),
                        &self.ast.identifiers.get_name(t2.name)
                    ))
                }
            }
            (Type::Reference(o1), Type::Reference(o2)) => {
                self.check_types(o1.inner_type, o2.inner_type, scope_id)
            }
            (Type::Enum(_expected_enum), Type::EnumVariant(actual_variant)) => {
                if actual_variant.enum_type_id == expected {
                    Ok(())
                } else {
                    Err(format!(
                        "expected enum {} but got variant {} of a different enum",
                        self.type_id_to_string(expected),
                        self.get_ident_str(actual_variant.tag_name)
                    ))
                }
            }
            (_expected, Type::Never) => Ok(()),
            // (Type::OpaqueAlias(opaque), other) => {
            //     eprintln!("Expecting opaque, got other");
            //     Err("opaque".to_string())
            // }
            (exp, act) => {
                // Resolve type variables
                if let Type::TypeVariable(expected_type_var) = exp {
                    if let Some(expected_resolved) =
                        self.scopes.find_type(scope_id, expected_type_var.name)
                    {
                        // We will recursively just resolve to the same type variable without this check
                        // this check requires us to make progress. Doesn't prevent cycles though I guess
                        if expected_resolved != expected {
                            return self.check_types(expected_resolved, actual, scope_id);
                        }
                    }
                }
                if let Type::TypeVariable(actual_type_var) = act {
                    if let Some(actual_resolved) =
                        self.scopes.find_type(scope_id, actual_type_var.name)
                    {
                        // We will recursively just resolve to the same type variable without this check
                        // this check requires us to make progress. Doesn't prevent cycles though I guess
                        if actual_resolved != actual {
                            return self.check_types(expected, actual_resolved, scope_id);
                        }
                        // ?? I remember the above check mattering but it looks like not?
                        return self.check_types(expected, actual_resolved, scope_id);
                    }
                }
                Err(format!(
                    "Expected {} but got {}",
                    self.type_to_string(exp),
                    self.type_to_string(act)
                ))
            }
        }
    }

    fn eval_const(&mut self, parsed_constant_id: ParsedConstantId) -> TyperResult<VariableId> {
        let parsed_constant = self.ast.get_constant(parsed_constant_id);
        let type_id = self.eval_const_type_expr(parsed_constant.ty)?;
        let parsed_constant = self.ast.get_constant(parsed_constant_id);
        let constant_name = parsed_constant.name;
        let constant_span = parsed_constant.span;
        let root_scope_id = self.scopes.get_root_scope_id();
        let expr = match self.ast.expressions.get(parsed_constant.value_expr) {
            ParsedExpression::Literal(Literal::Integer(integer)) => {
                let value = self.eval_integer_value(
                    &integer.text,
                    integer.span,
                    root_scope_id,
                    Some(type_id),
                )?;
                TypedExpr::Integer(TypedIntegerExpr { value, span: integer.span })
            }
            ParsedExpression::Literal(Literal::Bool(b, span)) => TypedExpr::Bool(*b, *span),
            ParsedExpression::Literal(Literal::Char(c, span)) => TypedExpr::Char(*c, *span),
            _other => {
                return make_fail_span(
                    "Only literals are currently supported as constants",
                    parsed_constant.span,
                );
            }
        };
        let variable_id = self.variables.add_variable(Variable {
            name: constant_name,
            type_id,
            is_mutable: false,
            owner_scope: root_scope_id,
        });
        self.constants.push(Constant { variable_id, expr, ty: type_id, span: constant_span });
        self.scopes.add_variable(root_scope_id, constant_name, variable_id);
        Ok(variable_id)
    }

    fn add_function(&mut self, function: TypedFunction) -> FunctionId {
        let id = self.functions.len();
        self.functions.push(function);
        FunctionId(id as u32)
    }

    pub fn get_function(&self, function_id: FunctionId) -> &TypedFunction {
        &self.functions[function_id.0 as usize]
    }

    pub fn get_function_mut(&mut self, function_id: FunctionId) -> &mut TypedFunction {
        &mut self.functions[function_id.0 as usize]
    }

    pub fn add_ability_impl(&mut self, ability_impl: TypedAbilityImpl) -> AbilityImplId {
        let id = self.ability_impls.len();
        self.ability_impls.push(ability_impl);
        AbilityImplId(id as u32)
    }

    pub fn get_ability_impl(&mut self, ability_impl_id: AbilityImplId) -> &TypedAbilityImpl {
        &self.ability_impls[ability_impl_id.0 as usize]
    }

    pub fn get_ability_impl_mut(
        &mut self,
        ability_impl_id: AbilityImplId,
    ) -> &mut TypedAbilityImpl {
        &mut self.ability_impls[ability_impl_id.0 as usize]
    }

    // If the expr is already a block, do nothing
    // If it is not, make a new block with just this expression inside.
    // Used main for if/else
    fn coerce_expr_to_block(&mut self, expr: TypedExpr, scope: ScopeId) -> TypedBlock {
        match expr {
            TypedExpr::Block(b) => b,
            expr => {
                let expr_span = expr.get_span();
                let statements = vec![TypedStmt::Expr(Box::new(expr))];
                self.synth_block(statements, scope, expr_span)
            }
        }
    }

    fn coerce_block_to_unit_block(&mut self, block: &mut TypedBlock) {
        if block.expr_type != UNIT_TYPE_ID {
            let span = block.statements.last().map(|s| s.get_span()).unwrap_or(block.span);
            let unit_literal = TypedExpr::Unit(span);
            block.push_expr(unit_literal);
        }
    }

    fn traverse_namespace_chain(
        &self,
        scope_id: ScopeId,
        namespaces: &[IdentifierId],
        span: SpanId,
    ) -> TyperResult<ScopeId> {
        trace!(
            "traverse_namespace_chain: {:?}",
            namespaces.iter().map(|id| self.get_ident_str(*id).to_string()).collect::<Vec<_>>()
        );
        let ns_iter = namespaces.iter();
        let mut cur_scope = scope_id;
        for ns in ns_iter {
            let namespace_id = self.scopes.find_namespace(cur_scope, *ns).ok_or(make_error(
                format!(
                    "Namespace not found: {} in scope: {:?}",
                    &*self.get_ident_str(*ns),
                    self.scopes.get_scope(scope_id)
                ),
                span,
            ))?;
            let namespace = self.namespaces.get(namespace_id);
            cur_scope = namespace.scope_id;
        }
        Ok(cur_scope)
    }

    fn eval_index_operation(
        &mut self,
        index_op: &IndexOperation,
        scope_id: ScopeId,
    ) -> TyperResult<TypedExpr> {
        let index_expr = self.eval_expr(index_op.index_expr, scope_id, Some(U64_TYPE_ID))?;
        if index_expr.get_type() != U64_TYPE_ID {
            return make_fail_span("index type must be u64", index_op.span);
        }

        let base_expr = self.eval_expr(index_op.target, scope_id, None)?;
        let target_type = base_expr.get_type();
        match target_type {
            STRING_TYPE_ID => Ok(TypedExpr::StringIndex(IndexOp {
                base_expr: Box::new(base_expr),
                index_expr: Box::new(index_expr),
                result_type: CHAR_TYPE_ID,
                span: index_op.span,
            })),
            target_type_id => {
                let target_type = self.types.get(target_type_id);
                match target_type {
                    Type::Array(array_type) => Ok(TypedExpr::ArrayIndex(IndexOp {
                        base_expr: Box::new(base_expr),
                        index_expr: Box::new(index_expr),
                        result_type: array_type.element_type,
                        span: index_op.span,
                    })),
                    _ => make_fail_span("index base must be an array", index_op.span),
                }
            }
        }
    }

    fn eval_integer_value(
        &self,
        parsed_text: &str,
        span: SpanId,
        _scope_id: ScopeId,
        expected_type_id: Option<TypeId>,
    ) -> TyperResult<TypedIntegerValue> {
        // let minus_start = parsed_text.starts_with('-');
        let expected_int_type = match expected_type_id {
            None => IntegerType::I64,
            Some(U8_TYPE_ID) => IntegerType::U8,
            Some(U16_TYPE_ID) => IntegerType::U16,
            Some(U32_TYPE_ID) => IntegerType::U32,
            Some(U64_TYPE_ID) => IntegerType::U64,
            Some(I8_TYPE_ID) => IntegerType::I8,
            Some(I16_TYPE_ID) => IntegerType::I16,
            Some(I32_TYPE_ID) => IntegerType::I32,
            Some(I64_TYPE_ID) => IntegerType::I64,
            // Parse as i64 and let typechecking fail
            Some(_) => IntegerType::I64,
        };
        macro_rules! parse_int {
            ($int_type:ident, $rust_int_type:ty, $base: expr, $offset: expr) => {{
                let result = <$rust_int_type>::from_str_radix(&parsed_text[$offset..], $base);
                result.map(|int| TypedIntegerValue::$int_type(int))
            }};
        }
        if parsed_text.starts_with("0x") {
            let hex_base = 16;
            let offset = 2;
            let value: Result<TypedIntegerValue, std::num::ParseIntError> = match expected_int_type
            {
                IntegerType::U8 => parse_int!(U8, u8, hex_base, offset),
                IntegerType::U16 => parse_int!(U16, u16, hex_base, offset),
                IntegerType::U32 => parse_int!(U32, u32, hex_base, offset),
                IntegerType::U64 => parse_int!(U64, u64, hex_base, offset),
                IntegerType::I8 => parse_int!(I8, i8, hex_base, offset),
                IntegerType::I16 => parse_int!(I16, i16, hex_base, offset),
                IntegerType::I32 => parse_int!(I32, i32, hex_base, offset),
                IntegerType::I64 => parse_int!(I64, i64, hex_base, offset),
            };
            let value = value
                .map_err(|e| make_error(&format!("Invalid hex {expected_int_type}: {e}"), span))?;
            Ok(value)
        } else if parsed_text.starts_with("0b") {
            let bin_base = 2;
            let offset = 2;
            let value: Result<TypedIntegerValue, std::num::ParseIntError> = match expected_int_type
            {
                IntegerType::U8 => parse_int!(U8, u8, bin_base, offset),
                IntegerType::U16 => parse_int!(U16, u16, bin_base, offset),
                IntegerType::U32 => parse_int!(U32, u32, bin_base, offset),
                IntegerType::U64 => parse_int!(U64, u64, bin_base, offset),
                IntegerType::I8 => parse_int!(I8, i8, bin_base, offset),
                IntegerType::I16 => parse_int!(I16, i16, bin_base, offset),
                IntegerType::I32 => parse_int!(I32, i32, bin_base, offset),
                IntegerType::I64 => parse_int!(I64, i64, bin_base, offset),
            };
            let value = value.map_err(|e| {
                make_error(&format!("Invalid binary {expected_int_type}: {e}"), span)
            })?;
            Ok(value)
        } else {
            let dec_base = 10;
            let offset = 0;
            let value: Result<TypedIntegerValue, std::num::ParseIntError> = match expected_int_type
            {
                IntegerType::U8 => parse_int!(U8, u8, dec_base, offset),
                IntegerType::U16 => parse_int!(U16, u16, dec_base, offset),
                IntegerType::U32 => parse_int!(U32, u32, dec_base, offset),
                IntegerType::U64 => parse_int!(U64, u64, dec_base, offset),
                IntegerType::I8 => parse_int!(I8, i8, dec_base, offset),
                IntegerType::I16 => parse_int!(I16, i16, dec_base, offset),
                IntegerType::I32 => parse_int!(I32, i32, dec_base, offset),
                IntegerType::I64 => parse_int!(I64, i64, dec_base, offset),
            };
            let value = value.map_err(|e| {
                make_error(&format!("Invalid integer {expected_int_type}: {e}"), span)
            })?;
            Ok(value)
        }
    }

    fn eval_variable(
        &self,
        variable: &parse::Variable,
        scope_id: ScopeId,
        is_assignment_lhs: bool,
    ) -> TyperResult<TypedExpr> {
        let variable_id = self.scopes.find_variable(scope_id, variable.name).ok_or(make_error(
            format!("{} is not defined", &*self.ast.identifiers.get_name(variable.name)),
            variable.span,
        ))?;
        let v = self.variables.get_variable(variable_id);
        if is_assignment_lhs && !v.is_mutable {
            return make_fail_span(
                format!(
                    "Cannot assign to immutable variable {}",
                    &*self.ast.identifiers.get_name(v.name)
                ),
                variable.span,
            );
        }
        let expr = TypedExpr::Variable(VariableExpr {
            type_id: v.type_id,
            variable_id,
            span: variable.span,
        });
        Ok(expr)
    }

    fn eval_field_access(
        &mut self,
        field_access: &parse::FieldAccess,
        scope_id: ScopeId,
        is_assignment_lhs: bool,
    ) -> TyperResult<TypedExpr> {
        let mut base_expr = self.eval_expr(field_access.base, scope_id, None)?;
        let type_id = base_expr.get_type();
        let base_type = match self.types.get(type_id) {
            Type::Reference(reference_type) => {
                // Auto de-reference everything for field access
                let inner_type = self.types.get(reference_type.inner_type);
                if !is_assignment_lhs {
                    // Dereference the base expression
                    base_expr = TypedExpr::UnaryOp(UnaryOp {
                        kind: UnaryOpKind::Dereference,
                        type_id: reference_type.inner_type,
                        span: base_expr.get_span(),
                        expr: Box::new(base_expr),
                    });
                }
                inner_type
            }
            other => other,
        };
        match base_type {
            Type::Struct(struct_type) => {
                let (field_index, target_field) =
                    struct_type.find_field(field_access.target).ok_or(make_error(
                        format!(
                            "Field {} not found on struct type",
                            &*self.ast.identifiers.get_name(field_access.target)
                        ),
                        field_access.span,
                    ))?;
                Ok(TypedExpr::StructFieldAccess(FieldAccess {
                    base: Box::new(base_expr),
                    target_field: field_access.target,
                    target_field_index: field_index as u32,
                    ty: target_field.type_id,
                    span: field_access.span,
                }))
            }
            Type::EnumVariant(ev) => {
                if self.ast.identifiers.get_name(field_access.target) != "payload" {
                    return make_fail_span(
                        &format!(
                            "Field {} does not exist; try .payload",
                            self.ast.identifiers.get_name(field_access.target)
                        ),
                        field_access.span,
                    );
                }
                let Some(payload_type_id) = ev.payload else {
                    return make_fail_span(
                        &format!(
                            "Variant {} has no payload",
                            self.ast.identifiers.get_name(ev.tag_name)
                        ),
                        field_access.span,
                    );
                };
                Ok(TypedExpr::EnumGetPayload(GetEnumPayload {
                    target_expr: Box::new(base_expr),
                    payload_type_id,
                    variant_name: ev.tag_name,
                    variant_index: ev.index,
                    span: field_access.span,
                }))
            }
            _ => make_fail_span(
                format!(
                    "Cannot access field {} on type: {}",
                    self.ast.identifiers.get_name(field_access.target),
                    self.type_id_to_string(base_expr.get_type())
                ),
                field_access.span,
            ),
        }
    }

    fn eval_assignment_lhs_expr(
        &mut self,
        expr: ParsedExpressionId,
        scope_id: ScopeId,
        _expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        match self.ast.expressions.get(expr) {
            ParsedExpression::IndexOperation(index_op) => {
                self.eval_index_operation(&index_op.clone(), scope_id)
            }
            ParsedExpression::Variable(variable) => self.eval_variable(variable, scope_id, true),
            ParsedExpression::FieldAccess(field_access) => {
                self.eval_field_access(&field_access.clone(), scope_id, true)
            }
            other => {
                make_fail_span(format!("Invalid assignment lhs: {:?}", other), other.get_span())
            }
        }
    }

    /// Used for
    /// - Auto Some()-boxing,
    /// - auto-referencing,
    /// - de-referencing,
    /// - enum construction
    /// - enum variant construction
    /// - Opaque type instance construction if in definition namespace
    /// This is a good place to do this because we just typechecked an expression, and
    /// we know the expected type.
    fn coerce_expression_to_expected_type(
        &self,
        expected_type_id: TypeId,
        expression: TypedExpr,
        scope_id: ScopeId,
    ) -> TypedExpr {
        // For some reason, we used to skip coercion for 'None'. Tests pass
        // with it commented out, but leaving here for future super-sleuthing
        // if let TypedExpr::None(_type_id, _span) = expression {
        //     return expression;
        // }
        match self.types.get(expected_type_id) {
            // TODO: DRY up identical enum and enum_variant cases
            Type::EnumVariant(ev) => match &expression {
                TypedExpr::Tag(tag_expr) => {
                    if ev.tag_name == tag_expr.name {
                        if let Some(_p) = ev.payload {
                            expression
                        } else {
                            debug!("We have a matching variant for coercion: {:?}", ev);
                            let span = tag_expr.span;
                            TypedExpr::EnumConstructor(TypedEnumConstructor {
                                type_id: expected_type_id,
                                variant_name: ev.tag_name,
                                variant_index: ev.index,
                                payload: None,
                                span,
                            })
                        }
                    } else {
                        // We 'fail' by just giving up the coercion and let typechecking fail
                        // better
                        expression
                    }
                }
                _ => expression,
            },
            Type::Enum(e) => match &expression {
                TypedExpr::Tag(tag_expr) => {
                    if let Some(matching_variant) = e.variant_by_name(tag_expr.name) {
                        if let Some(_p) = matching_variant.payload {
                            expression
                        } else {
                            debug!(
                                "We have a matching variant for coercion: {:?}",
                                matching_variant
                            );
                            let span = tag_expr.span;
                            TypedExpr::EnumConstructor(TypedEnumConstructor {
                                type_id: expected_type_id,
                                variant_name: matching_variant.tag_name,
                                variant_index: matching_variant.index,
                                payload: None,
                                span,
                            })
                        }
                    } else {
                        // We 'fail' by just giving up the coercion and let typechecking fail
                        // better
                        expression
                    }
                }
                _ => expression,
            },
            Type::Optional(optional_type) => {
                match self.check_types(optional_type.inner_type, expression.get_type(), scope_id) {
                    Ok(_) => TypedExpr::OptionalSome(OptionalSome {
                        inner_expr: Box::new(expression),
                        type_id: expected_type_id,
                    }),
                    Err(_) => expression,
                }
            }
            Type::OpaqueAlias(opaque) => {
                if !self
                    .is_inside_companion_scope(opaque.type_defn_info.companion_namespace, scope_id)
                {
                    return expression;
                }
                let Ok(_) = self.check_types(opaque.aliasee, expression.get_type(), scope_id)
                else {
                    return expression;
                };
                debug!(
                    "coerce into opaque from {} to {}",
                    self.type_id_to_string(expression.get_type()),
                    self.type_id_to_string(expected_type_id)
                );
                TypedExpr::Cast(TypedCast {
                    span: expression.get_span(),
                    base_expr: Box::new(expression),
                    target_type_id: expected_type_id,
                    cast_type: CastType::KnownNoOp,
                })
            }
            _other => match self.types.get(expression.get_type()) {
                Type::Reference(reference) => {
                    if self.check_types(expected_type_id, reference.inner_type, scope_id).is_ok() {
                        TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::Dereference,
                            type_id: reference.inner_type,
                            span: expression.get_span(),
                            expr: Box::new(expression),
                        })
                    } else {
                        expression
                    }
                }
                Type::OpaqueAlias(expression_opaque) => {
                    if !self.is_inside_companion_scope(
                        expression_opaque.type_defn_info.companion_namespace,
                        scope_id,
                    ) {
                        return expression;
                    }
                    let Ok(_) =
                        self.check_types(expected_type_id, expression_opaque.aliasee, scope_id)
                    else {
                        return expression;
                    };
                    debug!(
                        "coerce out of opaque from {} to {}",
                        self.type_id_to_string(expression.get_type()),
                        self.type_id_to_string(expression_opaque.aliasee)
                    );
                    TypedExpr::Cast(TypedCast {
                        target_type_id: expression_opaque.aliasee,
                        cast_type: CastType::KnownNoOp,
                        span: expression.get_span(),
                        base_expr: Box::new(expression),
                    })
                }
                _ => expression,
            },
        }
    }

    fn is_inside_companion_scope(
        &self,
        companion_namespace: Option<NamespaceId>,
        scope_id: ScopeId,
    ) -> bool {
        if let Some(companion_namespace) = companion_namespace {
            self.is_scope_inside_namespace(companion_namespace, scope_id)
        } else {
            false
        }
    }

    fn is_scope_inside_namespace(&self, namespace_id: NamespaceId, scope_id: ScopeId) -> bool {
        let ns_scope_id = self.namespaces.get_scope(namespace_id);
        self.scopes.scope_has_ancestor(scope_id, ns_scope_id)
    }

    fn eval_expr(
        &mut self,
        expr: ParsedExpressionId,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        let expected_type = match self.ast.expressions.get_type_hint(expr) {
            Some(t) => {
                let type_id = self.eval_type_expr(t, scope_id, None)?;
                Some(type_id)
            }
            None => expected_type,
        };
        let base_result = self.eval_expr_inner(expr, scope_id, expected_type)?;

        if let Some(expected_type_id) = expected_type {
            let coerced =
                self.coerce_expression_to_expected_type(expected_type_id, base_result, scope_id);
            Ok(coerced)
        } else {
            Ok(base_result)
        }
    }

    fn eval_expr_inner(
        &mut self,
        expr_id: ParsedExpressionId,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        trace!(
            "eval_expr: {}: {:?}",
            &self.ast.expression_to_string(expr_id),
            expected_type.map(|t| self.type_id_to_string(t))
        );
        let expr = self.ast.expressions.get(expr_id);
        let result = match expr {
            ParsedExpression::Array(array_expr) => {
                let mut element_type: Option<TypeId> = match expected_type {
                    Some(type_id) => match self.types.get(type_id) {
                        Type::Array(arr) => Ok(Some(arr.element_type)),
                        _ => Ok(None),
                    },
                    None => Ok(None),
                }?;
                let array_expr_span = array_expr.span;
                let elements: Vec<TypedExpr> = {
                    let mut elements = Vec::new();
                    for elem in &array_expr.elements.clone() {
                        let typed_expr = self.eval_expr(*elem, scope_id, element_type)?;
                        if element_type.is_none() {
                            element_type = Some(typed_expr.get_type())
                        };
                        elements.push(typed_expr);
                    }
                    elements
                };

                let element_type = element_type.expect("By now this should be populated");
                let type_id = match expected_type {
                    Some(t) => t,
                    None => {
                        let array_type = ArrayType { element_type };
                        self.types.add_type(Type::Array(array_type))
                    }
                };
                Ok(TypedExpr::Array(ArrayLiteral { elements, type_id, span: array_expr_span }))
            }
            ParsedExpression::IndexOperation(index_op) => {
                self.eval_index_operation(&index_op.clone(), scope_id)
            }
            ParsedExpression::Struct(ast_struct) => {
                let mut field_values = Vec::new();
                let mut field_defns = Vec::new();
                let ast_struct = ast_struct.clone();
                let expected_struct = if let Some(expected_type) = expected_type {
                    match self.types.get(expected_type) {
                        Type::Struct(struc) => Some((expected_type, struc.clone())),
                        Type::Optional(opt) => match self.types.get(opt.inner_type) {
                            Type::Struct(struc) => Some((opt.inner_type, struc.clone())),
                            other_ty => {
                                return make_fail_span(
                                    format!(
                                        "Got struct literal but expected {}",
                                        self.type_to_string(other_ty)
                                    ),
                                    ast_struct.span,
                                );
                            }
                        },
                        Type::Reference(refer) => match self.types.get(refer.inner_type) {
                            Type::Struct(struc) => Some((refer.inner_type, struc.clone())),
                            other_ty => {
                                return make_fail_span(
                                    format!(
                                        "Got struct literal but expected {}",
                                        self.type_to_string(other_ty)
                                    ),
                                    ast_struct.span,
                                );
                            }
                        },
                        other_ty => {
                            return make_fail_span(
                                format!(
                                    "Got struct literal but expected {}",
                                    self.type_to_string(other_ty)
                                ),
                                ast_struct.span,
                            );
                        }
                    }
                } else {
                    None
                };
                for (index, ast_field) in ast_struct.fields.iter().enumerate() {
                    let expected_field = expected_struct
                        .as_ref()
                        .and_then(|(_, rec)| rec.find_field(ast_field.name));
                    let expected_type_id = expected_field.map(|(_, f)| f.type_id);
                    let expr = self.eval_expr(ast_field.expr, scope_id, expected_type_id)?;
                    field_defns.push(StructTypeField {
                        name: ast_field.name,
                        type_id: expr.get_type(),
                        index: index as u32,
                    });
                    field_values.push(StructField { name: ast_field.name, expr });
                }
                // We can use 'expected type' here to just go ahead and typecheck or fail
                // rather than make a duplicate type
                let struct_type_id = match expected_struct {
                    None => {
                        let struct_type = StructType {
                            fields: field_defns,
                            type_defn_info: None,
                            ast_node: expr_id.into(),
                        };
                        let anon_struct_type_id = self.types.add_type(Type::Struct(struct_type));
                        Ok(anon_struct_type_id)
                    }
                    Some((expected_type_id, expected_struct)) => {
                        match self.typecheck_struct(
                            &expected_struct,
                            &StructType {
                                fields: field_defns,
                                type_defn_info: None,
                                ast_node: expr_id.into(),
                            },
                            scope_id,
                        ) {
                            Ok(_) => Ok(expected_type_id),
                            Err(s) => make_fail_span(
                                format!("Invalid struc type: {}", s),
                                ast_struct.span,
                            ),
                        }
                    }
                }?;
                let typed_struct =
                    Struct { fields: field_values, span: ast_struct.span, type_id: struct_type_id };
                let expr = TypedExpr::Struct(typed_struct);
                trace!("generated struc: {}", self.expr_to_string(&expr));
                Ok(expr)
            }
            ParsedExpression::If(if_expr) => {
                self.eval_if_expr(&if_expr.clone(), scope_id, expected_type)
            }
            ParsedExpression::BinaryOp(binary_op) => {
                let binary_op = binary_op.clone();
                self.eval_binary_op(&binary_op, scope_id, expected_type)
            }
            ParsedExpression::UnaryOp(op) => {
                let op = op.clone();
                let base_expr = self.eval_expr(op.expr, scope_id, None)?;
                match op.op_kind {
                    ParsedUnaryOpKind::Dereference => {
                        let reference_type =
                            self.types.get(base_expr.get_type()).as_reference().ok_or(
                                make_error(
                                    format!(
                                        "Cannot dereference non-reference type: {}",
                                        self.type_id_to_string(base_expr.get_type())
                                    ),
                                    op.span,
                                ),
                            )?;
                        debug!(
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
                    ParsedUnaryOpKind::Reference => {
                        let type_id = self.types.add_type(Type::Reference(ReferenceType {
                            inner_type: base_expr.get_type(),
                        }));
                        Ok(TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::Reference,
                            type_id,
                            expr: Box::new(base_expr),
                            span: op.span,
                        }))
                    }
                    ParsedUnaryOpKind::BooleanNegation => {
                        self.check_types(BOOL_TYPE_ID, base_expr.get_type(), scope_id)
                            .map_err(|s| make_error(s, op.span))?;
                        Ok(TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::BooleanNegation,
                            type_id: BOOL_TYPE_ID,
                            expr: Box::new(base_expr),
                            span: op.span,
                        }))
                    }
                }
            }
            ParsedExpression::Literal(Literal::Unit(span)) => Ok(TypedExpr::Unit(*span)),
            ParsedExpression::Literal(Literal::None(span)) => {
                // If we are expecting an Option, I need to reach inside it to get the inner type
                let expected_type = expected_type.ok_or(make_error(
                    "Cannot infer type of None literal without type hint",
                    *span,
                ))?;
                let expected_type =
                    self.types.get(expected_type).as_optional().ok_or(make_error(
                        format!(
                            "Expected optional type for None literal but got {:?}",
                            expected_type
                        ),
                        *span,
                    ))?;
                let inner_type = expected_type.inner_type;
                let none_type = Type::Optional(OptionalType { inner_type });
                let type_id = self.types.add_type(none_type);
                Ok(TypedExpr::None(type_id, *span))
            }
            ParsedExpression::Literal(Literal::Char(byte, span)) => {
                Ok(TypedExpr::Char(*byte, *span))
            }
            ParsedExpression::Literal(Literal::Integer(int)) => {
                let value =
                    self.eval_integer_value(&int.text, int.span, scope_id, expected_type)?;
                Ok(TypedExpr::Integer(TypedIntegerExpr { value, span: int.span }))
            }
            ParsedExpression::Literal(Literal::Bool(b, span)) => {
                let expr = TypedExpr::Bool(*b, *span);
                Ok(expr)
            }
            ParsedExpression::Literal(Literal::String(s, span)) => {
                // So sad, we could just point to the source. BUT if we ever do escaping and things
                // then the source string is not the same as the string the user meant; perhaps here
                // is the place, or maybe in the parser, that we would actually do some work, which
                // would justify storing it separately. But then, post-transform, we should intern
                // these
                let expr = TypedExpr::Str(s.clone(), *span);
                Ok(expr)
            }
            ParsedExpression::Variable(variable) => self.eval_variable(variable, scope_id, false),
            ParsedExpression::FieldAccess(field_access) => {
                let field_access = field_access.clone();
                self.eval_field_access(&field_access, scope_id, false)
            }
            ParsedExpression::Block(block) => {
                // This clone is actually sad because Block is still big. We need to intern blocks
                let block = block.clone();
                let block = self.eval_block(&block, scope_id, expected_type)?;
                Ok(TypedExpr::Block(block))
            }
            ParsedExpression::FnCall(fn_call) => {
                let call = self.eval_function_call(&fn_call.clone(), None, None, None, scope_id)?;
                Ok(call)
            }
            ParsedExpression::OptionalGet(optional_get) => {
                let span = optional_get.span;
                let base = self.eval_expr_inner(optional_get.base, scope_id, expected_type)?;
                let Type::Optional(optional_type) = self.types.get(base.get_type()) else {
                    return make_fail_span(
                        format!(
                            "Cannot get value with ! from non-optional type: {}",
                            self.type_id_to_string(base.get_type())
                        ),
                        span,
                    );
                };
                Ok(TypedExpr::OptionalGet(OptionalGet {
                    inner_expr: Box::new(base),
                    result_type_id: optional_type.inner_type,
                    span,
                    checked: true,
                }))
            }
            ParsedExpression::For(for_expr) => {
                self.eval_for_expr(&for_expr.clone(), scope_id, expected_type)
            }
            ParsedExpression::Tag(tag_expr) => {
                let type_id = self.types.get_type_for_tag(tag_expr.tag);
                let typed_expr = TypedExpr::Tag(TypedTagExpr {
                    name: tag_expr.tag,
                    type_id,
                    span: tag_expr.span,
                });
                Ok(typed_expr)
            }
            ParsedExpression::EnumConstructor(e) => {
                let span = e.span;
                let expected_type = expected_type
                    .ok_or(make_error("Could not infer enum type from context", e.span))?;
                let enum_type = {
                    let expected_type = self.types.get(expected_type);
                    match expected_type {
                        Type::Enum(e) => Ok(e),
                        Type::EnumVariant(ev) => Ok(self.types.get(ev.enum_type_id).expect_enum()),
                        _ => Err(make_error("Expected an enum type", e.span)),
                    }
                }?;
                let typed_variant = enum_type.variant_by_name(e.tag).ok_or(make_error(
                    format!(
                        "No variant {} exists in enum {}",
                        &*self.ast.identifiers.get_name(e.tag),
                        self.type_id_to_string(expected_type)
                    ),
                    e.span,
                ))?;
                let variant_payload = typed_variant.payload.ok_or(make_error(
                    format!(
                        "Variant {} does not have a payload",
                        &*self.ast.identifiers.get_name(e.tag)
                    ),
                    e.span,
                ))?;
                let variant_index = typed_variant.index;
                let variant_name = typed_variant.tag_name;
                // drop(typed_variant);
                let payload_expr = self.eval_expr(e.payload, scope_id, Some(variant_payload))?;
                if let Err(msg) =
                    self.check_types(variant_payload, payload_expr.get_type(), scope_id)
                {
                    return make_fail_span(&format!("Payload type mismatch: {}", msg), span);
                };
                Ok(TypedExpr::EnumConstructor(TypedEnumConstructor {
                    type_id: expected_type,
                    payload: Some(Box::new(payload_expr)),
                    variant_index,
                    variant_name,
                    span,
                }))
            }
            ParsedExpression::Is(is_expr) => {
                let is_expr = is_expr.clone();
                // If the 'is' is attached to an if/else, that is handled by if/else
                // This is just the case of the detached 'is' where we want to return a boolean
                // indicating whether or not the pattern matched only
                let true_expression = self.ast.expressions.add_expression(
                    parse::ParsedExpression::Literal(parse::Literal::Bool(true, is_expr.span)),
                );
                let false_expression = self.ast.expressions.add_expression(
                    parse::ParsedExpression::Literal(parse::Literal::Bool(false, is_expr.span)),
                );
                let true_case = parse::ParsedMatchCase {
                    pattern: is_expr.pattern,
                    expression: true_expression,
                };
                let wildcard_pattern =
                    self.ast.patterns.add_pattern(parse::ParsedPattern::Wildcard(is_expr.span));
                let false_case = parse::ParsedMatchCase {
                    pattern: wildcard_pattern,
                    expression: false_expression,
                };
                let as_match_expr = parse::ParsedMatchExpression {
                    target_expression: is_expr.target_expression,
                    cases: vec![true_case, false_case],
                    span: is_expr.span,
                };
                let match_expr_id = self
                    .ast
                    .expressions
                    .add_expression(parse::ParsedExpression::Match(as_match_expr));
                self.eval_expr(match_expr_id, scope_id, expected_type)
            }
            ParsedExpression::Match(_match_expr) => {
                self.eval_match_expr(expr_id, scope_id, expected_type)
            }
            ParsedExpression::AsCast(cast) => {
                let cast = cast.clone();
                let base_expr = self.eval_expr(cast.base_expr, scope_id, None)?;
                let base_expr_type = base_expr.get_type();
                let target_type = self.eval_type_expr(cast.dest_type, scope_id, None)?;
                let cast_type = match self.types.get(base_expr_type) {
                    Type::Integer(from_integer_type) => match self.types.get(target_type) {
                        Type::Integer(to_integer_type) => {
                            let cast_type = match from_integer_type
                                .bit_width()
                                .cmp(&to_integer_type.bit_width())
                            {
                                Ordering::Less => CastType::IntegerExtend,
                                Ordering::Greater => CastType::IntegerTruncate,
                                Ordering::Equal => CastType::KnownNoOp,
                            };
                            Ok(cast_type)
                        }
                        Type::Char => {
                            if from_integer_type.bit_width() == 8 {
                                Ok(CastType::Integer8ToChar)
                            } else {
                                ffail!(
                                    cast.span,
                                    "Cannot cast integer '{}' to char, must be 8 bits",
                                    from_integer_type
                                )
                            }
                        }
                        _ => ffail!(
                            cast.span,
                            "Cannot cast integer '{}' to '{}'",
                            from_integer_type,
                            self.type_id_to_string(target_type).blue()
                        ),
                    },
                    Type::Char => match self.types.get(target_type) {
                        Type::Integer(_to_integer_type) => Ok(CastType::IntegerExtendFromChar),
                        _ => ffail!(
                            cast.span,
                            "Cannot cast char to '{}'",
                            self.type_id_to_string(target_type).blue()
                        ),
                    },
                    Type::Enum(_e) => {
                        match self.types.get(target_type) {
                            Type::EnumVariant(variant) => {
                                // Enum cast to variant
                                if variant.enum_type_id == base_expr_type {
                                    Ok(CastType::EnumVariant)
                                } else {
                                    ffail!(
                                        cast.span,
                                        "Cannot cast enum '{}' to '{}'",
                                        self.type_id_to_string(base_expr_type).blue(),
                                        self.type_id_to_string(variant.enum_type_id).blue()
                                    )
                                }
                            }
                            _ => ffail!(
                                cast.span,
                                "Cannot cast enum '{}' to '{}'",
                                self.type_id_to_string(base_expr_type).blue(),
                                self.type_id_to_string(target_type).blue()
                            ),
                        }
                    }
                    _ => ffail!(
                        cast.span,
                        "Cannot cast '{}' to '{}'",
                        self.type_id_to_string(base_expr_type).blue(),
                        self.type_id_to_string(target_type).blue()
                    ),
                }?;
                Ok(TypedExpr::Cast(TypedCast {
                    base_expr: Box::new(base_expr),
                    target_type_id: target_type,
                    cast_type,
                    span: cast.span,
                }))
            }
        };
        result
    }

    fn eval_match_expr(
        &mut self,
        match_expr_id: ParsedExpressionId,
        scope_id: ScopeId,
        expected_type_id: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        let target_expr = {
            let match_expr = self.ast.expressions.get(match_expr_id).as_match().unwrap();
            if match_expr.cases.is_empty() {
                return Err(make_error("Match expression with no arms", match_expr.span));
            }
            self.eval_expr(match_expr.target_expression, scope_id, None)?
        };

        let match_block_scope_id =
            self.scopes.add_child_scope(scope_id, ScopeType::LexicalBlock, None, None);

        // Mangled; not a user-facing binding
        let match_target_ident = self.ast.identifiers.intern("match_target");
        let (_target_expr_variable_id, target_expr_decl_stmt, target_expr_variable_expr) =
            self.synth_variable_decl(match_target_ident, target_expr, false, false, scope_id);

        // Reborrow from ast
        let match_expr = self.ast.expressions.get(match_expr_id).as_match().unwrap();
        let match_expr_span = match_expr.span;
        let arms = self.eval_match_arms(
            target_expr_variable_expr.expect_variable(),
            &match_expr.cases.clone(),
            match_block_scope_id,
            expected_type_id,
        )?;

        let match_result_type = arms[0].arm_block.expr_type;
        let mut resulting_block = TypedBlock {
            expr_type: match_result_type,
            scope_id: match_block_scope_id,
            statements: vec![target_expr_decl_stmt],
            span: match_expr_span,
        };
        let mut pre_stmts: Vec<TypedStmt> = Vec::new();
        let mut the_arms = VecDeque::new();
        for match_case in arms.into_iter() {
            pre_stmts.extend(match_case.pre_stmts);
            the_arms.push_back((match_case.condition, match_case.arm_block));
        }
        let if_chain = self.chain_match_cases(
            match_result_type,
            the_arms,
            match_block_scope_id,
            match_expr_span,
        )?;
        resulting_block.statements.extend(pre_stmts);
        resulting_block.push_stmt(TypedStmt::Expr(Box::new(if_chain)));
        debug!("match result\n{}", self.block_to_string(&resulting_block));
        let result = TypedExpr::Block(resulting_block);
        Ok(result)
    }

    fn chain_match_cases(
        &mut self,
        match_result_type: TypeId,
        mut cases: VecDeque<(TypedExpr, TypedBlock)>,
        scope_id: ScopeId,
        span_if_no_cases: SpanId,
    ) -> TyperResult<TypedExpr> {
        if cases.is_empty() {
            let message_expr = TypedExpr::Str("Match Error".to_string(), span_if_no_cases);
            self.synth_function_call(
                vec![],
                self.ast.identifiers.get("crash").unwrap(),
                None,
                vec![message_expr],
                span_if_no_cases,
                scope_id,
            )
        } else {
            let (condition_expr, block) = cases.pop_front().unwrap();
            // Optimize out 'if true' and 'if false'
            if let TypedExpr::Bool(b, _) = condition_expr {
                if b {
                    Ok(TypedExpr::Block(block))
                } else {
                    self.chain_match_cases(
                        match_result_type,
                        cases,
                        scope_id,
                        condition_expr.get_span(),
                    )
                }
            } else {
                let res = self.chain_match_cases(
                    match_result_type,
                    cases,
                    scope_id,
                    condition_expr.get_span(),
                )?;
                let alternate = self.coerce_expr_to_block(res, scope_id);
                Ok(TypedExpr::If(Box::new(TypedIf {
                    span: condition_expr.get_span(),
                    condition: condition_expr,
                    consequent: block,
                    alternate,
                    ty: match_result_type,
                })))
            }
        }
    }

    fn eval_match_arms(
        &mut self,
        target_expr: VariableExpr,
        cases: &[parse::ParsedMatchCase],
        match_scope_id: ScopeId,
        expected_type_id: Option<TypeId>,
    ) -> TyperResult<Vec<TypedMatchCase>> {
        let mut typed_cases: Vec<TypedMatchCase> = Vec::new();

        let mut expected_arm_type_id = expected_type_id;

        for parsed_case in cases.iter() {
            let pattern =
                self.eval_pattern(parsed_case.pattern, target_expr.type_id, match_scope_id)?;
            let arm_expr_span = self.ast.expressions.get(parsed_case.expression).get_span();
            let mut arm_block = self.synth_block(vec![], match_scope_id, arm_expr_span);
            let (pre_stmts, condition) =
                self.eval_match_arm(&pattern, target_expr.clone(), &mut arm_block, match_scope_id)?;

            // Once we've evaluated the arm pattern, we can eval the consequent expression inside of it,
            // since the bindings are now in scope inside arm_block
            let arm_expr =
                self.eval_expr(parsed_case.expression, arm_block.scope_id, expected_arm_type_id)?;

            if let Some(expected_arm_type_id) = expected_arm_type_id.as_ref() {
                // Never is divergent so need not contribute to the overall type of the pattern
                if arm_expr.get_type() != NEVER_TYPE_ID {
                    if let Err(msg) =
                        self.check_types(*expected_arm_type_id, arm_expr.get_type(), match_scope_id)
                    {
                        return make_fail_span(
                            &format!("Mismatching type for match case: {}", msg),
                            arm_expr_span,
                        );
                    }
                }
            }

            arm_block.push_expr(arm_expr);

            expected_arm_type_id = Some(arm_block.expr_type);
            typed_cases.push(TypedMatchCase { pattern, pre_stmts, condition, arm_block });
        }

        // Exhaustiveness checking

        //  Warnings for pattern matching
        //  LUC MARANGET
        //  We'll try to construct a pattern input that does not get matched. We'll remember which patterns handle which inputs, such that we can
        // also report useless patterns. If any input gets through, that is our witness of in-exhaustiveness
        // let mut unhandled_cases = match self.types.get(target_expr.type_id) {
        //     Type::Unit => vec!["()"],
        //     Type::Char => {
        //         vec![]
        //     }
        //     Type::Int => {
        //         vec![]
        //     }
        //     Type::Bool => {
        //         vec!["true", "false"]
        //     }
        //     Type::String => vec![],
        //     Type::Struct(_) => vec![],
        //     Type::Array(_) => vec![],
        //     Type::Optional(_) => {
        //         vec!["Some", "None"]
        //     }
        //     Type::Reference(_) => todo!(),
        //     Type::TypeVariable(_) => todo!(),
        //     Type::TagInstance(_) => todo!(),
        //     Type::Enum(_) => todo!(),
        //     Type::EnumVariant(_) => todo!(),
        // };
        // for c in typed_cases.iter() {
        //     match &c.pattern {
        //         TypedPattern::LiteralBool(b, _) => {
        //             unhandled_cases.retain(|c2| -> bool {
        //                 if *b && *c2 == "true" {
        //                     false
        //                 } else if !*b && *c2 == "false" {
        //                     false
        //                 } else {
        //                     true
        //                 }
        //             });
        //         }
        //         TypedPattern::Variable(_) => unhandled_cases.clear(),
        //         TypedPattern::Wildcard(_) => unhandled_cases.clear(),
        //         _ => todo!("exhaustiveness checking for all matchable types"),
        //     }
        // }
        // if !unhandled_cases.is_empty() {
        //     return make_fail_span(
        //         &format!("Unhandled cases: {:?}", unhandled_cases),
        //         target_expr.span,
        //     );
        // }
        // End Exhaustiveness checking

        Ok(typed_cases)
    }

    /// For each match case we output
    /// 1) a series of statements to bind variables that are used by the condition
    /// TODO: These conditions should be in their own block; currently they pollute the match block
    /// 2) an expr that is expected to a boolean, representing the condition of the branch,
    fn eval_match_arm(
        &mut self,
        pattern: &TypedPattern,
        target_expr_variable_expr: VariableExpr,
        arm_block: &mut TypedBlock,
        match_scope_id: ScopeId,
    ) -> TyperResult<(Vec<TypedStmt>, TypedExpr)> {
        match pattern {
            TypedPattern::Struct(struct_pattern) => {
                let mut boolean_exprs: Vec<TypedExpr> =
                    Vec::with_capacity(struct_pattern.fields.len());
                let mut condition_statements: Vec<TypedStmt> =
                    Vec::with_capacity(struct_pattern.fields.len());
                for pattern_field in struct_pattern.fields.iter() {
                    let target_value = TypedExpr::StructFieldAccess(FieldAccess {
                        base: Box::new(TypedExpr::Variable(target_expr_variable_expr.clone())),
                        target_field: pattern_field.field_name,
                        target_field_index: pattern_field.field_index,
                        ty: pattern_field.field_type_id,
                        span: struct_pattern.span,
                    });
                    // I'm putting condition variables in the match scope id, but they really belong in the arms condition scope id, which
                    // we'll make later. This is just more hygenic since the variables needed for each arms condition shouldn't be visible
                    // to the other arms, even though mangled and unique... Maybe this is better because its harmless and more efficient, idk
                    let (
                        _struct_member_value_variable_id,
                        target_value_decl_stmt,
                        struct_member_value_expr,
                    ) = self.synth_variable_decl(
                        pattern_field.field_name, // Will be mangled
                        target_value,
                        false,
                        false,
                        match_scope_id,
                    );
                    condition_statements.push(target_value_decl_stmt);

                    let (inner_condition_stmts, condition) = self.eval_match_arm(
                        &pattern_field.field_pattern,
                        struct_member_value_expr.expect_variable(),
                        arm_block,
                        arm_block.scope_id,
                    )?;
                    condition_statements.extend(inner_condition_stmts);

                    boolean_exprs.push(condition);
                }
                let final_condition: TypedExpr = boolean_exprs
                    .into_iter()
                    .reduce(|acc, expr| {
                        TypedExpr::BinaryOp(BinaryOp {
                            kind: BinaryOpKind::And,
                            ty: BOOL_TYPE_ID,
                            lhs: Box::new(acc),
                            rhs: Box::new(expr),
                            span: struct_pattern.span,
                        })
                    })
                    .unwrap();
                Ok((condition_statements, final_condition))
            }
            TypedPattern::Some(some_pattern) => {
                let contained_type =
                    self.types.get(some_pattern.optional_type_id).expect_optional().inner_type;

                // Unchecked since the matching codegen ensures the optional has a value
                let inner_value = TypedExpr::OptionalGet(OptionalGet {
                    inner_expr: Box::new(target_expr_variable_expr.clone().into()),
                    result_type_id: contained_type,
                    span: some_pattern.span,
                    checked: false,
                });
                let optional_get_ident = self.ast.identifiers.intern("optional_get");
                let (_optional_value_variable_id, binding_stmt, optional_value_variable_expr) =
                    self.synth_variable_decl(
                        optional_get_ident,
                        inner_value,
                        false,
                        false,
                        arm_block.scope_id,
                    );
                let (inner_condition_stmts, inner_condition) = self.eval_match_arm(
                    &some_pattern.inner_pattern,
                    optional_value_variable_expr.expect_variable(),
                    arm_block,
                    arm_block.scope_id,
                )?;
                let mut condition_statements = vec![binding_stmt];
                condition_statements.extend(inner_condition_stmts);
                let has_value_expr = TypedExpr::OptionalHasValue(Box::new(TypedExpr::Variable(
                    target_expr_variable_expr.clone(),
                )));
                let final_condition = TypedExpr::BinaryOp(BinaryOp {
                    kind: BinaryOpKind::And,
                    ty: BOOL_TYPE_ID,
                    lhs: Box::new(has_value_expr),
                    rhs: Box::new(inner_condition),
                    span: some_pattern.span,
                });
                Ok((condition_statements, final_condition))
            }
            TypedPattern::Enum(enum_pattern) => {
                // Check tag, that's our condition. Recurse on nested pattern if true
                let is_variant_condition = TypedExpr::EnumIsVariant(TypedEnumIsVariantExpr {
                    target_expr: Box::new(target_expr_variable_expr.clone().into()),
                    variant_name: enum_pattern.variant_tag_name,
                    variant_index: enum_pattern.variant_index,
                    span: enum_pattern.span,
                });
                let mut condition_statements = vec![];
                let (inner_condition_stmts, inner_condition) = if let Some(payload_pattern) =
                    enum_pattern.payload.as_ref()
                {
                    let enum_type = self.types.get(enum_pattern.enum_type_id).expect_enum();
                    let variant = enum_type.variant_by_index(enum_pattern.variant_index).unwrap();
                    let Some(payload_type_id) = variant.payload else {
                        return make_fail_span(
                            "Impossible pattern: variant does not have payload",
                            enum_pattern.span,
                        );
                    };
                    let payload_value = TypedExpr::EnumGetPayload(GetEnumPayload {
                        target_expr: Box::new(target_expr_variable_expr.into()),
                        payload_type_id,
                        variant_name: variant.tag_name,
                        variant_index: variant.index,
                        span: enum_pattern.span,
                    });
                    let payload_value_synth_name = self.ast.identifiers.intern("payload");
                    let (_, payload_value_stmt, payload_value_variable_expr) = self
                        .synth_variable_decl(
                            payload_value_synth_name,
                            payload_value,
                            false,
                            false,
                            arm_block.scope_id,
                        );
                    let mut stmts = vec![payload_value_stmt];
                    let (inner_pattern_stmts, cond) = self.eval_match_arm(
                        &payload_pattern,
                        payload_value_variable_expr.expect_variable(),
                        arm_block,
                        arm_block.scope_id,
                    )?;
                    stmts.extend(inner_pattern_stmts);
                    (stmts, Some(cond))
                } else {
                    (vec![], None)
                };
                condition_statements.extend(inner_condition_stmts);
                let final_condition = match inner_condition {
                    None => is_variant_condition,
                    Some(inner_condition) => TypedExpr::BinaryOp(BinaryOp {
                        kind: BinaryOpKind::And,
                        ty: BOOL_TYPE_ID,
                        lhs: Box::new(is_variant_condition),
                        rhs: Box::new(inner_condition),
                        span: enum_pattern.span,
                    }),
                };
                Ok((condition_statements, final_condition))
            }
            TypedPattern::Variable(variable_pattern) => {
                let variable_ident = variable_pattern.ident;
                let (_variable_id, binding_stmt, _typed_expr) = self.synth_variable_decl(
                    variable_ident,
                    target_expr_variable_expr.into(),
                    true,
                    false,
                    arm_block.scope_id,
                );
                arm_block.push_stmt(binding_stmt);
                // `true` because variable patterns always match
                Ok((vec![], TypedExpr::Bool(true, variable_pattern.span)))
            }
            TypedPattern::LiteralUnit(span) => Ok((vec![], TypedExpr::Bool(true, *span))),
            TypedPattern::LiteralChar(byte, span) => {
                let bin_op = self.synth_equals_binop(
                    target_expr_variable_expr.into(),
                    TypedExpr::Char(*byte, *span),
                    *span,
                );
                Ok((vec![], bin_op))
            }
            TypedPattern::LiteralInteger(int_value, span) => {
                let bin_op = BinaryOp {
                    kind: BinaryOpKind::Equals,
                    ty: BOOL_TYPE_ID,
                    lhs: Box::new(target_expr_variable_expr.into()),
                    rhs: Box::new(TypedExpr::Integer(TypedIntegerExpr {
                        value: int_value.clone(),
                        span: *span,
                    })),
                    span: *span,
                };
                Ok((vec![], TypedExpr::BinaryOp(bin_op)))
            }
            TypedPattern::LiteralBool(bool_value, span) => {
                let bin_op = self.synth_equals_binop(
                    target_expr_variable_expr.into(),
                    TypedExpr::Bool(*bool_value, *span),
                    *span,
                );
                Ok((vec![], bin_op))
            }
            TypedPattern::LiteralString(string_value, span) => {
                let condition = self.synth_equals_call(
                    target_expr_variable_expr.into(),
                    TypedExpr::Str(string_value.clone(), *span),
                    *span,
                )?;
                Ok((vec![], condition))
            }
            TypedPattern::LiteralNone(span) => {
                let option_has_value =
                    TypedExpr::OptionalHasValue(Box::new(target_expr_variable_expr.into()));
                let optional_is_none = TypedExpr::UnaryOp(UnaryOp {
                    kind: UnaryOpKind::BooleanNegation,
                    type_id: BOOL_TYPE_ID,
                    expr: Box::new(option_has_value),
                    span: *span,
                });
                Ok((vec![], optional_is_none))
            }
            TypedPattern::Wildcard(span) => Ok((vec![], TypedExpr::Bool(true, *span))),
        }
    }

    fn synth_equals_binop(&self, lhs: TypedExpr, rhs: TypedExpr, span: SpanId) -> TypedExpr {
        TypedExpr::BinaryOp(BinaryOp {
            kind: BinaryOpKind::Equals,
            ty: BOOL_TYPE_ID,
            span,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    fn synth_block(
        &mut self,
        statements: Vec<TypedStmt>,
        parent_scope: ScopeId,
        span: SpanId,
    ) -> TypedBlock {
        let expr_type = match statements.last() {
            Some(stmt) => stmt.get_type(),
            _ => UNIT_TYPE_ID,
        };
        let block_scope_id =
            self.scopes.add_child_scope(parent_scope, ScopeType::LexicalBlock, None, None);
        let block = TypedBlock {
            expr_type,
            statements: statements.to_vec(),
            scope_id: block_scope_id,
            span,
        };
        block
    }

    /// no_mangle: Skip mangling if we want the variable to be accessible from user code
    fn synth_variable_decl(
        &mut self,
        name: IdentifierId,
        initializer: TypedExpr,
        no_mangle: bool,
        is_mutable: bool,
        owner_scope: ScopeId,
    ) -> (VariableId, TypedStmt, TypedExpr) {
        let type_id = initializer.get_type();
        let span = initializer.get_span();
        let new_ident = if no_mangle {
            name
        } else {
            let new_ident_name =
                format!("__{}_{}", self.ast.identifiers.get_name(name), owner_scope);
            self.ast.identifiers.intern(new_ident_name)
        };
        let variable =
            Variable { name: new_ident, is_mutable, owner_scope, type_id: initializer.get_type() };
        let variable_id = self.variables.add_variable(variable);
        let expr = VariableExpr { type_id, variable_id, span };
        let val_def =
            TypedStmt::ValDef(Box::new(ValDef { variable_id, ty: type_id, initializer, span }));
        self.scopes.add_variable(owner_scope, new_ident, variable_id);
        (variable_id, val_def, TypedExpr::Variable(expr))
    }

    fn synth_function_call(
        &mut self,
        namespaces: Vec<IdentifierId>,
        name: IdentifierId,
        known_type_args: Option<Vec<TypeId>>,
        value_arg_exprs: Vec<TypedExpr>,
        span: SpanId,
        scope_id: ScopeId,
    ) -> TyperResult<TypedExpr> {
        self.eval_function_call(
            &FnCall {
                name,
                type_args: None,
                args: Vec::with_capacity(0),
                namespaces,
                span,
                is_method: false,
            },
            None,
            known_type_args,
            Some(value_arg_exprs),
            scope_id,
        )
    }

    fn eval_for_expr(
        &mut self,
        for_expr: &ForExpr,
        scope_id: ScopeId,
        _expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        let binding_ident = for_expr.binding.unwrap_or({
            let this = &mut self.ast;
            this.identifiers.intern("it")
        });
        let iterable_expr = self.eval_expr(for_expr.iterable_expr, scope_id, None)?;
        let iteree_type = iterable_expr.get_type();
        let is_string_iteree = iteree_type == STRING_TYPE_ID;
        let iterable_span = iterable_expr.get_span();
        let body_span = for_expr.body_block.span;

        let Some(item_type) = self.types.item_type_of_iterable(iteree_type) else {
            return make_fail_span(
                format!(
                    "Type {} is not iterable",
                    self.type_id_to_string(iterable_expr.get_type())
                ),
                iterable_span,
            );
        };

        let is_do_block = for_expr.expr_type == ForExprType::Do;

        // We de-sugar the 'for ... do' expr into a typed while loop, synthesizing
        // a few local variables in order to achieve this.

        let for_expr_scope = self.scopes.add_child_scope(scope_id, ScopeType::ForExpr, None, None);

        let (index_variable, index_defn_stmt, index_variable_expr) = self.synth_variable_decl(
            self.ast.identifiers.get("it_index").unwrap(),
            TypedExpr::Integer(TypedIntegerExpr {
                value: TypedIntegerValue::U64(0),
                span: for_expr.body_block.span,
            }),
            true,
            true,
            for_expr_scope,
        );
        let (_iteree_variable_id, iteree_defn_stmt, iteree_variable_expr) = self
            .synth_variable_decl(
                self.ast.identifiers.get("iteree").unwrap(),
                iterable_expr,
                false,
                false,
                for_expr_scope,
            );
        let iteree_length_call = if is_string_iteree {
            self.synth_function_call(
                vec![self.ast.identifiers.get("string").unwrap()],
                self.ast.identifiers.get("length").unwrap(),
                None,
                vec![iteree_variable_expr.clone()],
                iterable_span,
                for_expr_scope,
            )?
        } else {
            self.synth_function_call(
                vec![self.ast.identifiers.get("Array").unwrap()],
                self.ast.identifiers.get("length").unwrap(),
                Some(vec![item_type]),
                vec![iteree_variable_expr.clone()],
                iterable_span,
                for_expr_scope,
            )?
        };
        let iteree_length_ident = self.ast.identifiers.intern("iteree_length");
        let (_iteree_length_variable_id, iteree_length_defn_stmt, iteree_length_variable_expr) =
            self.synth_variable_decl(
                iteree_length_ident,
                iteree_length_call,
                false,
                false,
                for_expr_scope,
            );

        let while_scope_id =
            self.scopes.add_child_scope(for_expr_scope, ScopeType::WhileBody, None, None);
        let binding_variable_id = self.variables.add_variable(Variable {
            name: binding_ident,
            type_id: item_type,
            is_mutable: false,
            owner_scope: while_scope_id,
        });
        self.scopes.add_variable(while_scope_id, binding_ident, binding_variable_id);
        let iteration_element_val_def = TypedStmt::ValDef(Box::new(ValDef {
            variable_id: binding_variable_id,
            ty: item_type,
            initializer: if is_string_iteree {
                TypedExpr::StringIndex(IndexOp {
                    base_expr: Box::new(iteree_variable_expr.clone()),
                    index_expr: Box::new(TypedExpr::Variable(VariableExpr {
                        type_id: U64_TYPE_ID,
                        variable_id: index_variable,
                        span: body_span,
                    })),
                    result_type: item_type,
                    span: body_span,
                })
            } else {
                TypedExpr::ArrayIndex(IndexOp {
                    base_expr: Box::new(iteree_variable_expr.clone()),
                    index_expr: Box::new(TypedExpr::Variable(VariableExpr {
                        type_id: U64_TYPE_ID,
                        variable_id: index_variable,
                        span: body_span,
                    })),
                    result_type: item_type,
                    span: body_span,
                })
            },
            span: body_span,
        }));

        // TODO: we can hint to the block based on the expected type of the entire for expr
        let body_scope_id =
            self.scopes.add_child_scope(while_scope_id, ScopeType::LexicalBlock, None, None);
        let body_block = self.eval_block(&for_expr.body_block, body_scope_id, None)?;
        let body_block_result_type = body_block.expr_type;

        let resulting_type = if is_do_block {
            UNIT_TYPE_ID
        } else {
            match self.types.get(iteree_type) {
                Type::Optional(_opt) => {
                    let new_optional =
                        Type::Optional(OptionalType { inner_type: body_block.expr_type });
                    self.types.add_type(new_optional)
                }
                Type::Array(_arr) => {
                    let new_array = Type::Array(ArrayType { element_type: body_block.expr_type });
                    self.types.add_type(new_array)
                }
                Type::String => {
                    let new_array = Type::Array(ArrayType { element_type: body_block.expr_type });
                    self.types.add_type(new_array)
                }
                other => {
                    todo!("Unsupported iteree type: {}", self.type_to_string(other))
                }
            }
        };
        let yield_decls = if !is_do_block {
            let yield_initializer = match self.types.get(resulting_type) {
                Type::Array(_array_type) => self.synth_function_call(
                    vec![self.ast.identifiers.get("Array").unwrap()],
                    self.ast.identifiers.get("new").unwrap(),
                    Some(vec![body_block_result_type]),
                    vec![iteree_length_variable_expr.clone()],
                    body_span,
                    for_expr_scope,
                )?,
                _ => {
                    return make_fail_span(
                        "unsupported resulting_type in for_expr yield",
                        for_expr.span,
                    )
                }
            };
            let yield_ident = self.ast.identifiers.intern("yielded_coll");
            Some(self.synth_variable_decl(
                yield_ident,
                yield_initializer,
                false,
                false,
                for_expr_scope,
            ))
        } else {
            None
        };
        let mut while_block = TypedBlock {
            expr_type: UNIT_TYPE_ID,
            scope_id: while_scope_id,
            statements: Vec::new(),
            span: body_span,
        };
        // Prepend the val def to the body block
        while_block.statements.push(iteration_element_val_def);
        let block_expr_val_ident = self.ast.identifiers.intern("block_expr_val");
        let (_user_block_val_id, user_block_val_def, user_block_val_expr) = self
            .synth_variable_decl(
                block_expr_val_ident,
                TypedExpr::Block(body_block),
                false,
                false,
                while_scope_id,
            );
        while_block.statements.push(user_block_val_def);

        // Assign element to yielded array
        if let Some((_yield_coll_variable_id, _yield_def, yielded_coll_expr)) = &yield_decls {
            match self.types.get(resulting_type) {
                Type::Array(_) => {
                    // yielded_coll[index] = block_expr_val;
                    let element_assign = TypedStmt::Assignment(Box::new(Assignment {
                        destination: Box::new(TypedExpr::ArrayIndex(IndexOp {
                            base_expr: Box::new(yielded_coll_expr.clone()),
                            index_expr: Box::new(index_variable_expr.clone()),
                            result_type: body_block_result_type,
                            span: body_span,
                        })),
                        value: Box::new(user_block_val_expr),
                        span: body_span,
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
                ty: U64_TYPE_ID,
                lhs: Box::new(TypedExpr::Variable(VariableExpr {
                    type_id: U64_TYPE_ID,
                    variable_id: index_variable,
                    span: iterable_span,
                })),
                rhs: Box::new(TypedExpr::Integer(TypedIntegerExpr {
                    value: TypedIntegerValue::U64(1),
                    span: iterable_span,
                })),
                span: iterable_span,
            })),
            span: iterable_span,
        }));
        while_block.statements.push(index_increment_statement);

        let while_stmt = TypedStmt::WhileLoop(Box::new(TypedWhileLoop {
            cond: TypedExpr::BinaryOp(BinaryOp {
                kind: BinaryOpKind::Less,
                ty: BOOL_TYPE_ID,
                lhs: Box::new(index_variable_expr.clone()),
                rhs: Box::new(iteree_length_variable_expr),
                span: iterable_span,
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
        Ok(final_expr)
    }

    fn expect_ability_implementation(
        &self,
        ability_id: AbilityId,
        type_id: TypeId,
        span_for_error: SpanId,
    ) -> TyperResult<&TypedAbilityImpl> {
        self.ability_impls
            .iter()
            .find(|imple| imple.type_id == type_id && imple.ability_id == ability_id)
            .ok_or(make_error(
                format!(
                    "Missing ability '{}' implementation for '{}'",
                    self.ast.identifiers.get_name(self.get_ability(ability_id).name),
                    self.type_id_to_string(type_id)
                ),
                span_for_error,
            ))
    }

    fn eval_binary_op(
        &mut self,
        binary_op: &parse::BinaryOp,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        fn is_scalar_for_equals(type_id: TypeId) -> bool {
            match type_id {
                UNIT_TYPE_ID | CHAR_TYPE_ID | U8_TYPE_ID | U16_TYPE_ID | U32_TYPE_ID
                | U64_TYPE_ID | I8_TYPE_ID | I16_TYPE_ID | I32_TYPE_ID | I64_TYPE_ID
                | BOOL_TYPE_ID => true,
                _other => false,
            }
        }
        // Special cases: Equality, and OptionalElse
        match binary_op.op_kind {
            BinaryOpKind::Equals | BinaryOpKind::NotEquals => {
                let lhs = self.eval_expr(binary_op.lhs, scope_id, None)?;
                if !is_scalar_for_equals(lhs.get_type()) {
                    return self.eval_equality_expr(lhs, &binary_op, scope_id, expected_type);
                }
            }
            BinaryOpKind::OptionalElse => {
                // LHS must be an optional and RHS must be its contained type
                let lhs = self.eval_expr(binary_op.lhs, scope_id, None)?;
                let Some(lhs_optional) = self.types.get(lhs.get_type()).as_optional() else {
                    return make_fail_span(
                        &format!(
                            "'else' operator can only be used on an optional; type was '{}'",
                            self.type_id_to_string(lhs.get_type())
                        ),
                        binary_op.span,
                    );
                };
                let lhs_inner = lhs_optional.inner_type;

                let rhs = self.eval_expr(binary_op.rhs, scope_id, Some(lhs_inner))?;
                let rhs_type = rhs.get_type();
                if let Err(msg) = self.check_types(lhs_inner, rhs_type, scope_id) {
                    return make_fail_span(
                        &format!("'else' value incompatible with optional: {}", msg),
                        binary_op.span,
                    );
                }
                let mut coalesce_block = self.synth_block(vec![], scope_id, binary_op.span);
                let lhs_variable_name = self.ast.identifiers.intern("optelse_lhs");
                let (_lhs_variable_id, lhs_decl_stmt, lhs_variable_expr) = self
                    .synth_variable_decl(
                        lhs_variable_name,
                        lhs,
                        false,
                        false,
                        coalesce_block.scope_id,
                    );
                let lhs_has_value =
                    TypedExpr::OptionalHasValue(Box::new(lhs_variable_expr.clone()));
                let lhs_unwrap_expr = TypedExpr::OptionalGet(OptionalGet {
                    inner_expr: Box::new(lhs_variable_expr),
                    result_type_id: lhs_inner,
                    span: binary_op.span,
                    checked: false,
                });
                let if_else = TypedExpr::If(Box::new(TypedIf {
                    condition: lhs_has_value,
                    consequent: self.coerce_expr_to_block(lhs_unwrap_expr, coalesce_block.scope_id),
                    alternate: self.coerce_expr_to_block(rhs, coalesce_block.scope_id),
                    ty: rhs_type,
                    span: binary_op.span,
                }));
                coalesce_block.push_stmt(lhs_decl_stmt);
                coalesce_block.push_expr(if_else);
                return Ok(TypedExpr::Block(coalesce_block));
            }
            _ => {}
        };

        // Rest of the binary ops

        let lhs = self.eval_expr(binary_op.lhs, scope_id, None)?;
        let kind = binary_op.op_kind;
        let lhs_type_id = lhs.get_type();
        let result_type = match self.types.get(lhs_type_id) {
            Type::Integer(_integer_type) => match kind {
                BinaryOpKind::Add => Ok(lhs_type_id),
                BinaryOpKind::Subtract => Ok(lhs_type_id),
                BinaryOpKind::Multiply => Ok(lhs_type_id),
                BinaryOpKind::Divide => Ok(lhs_type_id),
                BinaryOpKind::Rem => Ok(lhs_type_id),
                BinaryOpKind::Less => Ok(BOOL_TYPE_ID),
                BinaryOpKind::LessEqual => Ok(BOOL_TYPE_ID),
                BinaryOpKind::Greater => Ok(BOOL_TYPE_ID),
                BinaryOpKind::GreaterEqual => Ok(BOOL_TYPE_ID),
                BinaryOpKind::And => ffail!(binary_op.span, "Invalid left-hand side for and"),
                BinaryOpKind::Or => ffail!(binary_op.span, "Invalid left-hand side for or"),
                BinaryOpKind::Equals => Ok(BOOL_TYPE_ID),
                BinaryOpKind::NotEquals => Ok(BOOL_TYPE_ID),
                _ => unreachable!(),
            },
            Type::Bool => match kind {
                BinaryOpKind::Add
                | BinaryOpKind::Subtract
                | BinaryOpKind::Multiply
                | BinaryOpKind::Divide
                | BinaryOpKind::Rem
                | BinaryOpKind::Less
                | BinaryOpKind::LessEqual
                | BinaryOpKind::Greater
                | BinaryOpKind::GreaterEqual => {
                    ffail!(binary_op.span, "Invalid operation on bool: {}", kind)
                }
                BinaryOpKind::And => Ok(BOOL_TYPE_ID),
                BinaryOpKind::Or => Ok(BOOL_TYPE_ID),
                BinaryOpKind::Equals => Ok(BOOL_TYPE_ID),
                BinaryOpKind::NotEquals => Ok(BOOL_TYPE_ID),
                BinaryOpKind::OptionalElse => unreachable!(),
            },
            Type::Char => match kind {
                BinaryOpKind::Add
                | BinaryOpKind::Subtract
                | BinaryOpKind::Multiply
                | BinaryOpKind::Divide
                | BinaryOpKind::Rem
                | BinaryOpKind::Less
                | BinaryOpKind::LessEqual
                | BinaryOpKind::Greater
                | BinaryOpKind::GreaterEqual
                | BinaryOpKind::And
                | BinaryOpKind::Or => ffail!(binary_op.span, "Invalid operation on char: {}", kind),
                BinaryOpKind::Equals => Ok(BOOL_TYPE_ID),
                BinaryOpKind::NotEquals => Ok(BOOL_TYPE_ID),
                BinaryOpKind::OptionalElse => unreachable!(),
            },
            Type::Unit => match kind {
                BinaryOpKind::Equals => Ok(BOOL_TYPE_ID),
                BinaryOpKind::NotEquals => Ok(BOOL_TYPE_ID),
                _ => ffail!(binary_op.span, "Invalid operation on unit: {}", kind),
            },
            _other => {
                ffail!(binary_op.span, "Invalid left-hand side of binary operation {}", kind)
            }
        }?;

        // At this point I think all operations are symmetric but we'll leave this here
        // to signal that invariant and in case things change
        let expected_rhs_type = if kind.is_symmetric_binop() { Some(lhs.get_type()) } else { None };
        let rhs = self.eval_expr(binary_op.rhs, scope_id, expected_rhs_type)?;

        if kind.is_symmetric_binop() {
            // We already confirmed that the LHS is valid for this operation, and
            // if the op is symmetric, we just have to check the RHS matches
            if let Err(msg) = self.check_types(lhs.get_type(), rhs.get_type(), scope_id) {
                return ffail!(binary_op.span, "operand types did not match: {msg}");
            }
        } else {
            panic!("Unexpected asymmetric binop down here {}", kind)
        }

        let expr = TypedExpr::BinaryOp(BinaryOp {
            kind,
            ty: result_type,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            span: binary_op.span,
        });
        Ok(expr)
    }

    fn eval_equality_expr(
        &mut self,
        lhs: TypedExpr,
        binary_op: &parse::BinaryOp,
        scope_id: ScopeId,
        _expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        assert!(
            binary_op.op_kind == BinaryOpKind::Equals
                || binary_op.op_kind == BinaryOpKind::NotEquals
        );
        // let lhs = self.eval_expr(binary_op.lhs, scope_id, None)?;
        let lhs_type_id = lhs.get_type();
        let equals_expr = match self.types.get(lhs_type_id) {
            Type::TypeVariable(_type_var) => {
                let rhs = self.eval_expr(binary_op.rhs, scope_id, Some(lhs.get_type()))?;
                if let Err(msg) = self.check_types(lhs.get_type(), rhs.get_type(), scope_id) {
                    return make_fail_span(
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
            Type::TagInstance(tag) => {
                let tag_ident = tag.ident;
                let rhs = self.eval_expr(binary_op.rhs, scope_id, None)?;
                let Type::TagInstance(rhs_tag) = self.types.get(rhs.get_type()) else {
                    return make_fail_span("Expected string on rhs", binary_op.span);
                };
                if rhs_tag.ident != tag_ident {
                    return make_fail_span(
                        "Cannot compare different tags for equality",
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
            Type::Unit | Type::Char | Type::Integer(_) | Type::Bool => {
                panic!("Scalar ints shouldnt be passed to eval_equality_expr")
            }
            _other_lhs_type => {
                let rhs = self.eval_expr(binary_op.rhs, scope_id, Some(lhs.get_type()))?;
                if rhs.get_type() != lhs_type_id {
                    make_fail_span("Expected lhs and rhs to match", binary_op.span)
                } else {
                    let call_expr = self.synth_equals_call(lhs, rhs, binary_op.span)?;
                    Ok(call_expr)
                }
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

    fn synth_equals_call(
        &self,
        lhs: TypedExpr,
        rhs: TypedExpr,
        span: SpanId,
    ) -> TyperResult<TypedExpr> {
        let implementation =
            self.expect_ability_implementation(EQUALS_ABILITY_ID, lhs.get_type(), span)?;
        let ability = self.get_ability(EQUALS_ABILITY_ID);
        let equals_index =
            ability.find_function_by_name(self.ast.identifiers.get("equals").unwrap()).unwrap().0;
        let equals_implementation_function_id = implementation.function_at_index(equals_index);
        let call_expr = TypedExpr::FunctionCall(Call {
            callee_function_id: equals_implementation_function_id,
            args: vec![lhs, rhs],
            type_args: Vec::new(),
            ret_type: BOOL_TYPE_ID, // TODO: We should assert that equals does return a bool so we don't emit invalid bytecode
            span,
        });
        Ok(call_expr)
    }

    fn eval_if_expr(
        &mut self,
        if_expr: &IfExpr,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedExpr> {
        // If the condition is an IsExpr, we desugar to a match
        let cond = self.ast.expressions.get(if_expr.cond);
        if let ParsedExpression::Is(is_expr) = cond {
            let cond_pattern_id = is_expr.pattern;
            let cond_expr_id = is_expr.target_expression;
            let alternate_expr = match if_expr.alt {
                None => {
                    let id = self
                        .ast
                        .expressions
                        .add_expression(ParsedExpression::Literal(Literal::Unit(if_expr.span)));
                    id
                }
                Some(alt) => alt,
            };
            let wildcard_pattern_id =
                self.ast.patterns.add_pattern(parse::ParsedPattern::Wildcard(if_expr.span));
            let cases = vec![
                parse::ParsedMatchCase { pattern: cond_pattern_id, expression: if_expr.cons },
                parse::ParsedMatchCase { pattern: wildcard_pattern_id, expression: alternate_expr },
            ];
            let match_expr = parse::ParsedMatchExpression {
                target_expression: cond_expr_id,
                cases,
                span: if_expr.span,
            };
            // This could definitely double-borrow expressions
            let match_expr_id =
                self.ast.expressions.add_expression(ParsedExpression::Match(match_expr));
            // eprintln!("desugared if to match {}", self.ast.expression_to_string(match_expr_id));
            return self.eval_match_expr(match_expr_id, scope_id, expected_type);
        }
        //
        // End of match desugar case

        let condition = self.eval_expr(if_expr.cond, scope_id, None)?;

        let mut consequent = {
            if let Err(msg) = self.check_types(BOOL_TYPE_ID, condition.get_type(), scope_id) {
                return make_fail_span(
                    format!("Invalid if condition type: {}.", msg),
                    condition.get_span(),
                );
            }
            let consequent_expr = self.eval_expr(if_expr.cons, scope_id, expected_type)?;
            self.coerce_expr_to_block(consequent_expr, scope_id)
        };
        let consequent_type = consequent.expr_type;
        // De-sugar if without else:
        // If there is no alternate, we coerce the consequent to return Unit, so both
        // branches have a matching type, making codegen simpler
        // However, if the consequent is a never type, we don't need to do this, in
        // fact we can't because then we'd have an expression following a never expression
        let cons_never = consequent_type == NEVER_TYPE_ID;

        if if_expr.alt.is_none() && !cons_never {
            self.coerce_block_to_unit_block(&mut consequent);
        };
        let alternate = if let Some(alt) = if_expr.alt {
            let type_hint = if cons_never { None } else { Some(consequent_type) };
            let expr = self.eval_expr(alt, scope_id, type_hint)?;
            self.coerce_expr_to_block(expr, scope_id)
        } else {
            self.make_unit_block(scope_id, if_expr.span)
        };

        // Special typechecking that accounts for 'never'.
        // I don't think this scales; we probably need to typecheck this way in other places.
        // But I don't yet fully understand the pattern or how to extract it;
        // perhaps its a parameter to typecheck_types, or a function that delegates to typecheck types.
        // I think its different in that it takes in a list of types (think pattern matching where we have more than 2 branches)

        // Then essentially discards the 'never' types and runs resolution as if they weren't there. imagine
        // when x {
        //   | A -> crash()
        //   | B -> crash()
        //   | C -> 42
        //   | D -> crash()
        // }: int
        let cons_never = consequent_type == NEVER_TYPE_ID;
        let alt_never = alternate.expr_type == NEVER_TYPE_ID;
        let no_never = !cons_never && !alt_never;

        let overall_type = if no_never {
            if let Err(msg) = self.check_types(consequent_type, alternate.expr_type, scope_id) {
                return make_fail_span(
                    format!("else branch type did not match then branch type: {}", msg),
                    alternate.span,
                );
            };
            consequent.expr_type
        } else {
            if cons_never {
                alternate.expr_type
            } else {
                consequent_type
            }
        };
        Ok(TypedExpr::If(Box::new(TypedIf {
            condition,
            consequent,
            alternate,
            ty: overall_type,
            span: if_expr.span,
        })))
    }

    fn make_unit_block(&mut self, scope_id: ScopeId, span: SpanId) -> TypedBlock {
        let unit_expr = TypedExpr::Unit(span);
        let b = self.synth_block(vec![TypedStmt::Expr(Box::new(unit_expr))], scope_id, span);
        b
    }

    fn get_namespace_scope_in_immediate_scope(
        &self,
        scope_id: ScopeId,
        identifier_id: IdentifierId,
    ) -> Option<&Scope> {
        let search_scope = self.scopes.get_scope(scope_id);
        let Some(namespace_id) = search_scope.find_namespace(identifier_id) else {
            return None;
        };
        let namespace = self.namespaces.get(namespace_id);
        let scope = self.scopes.get_scope(namespace.scope_id);
        Some(scope)
    }

    /// Can 'shortcircuit' with Left if the function call to resolve
    /// is actually a builtin
    fn resolve_parsed_function_call(
        &mut self,
        fn_call: &FnCall,
        calling_scope: ScopeId,
    ) -> TyperResult<Either<TypedExpr, FunctionId>> {
        match self.ast.identifiers.get_name(fn_call.name) {
            "return" => {
                if fn_call.args.len() != 1 {
                    return make_fail_span(
                        "return(...) must have exactly one argument",
                        fn_call.span,
                    );
                }
                let return_value =
                    self.eval_expr_inner(fn_call.args[0].value, calling_scope, None)?;
                let enclosing_function = self.scopes.nearest_parent_function(calling_scope);
                let expected_return_type = self.get_function(enclosing_function).ret_type;
                if let Err(msg) =
                    self.check_types(expected_return_type, return_value.get_type(), calling_scope)
                {
                    return make_fail_span(
                        format!("return type did not match function return type: {}", msg),
                        fn_call.span,
                    );
                }
                return Ok(Either::Left(TypedExpr::Return(TypedReturn {
                    value: Box::new(return_value),
                    span: fn_call.span,
                })));
            }
            "Some" => {
                if fn_call.args.len() != 1 {
                    return make_fail_span(
                        "Some(...) must have exactly one argument",
                        fn_call.span,
                    );
                }
                let arg = self.eval_expr_inner(fn_call.args[0].value, calling_scope, None)?;
                let type_id = arg.get_type();
                let optional_type = Type::Optional(OptionalType { inner_type: type_id });
                let type_id = self.types.add_type(optional_type);
                return Ok(Either::Left(TypedExpr::OptionalSome(OptionalSome {
                    inner_expr: Box::new(arg),
                    type_id,
                })));
            }
            "compilerFile" => {
                if fn_call.args.len() != 0 {
                    return make_fail_span("compilerFile() takes no arguments", fn_call.span);
                }
                let span = self.ast.spans.get(fn_call.span);
                let filename = &self.ast.sources.source_by_span(span).filename;
                let string_expr = TypedExpr::Str(filename.clone(), fn_call.span);
                return Ok(Either::Left(string_expr));
            }
            "compilerLine" => {
                if fn_call.args.len() != 0 {
                    return make_fail_span("compilerLine() takes no arguments", fn_call.span);
                }
                let span = self.ast.spans.get(fn_call.span);
                let line = self.ast.sources.get_line_for_span(span).unwrap();
                let int_expr = TypedExpr::Integer(TypedIntegerExpr {
                    value: TypedIntegerValue::U64(line.line_number() as u64),
                    span: fn_call.span,
                });
                return Ok(Either::Left(int_expr));
            }
            _ => {}
        };

        // Two resolution paths:
        // 1. "method" call (aka known first arg type so we can check for ability impls and methods)
        // 2. "function" call, no abilities or methods to check, pure scoping-based resolution

        let root_scope = self.scopes.get_root_scope_id();
        let method_call_self = match fn_call.args.get(0) {
            Some(arg) if fn_call.is_method => {
                Some(self.eval_expr(arg.value, calling_scope, None)?)
            }
            _ => None,
        };
        let function_id = match method_call_self {
            Some(base_expr) => {
                // Resolve a method call
                let type_id = base_expr.get_type();
                if let Type::Reference(_r) = self.types.get(type_id) {
                    if fn_call.name == self.ast.identifiers.get("asRawPointer").unwrap()
                        && fn_call.args.len() == 1
                        && fn_call.type_args.is_none()
                    {
                        return Ok(Either::Left(TypedExpr::UnaryOp(UnaryOp {
                            kind: UnaryOpKind::ReferenceToInt,
                            type_id: RAW_POINTER_TYPE_ID,
                            expr: Box::new(base_expr.clone()),
                            span: fn_call.span,
                        })));
                    };
                };
                let method_id = match self.types.get_type_dereferenced(type_id) {
                    // TODO: DRY up. First resolve to a namespace, then same path
                    // Also use companion namespace method not just name of namespace
                    Type::Optional(_optional_type) => {
                        if fn_call.name == self.ast.identifiers.get("hasValue").unwrap()
                            && fn_call.args.len() == 1
                            && fn_call.type_args.is_none()
                        {
                            return Ok(Either::Left(TypedExpr::OptionalHasValue(Box::new(
                                base_expr.clone(),
                            ))));
                        } else {
                            None
                        }
                    }
                    Type::String => {
                        let string_ident_id = self.ast.identifiers.get("string").unwrap();
                        let string_scope = self
                            .get_namespace_scope_in_immediate_scope(root_scope, string_ident_id);
                        string_scope
                            .map(|string_scope| string_scope.find_function(fn_call.name))
                            .flatten()
                    }
                    Type::Char => {
                        let char_ident_id = self.ast.identifiers.get("char").unwrap();
                        let char_scope =
                            self.get_namespace_scope_in_immediate_scope(root_scope, char_ident_id);
                        char_scope
                            .map(|char_scope| char_scope.find_function(fn_call.name))
                            .flatten()
                    }
                    Type::Array(_array_type) => {
                        let array_ident_id = self.ast.identifiers.get("Array").unwrap();
                        let array_scope =
                            self.get_namespace_scope_in_immediate_scope(root_scope, array_ident_id);
                        array_scope
                            .map(|array_scope| array_scope.find_function(fn_call.name))
                            .flatten()
                    }
                    Type::Struct(struc) => {
                        // Need to distinguish between instances of 'named'
                        // structs and anonymous ones
                        let Some(struct_defn_info) = struc.type_defn_info.as_ref() else {
                            return make_fail_span(
                                "Anonymous structs currently have no methods",
                                fn_call.span,
                            );
                        };
                        if let Some(struct_companion_ns) = struct_defn_info.companion_namespace {
                            let struct_scope = self.get_namespace_scope(struct_companion_ns);
                            struct_scope.find_function(fn_call.name)
                        } else {
                            None
                        }
                    }
                    Type::Enum(e) => {
                        let Some(enum_defn_info) = e.type_defn_info.as_ref() else {
                            return make_fail_span(
                                "Anonymous enums currently have no methods",
                                fn_call.span,
                            );
                        };
                        if let Some(enum_companion_ns) = enum_defn_info.companion_namespace {
                            let enum_scope = self.get_namespace_scope(enum_companion_ns);
                            enum_scope.find_function(fn_call.name)
                        } else {
                            None
                        }
                    }
                    Type::EnumVariant(ev) => {
                        let parent_enum_info =
                            self.types.get(ev.enum_type_id).expect_enum().type_defn_info.as_ref();
                        let Some(parent_enum_info) = parent_enum_info else {
                            return make_fail_span(
                                "Anonymous enums currently have no methods",
                                fn_call.span,
                            );
                        };
                        let enum_scope = self.get_namespace_scope_in_immediate_scope(
                            parent_enum_info.scope,
                            parent_enum_info.name,
                        );
                        enum_scope.map(|s| s.find_function(fn_call.name)).flatten()
                    }
                    Type::OpaqueAlias(opaque) => {
                        let opaque_scope = self.get_namespace_scope_in_immediate_scope(
                            opaque.type_defn_info.scope,
                            opaque.type_defn_info.name,
                        );
                        if let Some(opaque_scope) = opaque_scope {
                            opaque_scope.find_function(fn_call.name)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let ability_fn_id = if method_id.is_none() {
                    self.find_ability_implementation(fn_call.name, type_id, None, fn_call.span)?
                } else {
                    None
                };
                match method_id.or(ability_fn_id) {
                    Some(function_id) => function_id,
                    None => {
                        return make_fail_span(
                            format!(
                                "Method '{}' does not exist on type {}",
                                &*self.get_ident_str(fn_call.name).blue(),
                                self.type_id_to_string(type_id),
                            ),
                            fn_call.span,
                        );
                    }
                }
            }
            None => {
                // Resolve a non-method call
                let is_namespaced_call = !fn_call.namespaces.is_empty();
                let scope_to_search = self.traverse_namespace_chain(
                    calling_scope,
                    &fn_call.namespaces,
                    fn_call.span,
                )?;
                // Qualified lookups shouldn't recurse up the scopes; they should search only the referenced namespace
                let recursive = !is_namespaced_call;
                let function_id = self
                    .scopes
                    .find_function(scope_to_search, fn_call.name, recursive)
                    .ok_or(make_error(
                        format!(
                            "Function not found: {} in scope: {:?}",
                            &*self.get_ident_str(fn_call.name),
                            self.scopes.get_scope(calling_scope)
                        ),
                        fn_call.span,
                    ))?;
                if self.scopes.get_scope(scope_to_search).scope_type == ScopeType::AbilityDefn {
                    let ability_id =
                        self.scopes.get_scope(scope_to_search).owner_id.unwrap().expect_ability();
                    let first_arg = fn_call.args.first().ok_or(make_error(
                        "Ability functions must have at least one argument",
                        fn_call.span,
                    ))?;
                    let base_expr = self.eval_expr(first_arg.value, calling_scope, None)?;
                    let function_id = self.find_ability_implementation(
                        fn_call.name,
                        base_expr.get_type(),
                        Some(ability_id),
                        fn_call.span,
                    )?;
                    function_id.ok_or(ferr!(
                        fn_call.span,
                        "Type {} does not implement ability {}",
                        self.type_id_to_string(base_expr.get_type()),
                        self.get_ident_str(fn_call.name)
                    ))?
                } else {
                    function_id
                }
            }
        };
        return Ok(Either::Right(function_id));
    }

    fn find_ability_implementation(
        &self,
        function_name: IdentifierId,
        type_id: TypeId,
        only_ability_id: Option<AbilityId>,
        span: SpanId,
    ) -> TyperResult<Option<FunctionId>> {
        let impls = self.ability_impls.iter().filter(|imp| {
            imp.type_id == type_id
            // Inlined Option.is_none_or
            && match only_ability_id {
                    None => true,
                    Some(x) => x == imp.ability_id,
                }
        });
        let mut matching_fns = Vec::new();
        for imp in impls {
            let ability = self.get_ability(imp.ability_id);
            let matching = ability.find_function_by_name(function_name);
            if let Some((matching_index, _generic_fn)) = matching {
                let impl_fn_id = imp.function_at_index(matching_index);
                matching_fns.push(impl_fn_id);
            }
        }
        if matching_fns.len() == 0 {
            Ok(None)
        } else if matching_fns.len() > 1 {
            ffail!(span, "Ambiguous ability")
        } else {
            Ok(Some(matching_fns[0]))
        }
    }

    // skip_typecheck: I actually just want this to handle the 'self' order and zipping thing because I sometimes
    // don't want these checked just yet because I'm using them for inference
    // ^ That use case may not be valid anymore now that 'self' is handled sanely as just the first argument
    fn typecheck_call_arguments(
        &mut self,
        fn_call: &FnCall,
        params: &Vec<FnArgDefn>,
        pre_evaled_params: Option<Vec<TypedExpr>>,
        calling_scope: ScopeId,
        skip_typecheck: bool,
    ) -> TyperResult<Vec<TypedExpr>> {
        let total_passed =
            pre_evaled_params.as_ref().map(|v| v.len()).unwrap_or(fn_call.args.len());
        if total_passed != params.len() {
            return make_fail_span(
                format!(
                    "Incorrect number of arguments: expected {}, got {}",
                    params.len(),
                    total_passed
                ),
                fn_call.span,
            );
        }

        let mut final_args: Vec<TypedExpr> = Vec::new();
        let mut pre_evaled_params = match pre_evaled_params {
            Some(v) => v.into_iter(),
            None => Vec::new().into_iter(),
        };
        for fn_param in params {
            if let Some(pre_evaled_expr) = pre_evaled_params.next() {
                if let Err(msg) =
                    self.check_types(fn_param.type_id, pre_evaled_expr.get_type(), calling_scope)
                {
                    return make_fail_span(
                        format!(
                            "Invalid parameter type passed to function {}: {}",
                            &*self.ast.identifiers.get_name(fn_call.name),
                            msg
                        ),
                        pre_evaled_expr.get_span(),
                    );
                } else {
                    final_args.push(pre_evaled_expr);
                }
            } else {
                let matching_param_by_name =
                    fn_call.args.iter().find(|arg| arg.name == Some(fn_param.name));
                let matching_idx = fn_param.position;
                let matching_param =
                    matching_param_by_name.or(fn_call.args.get(matching_idx as usize));

                if let Some(param) = matching_param {
                    let expected_type_for_param =
                        if skip_typecheck { None } else { Some(fn_param.type_id) };
                    let expr =
                        self.eval_expr(param.value, calling_scope, expected_type_for_param)?;
                    if !skip_typecheck {
                        if let Err(e) =
                            self.check_types(fn_param.type_id, expr.get_type(), calling_scope)
                        {
                            return make_fail_span(
                                format!(
                                    "Invalid parameter type passed to function {}: {}",
                                    &*self.ast.identifiers.get_name(fn_call.name),
                                    e
                                ),
                                expr.get_span(),
                            );
                        }
                    }
                    final_args.push(expr);
                } else {
                    return make_fail_span(
                        format!(
                            "Missing argument to function {}: {}",
                            &*self.ast.identifiers.get_name(fn_call.name),
                            &*self.get_ident_str(fn_param.name)
                        ),
                        fn_call.span,
                    );
                }
            }
        }
        Ok(final_args)
    }

    fn eval_function_call(
        &mut self,
        fn_call: &FnCall,
        this_expr: Option<TypedExpr>,
        known_type_args: Option<Vec<TypeId>>,
        known_value_args: Option<Vec<TypedExpr>>,
        scope_id: ScopeId,
    ) -> TyperResult<TypedExpr> {
        assert!(
            fn_call.args.is_empty() || known_value_args.is_none(),
            "cannot pass both typed value args and parsed value args to eval_function_call"
        );
        let function_id = match self.resolve_parsed_function_call(fn_call, scope_id)? {
            Either::Left(expr) => return Ok(expr),
            Either::Right(function_id) => function_id,
        };

        // Re-evaluate known-expr with a type hint now that we know the function
        // let this_expr =
        //     fn_call.args.get(0).map(|arg| self.eval_expr(arg.value, scope_id, expected_type));

        // Now that we have resolved to a function id, we need to specialize it if generic
        let original_function = self.get_function(function_id);
        let original_params = original_function.params.clone();

        let (function_to_call, typechecked_arguments, type_args) =
            match original_function.type_params.is_empty() {
                true => (
                    function_id,
                    self.typecheck_call_arguments(
                        fn_call,
                        &original_params,
                        known_value_args,
                        scope_id,
                        false,
                    )?,
                    Vec::new(),
                ),
                false => {
                    let type_params = &original_function.type_params;
                    let intrinsic_type = original_function.intrinsic_type;

                    // We infer the type arguments, or just use them if the user has supplied them
                    let type_args = match known_type_args {
                        Some(ta) => {
                            // Need the ident
                            ta.into_iter()
                                .enumerate()
                                .map(|(idx, type_id)| TypeParam {
                                    ident: type_params[idx].ident,
                                    type_id,
                                })
                                .collect()
                        }
                        None => self.infer_call_type_args(
                            fn_call,
                            function_id,
                            // We shouldn't have to clone this because we should always
                            // pass the type args if we pass the known value args
                            known_value_args.clone(),
                            scope_id,
                        )?,
                    };

                    // We skip specialization if any of the type arguments are type variables: `any_type_vars`
                    // because we could just be evaluating a generic function that calls another generic function,
                    // in which case we don't want to generate a 'specialized' function where we haven't
                    // actually specialized everything
                    let mut fully_concrete = true;
                    for TypeParam { type_id, .. } in type_args.iter() {
                        if self.types.does_type_reference_type_variables(*type_id) {
                            fully_concrete = false
                        }
                    }
                    if !fully_concrete {
                        (
                            function_id,
                            self.typecheck_call_arguments(
                                fn_call,
                                &original_params,
                                known_value_args,
                                scope_id,
                                false,
                            )?,
                            type_args,
                        )
                    } else {
                        let (function_id, args) = self.get_specialized_function_for_call(
                            fn_call,
                            &type_args,
                            function_id,
                            intrinsic_type,
                            this_expr,
                            scope_id,
                            known_value_args,
                        )?;
                        (function_id, args, type_args)
                    }
                }
            };

        let function_ret_type = self.get_function(function_to_call).ret_type;
        let call = Call {
            callee_function_id: function_to_call,
            args: typechecked_arguments,
            type_args,
            ret_type: function_ret_type,
            span: fn_call.span,
        };
        Ok(TypedExpr::FunctionCall(call))
    }

    fn infer_call_type_args(
        &mut self,
        fn_call: &FnCall,
        generic_function_id: FunctionId,
        pre_evaled_params: Option<Vec<TypedExpr>>,
        calling_scope: ScopeId,
    ) -> TyperResult<Vec<TypeParam>> {
        let generic_function = self.get_function(generic_function_id);
        let generic_type_params = generic_function.type_params.clone();
        debug_assert!(!generic_type_params.is_empty());
        let generic_name = generic_function.name;
        let generic_params = generic_function.params.clone();
        let type_params = match &fn_call.type_args {
            Some(type_args) => {
                if type_args.len() != generic_type_params.len() {
                    return make_fail_span(
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
                    let type_id = self.eval_type_expr(type_arg.type_expr, calling_scope, None)?;
                    checked_params.push(TypeParam { ident: param.ident, type_id });
                }
                checked_params
            }
            None => {
                let exprs = self.typecheck_call_arguments(
                    fn_call,
                    &generic_params,
                    pre_evaled_params,
                    calling_scope,
                    true,
                )?;

                let mut solved_params: Vec<TypeParam> = Vec::new();

                for (idx, expr) in exprs.iter().enumerate() {
                    let param = &generic_params[idx];

                    // This 'match' is where we would ideally implement some actual
                    // recursive type unification algorithm rather than hardcode 2 cases
                    let maybe_solved_param = match self.types.get(param.type_id) {
                        Type::TypeVariable(tv) => {
                            // If the type param is used in the type of the argument, we can infer
                            // the type param from the type of the argument
                            Some(TypeParam { ident: tv.name, type_id: expr.get_type() })
                        }
                        Type::Array(array_type) => {
                            // If the type param is used in the element type of an array, we can infer
                            // the type param from the type of the array
                            if let Type::TypeVariable(tv) = self.types.get(array_type.element_type)
                            {
                                if let Type::Array(at) = self.types.get(expr.get_type()) {
                                    Some(TypeParam { ident: tv.name, type_id: at.element_type })
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
                            if let Err(msg) = self.check_types(
                                existing_solution.type_id,
                                solved_param.type_id,
                                calling_scope,
                            ) {
                                return make_fail_span(
                                    format!(
                                        "Conflicting type parameters for type param {} in call to {}: {}",
                                        self.get_ident_str(solved_param.ident),
                                        self.get_ident_str(generic_name),
                                        msg
                                    ),
                                    fn_call.span,
                                );
                            } else {
                                debug!("We double-solved type param {} but that's ok because it matched", self.get_ident_str(existing_solution.ident))
                            }
                        } else {
                            // Only push if we haven't solved this one yet
                            solved_params.push(solved_param);
                        }
                    }
                }
                if solved_params.len() < generic_type_params.len() {
                    return make_fail_span(
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
        inferred_or_passed_type_args: &Vec<TypeParam>,
        generic_function_id: FunctionId,
        intrinsic_type: Option<IntrinsicFunction>,
        this_expr: Option<TypedExpr>,
        calling_scope: ScopeId,
        pre_evaled_value_args: Option<Vec<TypedExpr>>,
    ) -> TyperResult<(FunctionId, Vec<TypedExpr>)> {
        let generic_function = self.get_function(generic_function_id);
        let generic_function_parent_scope = self
            .scopes
            .get_scope(generic_function.scope)
            .parent
            .expect("No function scope should be a root scope");
        let generic_function_metadata = generic_function.metadata;
        let generic_function_span = generic_function.span;
        let specializations = generic_function.specializations.clone();
        let name = String::from(self.get_ident_str(generic_function.name));
        // drop(generic_function);

        // Place specializations as siblings aside their generic parent
        // Just don't add them as functions to the parent scope so they can't be resolved
        let spec_fn_scope_id = self.scopes.add_child_scope(
            generic_function_parent_scope,
            ScopeType::FunctionScope,
            None,
            None,
        );

        let mut new_name = "".to_string();
        new_name.push_str(&name);

        // Add type_args to scope and typecheck them against the actual params
        for type_param in inferred_or_passed_type_args.iter() {
            if let Type::TypeVariable(tv) = self.types.get(type_param.type_id) {
                return make_fail_span(
                    format!(
                        "Cannot specialize function with type variable: {}",
                        &*self.get_ident_str(tv.name)
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
            if !self
                .scopes
                .get_scope_mut(spec_fn_scope_id)
                .add_type(type_param.ident, type_param.type_id)
            {
                return make_fail_span(
                    &format!(
                        "Type {} already existed in spec fn scope",
                        self.get_ident_str(type_param.ident)
                    ),
                    generic_function_span,
                );
            }
        }
        let type_ids = inferred_or_passed_type_args
            .iter()
            .map(|type_param| type_param.type_id)
            .collect::<Vec<_>>();
        new_name.push_str("_spec_");
        new_name.push_str(
            &type_ids.iter().map(|type_id| type_id.to_string()).collect::<Vec<_>>().join("_"),
        );

        self.scopes.get_scope_mut(spec_fn_scope_id).name =
            Some(self.ast.identifiers.intern(&new_name));

        for (i, existing_specialization) in specializations.iter().enumerate() {
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
                    &existing_specialization.specialized_params,
                    pre_evaled_value_args,
                    calling_scope,
                    false,
                )?;
                return Ok((existing_specialization.specialized_function_id, exprs));
            }
        }

        // TODO: new_name should maybe be calculated by specialize_function
        let generic_function_ast_id = generic_function_metadata.parsed_function_id();
        let new_name_ident = self.ast.identifiers.intern(&new_name);
        let specialized_function_id = self.specialize_function(
            generic_function_ast_id,
            SpecializationParams {
                fn_scope_id: spec_fn_scope_id,
                new_name: new_name_ident,
                known_intrinsic: intrinsic_type,
                generic_parent_function: generic_function_id,
                ability_impl_info: None,
            },
        )?;
        let specialized_params = self.get_function(specialized_function_id).params.clone();
        self.get_function_mut(generic_function_id).specializations.push(SpecializationStruct {
            specialized_function_id,
            specialized_type_params: type_ids,
            specialized_params: specialized_params.clone(),
        });

        let typechecked_exprs = self.typecheck_call_arguments(
            fn_call,
            &specialized_params,
            pre_evaled_value_args,
            calling_scope,
            false,
        )?;
        Ok((specialized_function_id, typechecked_exprs))
    }

    fn eval_block_stmt(
        &mut self,
        stmt: &ParsedStmt,
        scope_id: ScopeId,
        expected_type: Option<TypeId>,
    ) -> TyperResult<TypedStmt> {
        match stmt {
            ParsedStmt::ValDef(val_def) => {
                let provided_type = match val_def.type_expr.as_ref() {
                    None => None,
                    Some(&type_expr) => Some(self.eval_type_expr(type_expr, scope_id, None)?),
                };
                let value_expr = self.eval_expr(val_def.value, scope_id, provided_type)?;
                let actual_type = value_expr.get_type();
                let variable_type = if let Some(expected_type) = provided_type {
                    if let Err(msg) = self.check_types(expected_type, actual_type, scope_id) {
                        return make_fail_span(
                            format!("Local variable type mismatch: {}", msg),
                            val_def.span,
                        );
                    }
                    expected_type
                } else {
                    actual_type
                };

                let variable_id = self.variables.add_variable(Variable {
                    is_mutable: val_def.is_mutable,
                    name: val_def.name,
                    type_id: variable_type,
                    owner_scope: scope_id,
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
            ParsedStmt::Assignment(assignment) => {
                // let lhs = self.eval_expr(&assignment.lhs, scope_id, None)?;
                let lhs = self.eval_assignment_lhs_expr(assignment.lhs, scope_id, None)?;
                let rhs = self.eval_expr(assignment.rhs, scope_id, Some(lhs.get_type()))?;
                if let Err(msg) = self.check_types(lhs.get_type(), rhs.get_type(), scope_id) {
                    return make_fail_span(
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
            ParsedStmt::LoneExpression(expression) => {
                let expr = self.eval_expr(*expression, scope_id, expected_type)?;
                Ok(TypedStmt::Expr(Box::new(expr)))
            }
            ParsedStmt::While(while_stmt) => {
                let cond = self.eval_expr(while_stmt.cond, scope_id, Some(BOOL_TYPE_ID))?;
                if let Err(e) = self.check_types(BOOL_TYPE_ID, cond.get_type(), scope_id) {
                    return make_fail_span(
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
        let mut last_expr_type: TypeId = UNIT_TYPE_ID;
        for (index, stmt) in block.stmts.iter().enumerate() {
            if last_expr_type == NEVER_TYPE_ID {
                return make_fail_span(
                    "Dead code following divergent statement",
                    self.ast.get_stmt_span(stmt),
                );
            }
            let is_last = index == block.stmts.len() - 1;
            let expected_type = if is_last { expected_type } else { None };

            let stmt = self.eval_block_stmt(stmt, scope_id, expected_type)?;
            last_expr_type = stmt.get_type();
            statements.push(stmt);
        }

        let typed_block =
            TypedBlock { expr_type: last_expr_type, scope_id, statements, span: block.span };
        Ok(typed_block)
    }

    fn resolve_intrinsic_function_type(
        &self,
        fn_name: IdentifierId,
        namespace_chain: &[IdentifierId],
        specialization_params: Option<&SpecializationParams>,
    ) -> Result<IntrinsicFunction, String> {
        let fn_name_str = self.ast.identifiers.get_name(fn_name);
        let second = namespace_chain.get(1).map(|id| self.get_ident_str(*id));
        // eprintln!(
        //     "Resolve intrinsic chain {} {:?}",
        //     second.unwrap_or("None"),
        //     specialization_params
        // );
        let result = if let Some((ability_id, ability_impl_type_id)) =
            specialization_params.and_then(|params| params.ability_impl_info)
        {
            match (ability_id, self.types.get(ability_impl_type_id)) {
                (EQUALS_ABILITY_ID, Type::String) => {
                    if fn_name_str == "equals" {
                        Some(IntrinsicFunction::StringEquals)
                    } else {
                        None
                    }
                }
                (BITWISE_ABILITY_ID, Type::Integer(_)) => match fn_name_str {
                    "not" => Some(IntrinsicFunction::BitNot),
                    "and" => Some(IntrinsicFunction::BitAnd),
                    "or" => Some(IntrinsicFunction::BitOr),
                    "xor" => Some(IntrinsicFunction::BitXor),
                    "shiftLeft" => Some(IntrinsicFunction::BitShiftLeft),
                    "shiftRight" => Some(IntrinsicFunction::BitShiftRight),
                    _ => None,
                },
                _ => None,
            }
        } else {
            match second {
                // _root
                None => match fn_name_str {
                    "printInt" => Some(IntrinsicFunction::PrintInt),
                    "print" => Some(IntrinsicFunction::PrintString),
                    "exit" => Some(IntrinsicFunction::Exit),
                    "sizeOf" => Some(IntrinsicFunction::SizeOf),
                    "alignOf" => Some(IntrinsicFunction::AlignOf),
                    _ => None,
                },
                Some("string") => match fn_name_str {
                    "length" => Some(IntrinsicFunction::StringLength),
                    "fromChars" => Some(IntrinsicFunction::StringFromCharArray),
                    // Ability impl, won't work just yet
                    "equals" => Some(IntrinsicFunction::StringEquals),
                    _ => None,
                },
                Some("Array") => match fn_name_str {
                    "length" => Some(IntrinsicFunction::ArrayLength),
                    "capacity" => Some(IntrinsicFunction::ArrayCapacity),
                    "grow" => Some(IntrinsicFunction::ArrayGrow),
                    "new" => Some(IntrinsicFunction::ArrayNew),
                    "set_length" => Some(IntrinsicFunction::ArraySetLength),
                    _ => None,
                },
                Some("char") => None,
                Some("RawPointer") => match fn_name_str {
                    "asUnsafe" => Some(IntrinsicFunction::RawPointerToReference),
                    _ => None,
                },
                Some("Bits") => match fn_name_str {
                    "not" => Some(IntrinsicFunction::BitNot),
                    "and" => Some(IntrinsicFunction::BitAnd),
                    "or" => Some(IntrinsicFunction::BitOr),
                    "xor" => Some(IntrinsicFunction::BitXor),
                    "shiftLeft" => Some(IntrinsicFunction::BitShiftLeft),
                    "shiftRight" => Some(IntrinsicFunction::BitShiftRight),
                    _ => None,
                },
                Some(_) => None,
            }
        };
        match result {
            Some(result) => Ok(result),
            None => {
                return Err(format!(
                    "Could not resolve intrinsic function type for function {}",
                    &*self.get_ident_str(fn_name),
                ))
            }
        }
    }

    fn eval_function_predecl(
        &mut self,
        parsed_function_id: ParsedFunctionId,
        // Note: messy: parent_scope_id is only used if not specializing
        parent_scope_id: ScopeId,
        specialization_params: Option<SpecializationParams>,
        is_ability_decl: bool,
        namespace_id: NamespaceId,
    ) -> TyperResult<FunctionId> {
        let namespace = self.namespaces.get(namespace_id);
        let namespace_name = namespace.name;
        let specialization_params = specialization_params.as_ref();
        let companion_type_id = namespace.companion_type_id;
        let specialize = specialization_params.is_some();
        let parsed_function = self.ast.get_function(parsed_function_id);
        let parsed_function_linkage = parsed_function.linkage;
        let parsed_function_ret_type = parsed_function.ret_type;
        let parsed_function_name = parsed_function.name;
        let parsed_function_span = parsed_function.span;
        let parsed_function_args = parsed_function.args.clone();
        let name =
            specialization_params.map(|params| params.new_name).unwrap_or(parsed_function.name);
        let fn_scope_id = match specialization_params.map(|params| params.fn_scope_id) {
            None => self.scopes.add_child_scope(
                parent_scope_id,
                ScopeType::FunctionScope,
                None,
                Some(name),
            ),
            Some(fn_scope_id) => fn_scope_id,
        };
        // Some madness to get the actual enclosing scope
        let parent_scope_id =
            self.scopes.get_scope(fn_scope_id).parent.unwrap_or(self.scopes.get_root_scope_id());

        // Instantiate type arguments
        let mut type_params: Vec<TypeParam> = Vec::with_capacity(parsed_function.type_args.len());
        if !specialize {
            for type_parameter in parsed_function.type_args.iter() {
                let type_variable = TypeVariable {
                    name: type_parameter.ident,
                    scope_id: fn_scope_id,
                    _constraints: None,
                };
                let type_variable_id = self.types.add_type(Type::TypeVariable(type_variable));
                let fn_scope = self.scopes.get_scope_mut(fn_scope_id);
                let type_param =
                    TypeParam { ident: type_parameter.ident, type_id: type_variable_id };
                type_params.push(type_param);
                if !fn_scope.add_type(type_parameter.ident, type_variable_id) {
                    return make_fail_span("Generic type {} already exists", type_parameter.span);
                }
            }
        }
        trace!(
            "Added type arguments to function {} scope {:?}",
            &*self.get_ident_str(parsed_function.name),
            self.scopes.get_scope(fn_scope_id)
        );

        // Declare arguments
        let mut params = Vec::new();
        let mut is_method_of = None;
        for (idx, fn_arg) in parsed_function_args.iter().enumerate() {
            let type_id = self.eval_type_expr(fn_arg.ty, fn_scope_id, None)?;

            // First arg Self shenanigans
            if idx == 0 && !specialize {
                let name_is_self = self.ast.identifiers.get_name(fn_arg.name) == "self";

                // If the first argument is named self, check if it's a method of the companion type
                let is_ability_fn = is_ability_decl
                    || specialization_params
                        .is_some_and(|params| params.ability_impl_info.is_some());
                if name_is_self && !is_ability_fn {
                    if let Some(companion_type_id) = companion_type_id {
                        if self.types.get_type_id_dereferenced(type_id) == companion_type_id {
                            is_method_of = Some(companion_type_id);
                        } else {
                            match (self.types.get(companion_type_id), self.types.get(type_id)) {
                                (Type::Generic(_g), instance) => {
                                    let ok = instance.defn_info().is_some_and(|info| {
                                        info.generic_parent == Some(companion_type_id)
                                    });
                                    if !ok {
                                        return make_fail_span(
                                            "First argument named 'self' must be of the companion type",
                                            fn_arg.span,
                                        );
                                    }
                                }
                                _other => {
                                    return ffail!(
                                        fn_arg.span,
                                        "First argument named 'self' must be of the companion type, expected {} got {}, {} vs {}",
                                        self.type_id_to_string(companion_type_id),
                                        self.type_id_to_string(type_id),
                                        companion_type_id,
                                        type_id
                                    );
                                }
                            }
                        }
                    } else {
                        if self.types.get(type_id).as_array().is_some() {
                            // Special-case to allow Array methods since its a builtin generic so there's
                            // no companion type id
                        } else {
                            let ns = self.namespaces.get(namespace_id);
                            dbg!(
                                self.get_ident_str(namespace_name),
                                ns.companion_type_id,
                                companion_type_id
                            );
                            return make_fail_span(
                                "Cannot use name 'self' unless defining a method",
                                fn_arg.span,
                            );
                        }
                    }
                };

                // For an ability function, the first argument MUST be of type Self
                if is_ability_decl {
                    let type_is_self = self
                        .types
                        .get(type_id)
                        .as_tvar()
                        .map(|tvar| self.get_ident_str(tvar.name) == "Self")
                        .unwrap_or(false);
                    if !type_is_self || !name_is_self {
                        return make_fail_span(
                            "First argument of ability function must be self: Self",
                            fn_arg.span,
                        );
                    }
                }
            }

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
                owner_scope: fn_scope_id,
            };

            let variable_id = self.variables.add_variable(variable);
            params.push(FnArgDefn {
                name: fn_arg.name,
                variable_id,
                position: idx as u32,
                type_id,
                span: fn_arg.span,
            });
            self.scopes.add_variable(fn_scope_id, fn_arg.name, variable_id);
        }

        let intrinsic_type = if let Some(known_intrinsic) =
            specialization_params.and_then(|params| params.known_intrinsic)
        {
            Some(known_intrinsic)
        } else if parsed_function_linkage == Linkage::Intrinsic {
            let mut namespace_chain = self.namespaces.name_chain(namespace_id);
            let resolved = self
                .resolve_intrinsic_function_type(
                    parsed_function_name,
                    namespace_chain.make_contiguous(),
                    specialization_params,
                )
                .map_err(|msg| {
                    make_error(
                        format!("Error typechecking function: {}", msg),
                        parsed_function_span,
                    )
                })?;
            Some(resolved)
        } else {
            None
        };
        let given_ret_type = match parsed_function_ret_type {
            None => UNIT_TYPE_ID,
            Some(type_expr) => self.eval_type_expr(type_expr, fn_scope_id, None)?,
        };
        let metadata = if is_ability_decl {
            TypedFunctionMetadata::AbilityDefn(parsed_function_id)
        } else if let Some(spec_params) = specialization_params {
            if spec_params.ability_impl_info.is_some() {
                TypedFunctionMetadata::AbilityImpl {
                    generic_defn: spec_params.generic_parent_function,
                    impl_parsed_function_id: parsed_function_id,
                }
            } else {
                TypedFunctionMetadata::Specialization {
                    generic_parsed_function_id: parsed_function_id,
                    generic_defn: spec_params.generic_parent_function,
                }
            }
        } else {
            TypedFunctionMetadata::Standard { parsed_function_id, is_method_of }
        };
        let function = TypedFunction {
            name,
            scope: fn_scope_id,
            ret_type: given_ret_type,
            params,
            type_params,
            block: None,
            intrinsic_type,
            linkage: parsed_function_linkage,
            specializations: Vec::new(),
            metadata,
            span: parsed_function_span,
        };
        let function_id = self.add_function(function);

        // We do not want to resolve specialized functions by name!
        // So don't add them to any scope.
        if !specialize {
            if !self.scopes.add_function(parent_scope_id, parsed_function_name, function_id) {
                return make_fail_span(&format!("Function name is taken"), parsed_function_span);
            }
        }

        // Only if this isn't a specialization, add the ast mapping
        if let TypedFunctionMetadata::Specialization { .. } = metadata {
        } else {
            self.function_ast_mappings.insert(metadata.parsed_function_id(), function_id);
        }

        self.scopes.set_scope_owner_id(fn_scope_id, ScopeOwnerId::Function(function_id));

        Ok(function_id)
    }

    fn get_root_namespace_id(&self) -> NamespaceId {
        NamespaceId(0)
    }

    fn eval_function_body(&mut self, declaration_id: FunctionId) -> TyperResult<()> {
        let function = self.get_function(declaration_id);
        let function_name = function.name;
        let fn_scope_id = function.scope;
        let given_ret_type = function.ret_type;
        let is_extern = function.linkage == Linkage::External;
        let ast_id = function.metadata.parsed_function_id();
        let is_intrinsic = function.intrinsic_type.is_some();

        let ast_fn_def = self.ast.get_function(ast_id);
        let function_span = ast_fn_def.span;

        let body_block = match ast_fn_def.block.as_ref() {
            Some(block_ast) => {
                // TODO(clone): Intern blocks
                let block_ast = block_ast.clone();
                let block = self.eval_block(&block_ast, fn_scope_id, Some(given_ret_type))?;
                if let Err(msg) = self.check_types(given_ret_type, block.expr_type, fn_scope_id) {
                    return make_fail_span(
                        format!(
                            "Function {} return type mismatch: {}",
                            &*self.get_ident_str(function_name),
                            msg
                        ),
                        function_span,
                    );
                } else {
                    Some(block)
                }
            }
            None if is_intrinsic || is_extern => None,
            None => return make_fail_span("function is missing implementation", function_span),
        };
        // Add the body now
        if let Some(body_block) = body_block {
            self.get_function_mut(declaration_id).block = Some(body_block);
        }
        Ok(())
    }

    fn specialize_function(
        &mut self,
        parsed_function_id: ParsedFunctionId,
        specialization_params: SpecializationParams,
    ) -> TyperResult<FunctionId> {
        let specialized_function_id = self.eval_function_predecl(
            parsed_function_id,
            // messy: This scope is unused when specializing, so we just pass the root
            self.scopes.get_root_scope_id(),
            Some(specialization_params),
            false,
            self.get_root_namespace_id(),
        )?;
        self.eval_function_body(specialized_function_id)?;
        Ok(specialized_function_id)
    }

    fn eval_ability_defn(
        &mut self,
        parsed_ability_id: ParsedAbilityId,
        scope_id: ScopeId,
    ) -> TyperResult<()> {
        let parsed_ability = self.ast.get_ability(parsed_ability_id).clone();
        let parent_namespace_id = self
            .namespaces
            .namespace_for_scope(scope_id)
            .expect("should be directly inside a namespace");
        let ability_scope_id = self.scopes.add_child_scope(
            scope_id,
            ScopeType::AbilityDefn,
            None,
            Some(parsed_ability.name),
        );
        // Open up the scope for the ability, and add type variable "Self" into scope
        let self_ident_id = {
            let this = &mut self.ast;
            this.identifiers.intern("Self")
        };
        let self_type_id = self.types.add_type(Type::TypeVariable(TypeVariable {
            name: self_ident_id,
            scope_id: ability_scope_id,
            _constraints: None,
        }));
        if !self.scopes.get_scope_mut(ability_scope_id).add_type(self_ident_id, self_type_id) {
            panic!("Self already exists in ability scope?")
        };

        // Make a namespace for the ability
        let ability_namespace = Namespace {
            name: parsed_ability.name,
            scope_id: ability_scope_id,
            namespace_type: NamespaceType::Ability,
            companion_type_id: None,
            parent_id: Some(parent_namespace_id),
        };
        let namespace_id = self.namespaces.add(ability_namespace);
        let ns_added =
            self.scopes.get_scope_mut(scope_id).add_namespace(parsed_ability.name, namespace_id);
        if !ns_added {
            return ffail!(
                parsed_ability.span,
                "Namespace with name {} already exists",
                self.get_ident_str(parsed_ability.name)
            );
        }

        let mut typed_functions: Vec<TypedAbilityFunctionRef> =
            Vec::with_capacity(parsed_ability.functions.len());
        for parsed_function_id in parsed_ability.functions.iter() {
            let function_id = self.eval_function_predecl(
                *parsed_function_id,
                ability_scope_id,
                None,
                true,
                namespace_id,
            )?;
            let function_name = self.ast.get_function(*parsed_function_id).name;
            typed_functions.push(TypedAbilityFunctionRef { function_name, function_id });
        }
        let typed_ability = TypedAbility {
            name: parsed_ability.name,
            functions: typed_functions,
            scope_id: ability_scope_id,
            ast_id: parsed_ability.id,
        };
        let ability_id = self.add_ability(typed_ability);
        let added = self
            .scopes
            .get_scope_mut(self.scopes.get_root_scope_id())
            .add_ability(parsed_ability.name, ability_id);
        if !added {
            return ffail!(
                parsed_ability.span,
                "Ability with name {} already exists",
                self.get_ident_str(parsed_ability.name)
            );
        }
        self.scopes.set_scope_owner_id(ability_scope_id, ScopeOwnerId::Ability(ability_id));
        Ok(())
    }

    fn eval_ability_impl_decl(
        &mut self,
        parsed_id: ParsedAbilityImplId,
        scope_id: ScopeId,
    ) -> TyperResult<AbilityImplId> {
        let parsed_ability_impl = self.ast.get_ability_impl(parsed_id);
        let span = parsed_ability_impl.span;
        let ability_name = parsed_ability_impl.ability_name;
        let parsed_functions = parsed_ability_impl.functions.clone();
        // FIXME: Search from current scope. I'm just not sure how we want to do ability scoping
        let Some(ability_id) = self.scopes.get_root_scope().find_ability(ability_name) else {
            return make_fail_span(
                format!("Ability does not exist: {}", self.get_ident_str(ability_name)),
                span,
            );
        };
        let target_type = self.eval_type_expr(parsed_ability_impl.target_type, scope_id, None)?;

        // Scoping / orphan / coherence: For now, let's globally allow only one implementation per (Ability, Target Type) pair
        // Check for existing implementation
        for existing_impl in &self.ability_impls {
            if existing_impl.ability_id == ability_id && existing_impl.type_id == target_type {
                return make_fail_span(
                    format!(
                        "Ability '{}' already implemented for type: {}",
                        &*self.get_ident_str(ability_name).blue(),
                        self.type_id_to_string(target_type).blue()
                    ),
                    span,
                );
            }
        }

        let ability = self.get_ability(ability_id);
        let ability_scope_id = ability.scope_id;
        let mut typed_functions = Vec::new();
        // Note(clone): TypedAbilityFunctionRef is super cheap to clone
        for ability_function_ref in &ability.functions.clone() {
            let Some((parsed_impl_function_id, impl_function_span)) =
                parsed_functions.iter().find_map(|&fn_id| {
                    let the_fn = self.ast.get_function(fn_id);
                    if the_fn.name == ability_function_ref.function_name {
                        Some((fn_id, the_fn.span))
                    } else {
                        None
                    }
                })
            else {
                return make_fail_span(
                    format!(
                        "Missing implementation for function '{}' in ability '{}'",
                        &*self.get_ident_str(ability_function_ref.function_name).blue(),
                        &*self.get_ident_str(ability_name).blue()
                    ),
                    span,
                );
            };
            let function_name = ability_function_ref.function_name;
            // Make a scope for the impl function
            let new_name =
                format!("{}_impl_{}", &*self.ast.identifiers.get_name(function_name), target_type);
            let new_name_ident = self.ast.identifiers.intern(&new_name);
            let spec_fn_scope_id = self.scopes.add_child_scope(
                ability_scope_id,
                ScopeType::FunctionScope,
                None,
                Some(new_name_ident),
            );
            // Bind 'Self' = target_type
            // Discard because we just made this scope
            let _ = self
                .scopes
                .get_scope_mut(spec_fn_scope_id)
                .add_type(self.ast.identifiers.intern("Self"), target_type);

            let function_impl = self.eval_function_predecl(
                parsed_impl_function_id,
                // messy: This scope is unused when specializing, so we just pass the root
                self.scopes.get_root_scope_id(),
                Some(SpecializationParams {
                    fn_scope_id: spec_fn_scope_id,
                    new_name: new_name_ident,
                    known_intrinsic: None,
                    generic_parent_function: ability_function_ref.function_id,
                    ability_impl_info: Some((ability_id, target_type)),
                }),
                false,
                self.get_root_namespace_id(),
            )?;

            let specialized = self.get_function(function_impl);
            let generic = self.get_function(ability_function_ref.function_id);
            if specialized.params.len() != generic.params.len() {
                return make_fail_span(
                    format!(
                        "Invalid implementation of {} in ability {}: wrong number of parameters",
                        self.ast.identifiers.get_name(ability_function_ref.function_name),
                        self.ast.identifiers.get_name(ability_name)
                    ),
                    impl_function_span,
                );
            }
            for (index, specialized_param) in specialized.params.iter().enumerate() {
                let generic_param = &generic.params[index];
                if let Err(msg) = self.check_types(
                    generic_param.type_id,
                    specialized_param.type_id,
                    spec_fn_scope_id,
                ) {
                    return make_fail_span(
                        format!(
                            "Invalid implementation of {} in ability {} for parameter {}: {}",
                            self.ast.identifiers.get_name(ability_function_ref.function_name),
                            self.ast.identifiers.get_name(ability_name),
                            self.ast.identifiers.get_name(generic_param.name),
                            msg
                        ),
                        impl_function_span,
                    );
                }
            }
            if let Err(msg) =
                self.check_types(generic.ret_type, specialized.ret_type, spec_fn_scope_id)
            {
                return make_fail_span(
                    format!(
                        "Invalid implementation of '{}' in ability '{}': Wrong return type: {}",
                        self.ast.identifiers.get_name(ability_function_ref.function_name),
                        self.ast.identifiers.get_name(ability_name),
                        msg
                    ),
                    impl_function_span,
                );
            }

            typed_functions.push(function_impl);
        }

        let typed_impl_id = self.add_ability_impl(TypedAbilityImpl {
            type_id: target_type,
            ability_id,
            functions: typed_functions,
            span,
        });

        self.ability_impl_ast_mappings.insert(parsed_id, typed_impl_id);
        Ok(typed_impl_id)
    }

    /// All we have to do is fill in the function bodies; the prior phase has already done all
    /// the work
    fn eval_ability_impl(
        &mut self,
        parsed_ability_impl_id: ParsedAbilityImplId,
        _scope_id: ScopeId,
    ) -> TyperResult<()> {
        let ability_impl_id = *self.ability_impl_ast_mappings.get(&parsed_ability_impl_id).unwrap();
        let ability_impl = self.get_ability_impl(ability_impl_id);

        for impl_fn in ability_impl.functions.clone().iter() {
            self.eval_function_body(*impl_fn)?
        }

        Ok(())
    }

    fn eval_definition(&mut self, def: ParsedId, scope_id: ScopeId) -> TyperResult<()> {
        match def {
            ParsedId::Namespace(namespace) => {
                self.eval_namespace(namespace)?;
                Ok(())
            }
            ParsedId::Constant(_const_val) => {
                // Nothing to do in this phase for a const
                Ok(())
            }
            ParsedId::Function(parsed_function_id) => {
                let function_declaration_id = self
                    .function_ast_mappings
                    .get(&parsed_function_id)
                    .expect("function predecl lookup failed");
                self.eval_function_body(*function_declaration_id)?;
                Ok(())
            }
            ParsedId::TypeDefn(_type_defn_id) => {
                // Done in prior phase
                Ok(())
            }
            ParsedId::Ability(_ability) => {
                // Nothing to do in this phase for an ability
                Ok(())
            }
            ParsedId::AbilityImpl(ability_impl) => {
                self.eval_ability_impl(ability_impl, scope_id)?;
                Ok(())
            }
            other_id => {
                panic!("Was asked to eval definition of a non-definition ast node {:?}", other_id)
            }
        }
    }

    fn eval_namespace_type_defn_phase(
        &mut self,
        parsed_namespace_id: ParsedNamespaceId,
    ) -> TyperResult<()> {
        let namespace_id = *self.namespace_ast_mappings.get(&parsed_namespace_id).unwrap();
        let namespace_scope_id = self.namespaces.get(namespace_id).scope_id;
        for &parsed_definition_id in
            self.ast.get_namespace(parsed_namespace_id).definitions.clone().iter()
        {
            if let ParsedId::TypeDefn(type_defn_id) = parsed_definition_id {
                let parsed_type_defn = self.ast.get_type_defn(type_defn_id).clone();
                let added = self
                    .scopes
                    .get_scope_mut(namespace_scope_id)
                    .add_pending_type_defn(parsed_type_defn.name, type_defn_id);
                if !added {
                    return ffail!(
                        parsed_type_defn.span,
                        "Type {} exists",
                        self.get_ident_str(parsed_type_defn.name)
                    );
                }
            }
            if let ParsedId::Namespace(namespace_id) = parsed_definition_id {
                self.eval_namespace_type_defn_phase(namespace_id)?;
            }
        }
        Ok(())
    }

    fn eval_namespace_type_eval_phase(
        &mut self,
        parsed_namespace_id: ParsedNamespaceId,
    ) -> TyperResult<()> {
        let namespace_id = self.namespace_ast_mappings.get(&parsed_namespace_id).unwrap();
        let namespace = self.namespaces.get(*namespace_id);
        let namespace_scope_id = namespace.scope_id;
        let parsed_namespace = self.ast.get_namespace(parsed_namespace_id);

        for parsed_defn_id in parsed_namespace.definitions.clone().iter() {
            if let ParsedId::TypeDefn(type_defn_id) = parsed_defn_id {
                self.eval_type_defn(*type_defn_id, namespace_scope_id)?;
            }
            if let ParsedId::Namespace(namespace_id) = parsed_defn_id {
                self.eval_namespace_type_eval_phase(*namespace_id)?;
            }
        }
        Ok(())
    }

    fn eval_namespace_declaration_phase(
        &mut self,
        parsed_namespace_id: ParsedNamespaceId,
    ) -> TyperResult<()> {
        let parsed_namespace = self.ast.get_namespace(parsed_namespace_id);
        let namespace_id = *self.namespace_ast_mappings.get(&parsed_namespace_id).unwrap();
        let namespace = self.namespaces.get(namespace_id);
        let namespace_scope_id = namespace.scope_id;
        for defn in &parsed_namespace.definitions.clone() {
            self.eval_definition_declaration_phase(*defn, namespace_scope_id, namespace_id)?;
        }
        Ok(())
    }

    fn eval_namespace(&mut self, ast_namespace_id: ParsedNamespaceId) -> TyperResult<NamespaceId> {
        let ast_namespace = self.ast.get_namespace(ast_namespace_id).clone();
        let namespace_id = *self.namespace_ast_mappings.get(&ast_namespace.id).unwrap();
        let ns_scope_id = self.namespaces.get(namespace_id).scope_id;
        for defn in &ast_namespace.definitions {
            self.eval_definition(*defn, ns_scope_id)?;
        }
        Ok(namespace_id)
    }

    fn eval_definition_declaration_phase(
        &mut self,
        defn_id: ParsedId,
        scope_id: ScopeId,
        namespace_id: NamespaceId,
    ) -> TyperResult<()> {
        match defn_id {
            ParsedId::Namespace(namespace_id) => {
                self.eval_namespace_declaration_phase(namespace_id)?;
                Ok(())
            }
            ParsedId::Constant(constant_id) => {
                let _variable_id: VariableId = self.eval_const(constant_id)?;
                Ok(())
            }
            ParsedId::Function(parsed_function_id) => {
                self.eval_function_predecl(
                    parsed_function_id,
                    scope_id,
                    None,
                    false,
                    namespace_id,
                )?;
                Ok(())
            }
            ParsedId::TypeDefn(_type_defn_id) => {
                // Handled by prior phase
                Ok(())
            }
            ParsedId::Ability(parsed_ability_id) => {
                // FIXME: Move to type defn phase
                self.eval_ability_defn(parsed_ability_id, scope_id)?;
                Ok(())
            }
            ParsedId::AbilityImpl(ability_impl) => {
                let _impl_id = self.eval_ability_impl_decl(ability_impl, scope_id)?;
                Ok(())
            }
            other_id => {
                panic!("Was asked to eval definition of a non-definition ast node {:?}", other_id)
            }
        }
    }

    fn create_namespace(
        &mut self,
        parsed_namespace_id: ParsedNamespaceId,
        parent_scope: Option<ScopeId>,
    ) -> TyperResult<NamespaceId> {
        let ast_namespace = self.ast.get_namespace(parsed_namespace_id);
        let name = ast_namespace.name;
        let span = ast_namespace.span;

        match parent_scope {
            None => {
                let root_scope_id = self.scopes.add_root_scope(Some(name));
                let namespace = Namespace {
                    name,
                    scope_id: root_scope_id,
                    namespace_type: NamespaceType::Root,
                    companion_type_id: None,
                    parent_id: None,
                };
                let root_namespace_id = self.namespaces.add(namespace);
                self.scopes
                    .set_scope_owner_id(root_scope_id, ScopeOwnerId::Namespace(root_namespace_id));

                self.namespace_ast_mappings.insert(parsed_namespace_id, root_namespace_id);
                Ok(root_namespace_id)
            }
            Some(parent_scope_id) => {
                let ns_scope_id = self.scopes.add_child_scope(
                    parent_scope_id,
                    ScopeType::Namespace,
                    None,
                    Some(name),
                );
                let parent_ns_id = self
                    .namespaces
                    .namespace_for_scope(parent_scope_id)
                    .expect("ns should be defined immediately above a namespace");

                let namespace = Namespace {
                    name,
                    scope_id: ns_scope_id,
                    namespace_type: NamespaceType::User,
                    companion_type_id: None,
                    parent_id: Some(parent_ns_id),
                };
                let namespace_id = self.namespaces.add(namespace);
                self.scopes.set_scope_owner_id(ns_scope_id, ScopeOwnerId::Namespace(namespace_id));

                let parent_scope = self.scopes.get_scope_mut(parent_scope_id);
                if !parent_scope.add_namespace(name, namespace_id) {
                    return ffail!(
                        span,
                        "Namespace name {} is taken",
                        self.get_ident_str(name).blue()
                    );
                }

                self.namespace_ast_mappings.insert(parsed_namespace_id, namespace_id);
                Ok(namespace_id)
            }
        }
    }

    fn eval_namespace_ns_phase(
        &mut self,
        parsed_namespace_id: ParsedNamespaceId,
        parent_scope: Option<ScopeId>,
    ) -> TyperResult<NamespaceId> {
        let ast_namespace = self.ast.get_namespace(parsed_namespace_id).clone();
        let namespace_id = self.create_namespace(parsed_namespace_id, parent_scope)?;

        let namespace_scope_id = self.namespaces.get(namespace_id).scope_id;

        for defn in &ast_namespace.definitions {
            if let ParsedId::Namespace(namespace_id) = defn {
                let _namespace_id =
                    self.eval_namespace_ns_phase(*namespace_id, Some(namespace_scope_id))?;
            }
        }
        Ok(namespace_id)
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        let root_scope_id = self.scopes.get_root_scope_id();

        let root_namespace_id = self.ast.get_root_namespace().id;

        // Namespace phase
        eprintln!("**** ns phase begin ****");
        let ns_phase_res = self.eval_namespace_ns_phase(root_namespace_id, None);
        if let Err(e) = ns_phase_res {
            // TODO: I'm not sure if we can just keep going if this fails; the namespaces
            // won't even have scopes so a lot of things just won't work, and
            // may result in internal compiler errors instead of helpful ones
            print_error(&self.ast.spans, &self.ast.sources, &e.message, e.span);
            self.errors.push(e);
        }
        eprintln!("**** ns phase end ****");

        if !self.errors.is_empty() {
            bail!(
                "{} failed namespace declaration phase with {} errors",
                self.name(),
                self.errors.len()
            )
        }

        // Pending Type declaration phase
        eprintln!("type **** defn phase begin ****");
        let type_defn_result = self.eval_namespace_type_defn_phase(root_namespace_id);
        if let Err(e) = type_defn_result {
            print_error(&self.ast.spans, &self.ast.sources, &e.message, e.span);
            self.errors.push(e);
        }
        eprintln!("**** type defn phase end ****");

        // Type evaluation phase
        eprintln!("type **** eval phase begin ****");
        let type_eval_result = self.eval_namespace_type_eval_phase(root_namespace_id);
        if let Err(e) = type_eval_result {
            print_error(&self.ast.spans, &self.ast.sources, &e.message, e.span);
            self.errors.push(e);
        }
        let pendings = self.scopes.all_pending_type_defns_below(self.scopes.get_root_scope_id());
        if !pendings.is_empty() {
            for pending in pendings.iter() {
                let defn = self.ast.get_type_defn(*pending);
                dbg!(self.get_ident_str(defn.name));
            }
            panic!("Unevaluated type defns!!!")
        }
        eprintln!("**** type eval phase end ****");

        // Everything else declaration phase
        let root_ns_id = NamespaceId(0);
        eprintln!("**** declaration phase begin ****");
        for &parsed_definition_id in self.ast.get_root_namespace().definitions.clone().iter() {
            let result = self.eval_definition_declaration_phase(
                parsed_definition_id,
                root_scope_id,
                root_ns_id,
            );
            if let Err(e) = result {
                print_error(&self.ast.spans, &self.ast.sources, &e.message, e.span);
                self.errors.push(e);
            }
        }
        eprintln!("**** declaration phase end ****");
        if !self.errors.is_empty() {
            bail!("{} failed declaration phase with {} errors", self.name(), self.errors.len())
        }

        debug_assert!(
            self.get_ability(EQUALS_ABILITY_ID).name == self.ast.identifiers.get("Equals").unwrap()
        );
        debug_assert!(
            self.get_ability(BITWISE_ABILITY_ID).name
                == self.ast.identifiers.get("Bitwise").unwrap()
        );

        // Everything else evaluation phase
        for &parsed_definition_id in self.ast.get_root_namespace().definitions.clone().iter() {
            let result = self.eval_definition(parsed_definition_id, root_scope_id);
            if let Err(e) = result {
                print_error(&self.ast.spans, &self.ast.sources, &e.message, e.span);
                self.errors.push(e);
            }
        }
        if !self.errors.is_empty() {
            debug!("{}", self);
            bail!("{} failed typechecking with {} errors", self.name(), self.errors.len())
        }
        Ok(())
    }

    pub fn make_qualified_name(
        &self,
        scope: ScopeId,
        name: IdentifierId,
        delimiter: &str,
    ) -> String {
        let starting_namespace = self.scopes.nearest_parent_namespace(scope);
        let namespace_chain = self.namespaces.name_chain(starting_namespace);
        let mut s = String::new();
        for identifier in namespace_chain.iter() {
            let ident_str = self.get_ident_str(*identifier);
            if ident_str != "_root" {
                s.push_str(&ident_str);
                s.push_str(delimiter);
            }
        }
        s.push_str(&self.get_ident_str(name));
        s
    }
}
