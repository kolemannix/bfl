use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Deref;
use std::path::Path;
use std::rc::Rc;

use anyhow::Result;
use inkwell::attributes::AttributeLoc;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFile, DILocation, DIScope, DISubprogram, DIType, DWARFEmissionKind,
    DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::module::{Linkage as LlvmLinkage, Module as LlvmModule};
use inkwell::passes::{PassManager, PassManagerBuilder};
use inkwell::targets::{InitializationConfig, Target, TargetMachine, TargetTriple};
use inkwell::types::{
    BasicMetadataTypeEnum, BasicType, BasicTypeEnum, IntType, PointerType, StructType,
};
use inkwell::values::{
    ArrayValue, BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, GlobalValue,
    IntValue, PointerValue, StructValue,
};
use inkwell::{AddressSpace, IntPredicate, OptimizationLevel};
use log::trace;

use crate::lex::Span;
use crate::parse::{ForExprType, IdentifierId};
use crate::typer::{Linkage as TyperLinkage, *};

const STRING_LENGTH_FIELD_INDEX: u32 = 0;
const STRING_DATA_FIELD_INDEX: u32 = 1;

const ARRAY_LENGTH_FIELD_INDEX: u32 = 0;
const ARRAY_CAPACITY_FIELD_INDEX: u32 = 1;
const ARRAY_DATA_FIELD_INDEX: u32 = 2;

const WORD_SIZE_BITS: u64 = 64;

#[derive(Debug, Copy, Clone)]
struct LlvmPointerType<'ctx> {
    #[allow(unused)]
    type_id: TypeId,
    pointer_type: BasicTypeEnum<'ctx>,
    pointee_type: BasicTypeEnum<'ctx>,
}

#[derive(Debug, Copy, Clone)]
struct LlvmValueType<'ctx> {
    #[allow(unused)]
    type_id: TypeId,
    basic_type: BasicTypeEnum<'ctx>,
}

#[derive(Debug, Copy, Clone)]
enum LlvmType<'ctx> {
    Value(LlvmValueType<'ctx>),
    Pointer(LlvmPointerType<'ctx>),
}

impl<'ctx> From<LlvmValueType<'ctx>> for LlvmType<'ctx> {
    fn from(value: LlvmValueType<'ctx>) -> Self {
        LlvmType::Value(value)
    }
}

impl<'ctx> From<LlvmPointerType<'ctx>> for LlvmType<'ctx> {
    fn from(value: LlvmPointerType<'ctx>) -> Self {
        LlvmType::Pointer(value)
    }
}

impl<'ctx> LlvmType<'ctx> {
    pub fn expect_pointer(&self) -> LlvmPointerType<'ctx> {
        match self {
            LlvmType::Value(value) => panic!("expected pointer on value: {:?}", value),
            LlvmType::Pointer(pointer) => *pointer,
        }
    }

    #[allow(unused)]
    fn type_id(&self) -> TypeId {
        match self {
            LlvmType::Value(value) => value.type_id,
            LlvmType::Pointer(pointer) => pointer.type_id,
        }
    }

    fn value_type(&self) -> BasicTypeEnum<'ctx> {
        match self {
            LlvmType::Value(value) => value.basic_type,
            LlvmType::Pointer(pointer) => pointer.pointer_type,
        }
    }
}

struct BuiltinTypes<'ctx> {
    ctx: &'ctx Context,
    i64: IntType<'ctx>,
    unit: IntType<'ctx>,
    unit_value: IntValue<'ctx>,
    boolean: IntType<'ctx>,
    true_value: IntValue<'ctx>,
    false_value: IntValue<'ctx>,
    char: IntType<'ctx>,
    c_str: PointerType<'ctx>,
    string_struct: StructType<'ctx>,
}

impl<'ctx> BuiltinTypes<'ctx> {
    fn array_struct(&self, elem_type: BasicTypeEnum<'ctx>) -> StructType<'ctx> {
        // NOTE: All the 'length' types could probably be i32 but our language int type is only i64
        //       so we couldn't enforce that at the lang level right now
        let length_type = self.ctx.i64_type().as_basic_type_enum();
        let capacity_type = self.ctx.i64_type().as_basic_type_enum();
        let data_type = elem_type.ptr_type(AddressSpace::default()).as_basic_type_enum();
        self.ctx.struct_type(&[length_type, capacity_type, data_type], false)
    }

    fn array_length(&self, builder: &Builder<'ctx>, array: StructValue<'ctx>) -> IntValue<'ctx> {
        let length =
            builder.build_extract_value(array, ARRAY_LENGTH_FIELD_INDEX, "array_length").unwrap();
        length.into_int_value()
    }

    fn build_array_capacity(
        &self,
        builder: &Builder<'ctx>,
        array: StructValue<'ctx>,
    ) -> IntValue<'ctx> {
        let cap =
            builder.build_extract_value(array, ARRAY_CAPACITY_FIELD_INDEX, "array_cap").unwrap();
        cap.into_int_value()
    }
    fn array_data(&self, builder: &Builder<'ctx>, array: StructValue<'ctx>) -> PointerValue<'ctx> {
        builder
            .build_extract_value(array, ARRAY_DATA_FIELD_INDEX, "array_data")
            .unwrap()
            .into_pointer_value()
    }

    fn string_length(
        &self,
        builder: &Builder<'ctx>,
        string_struct: StructValue<'ctx>,
    ) -> IntValue<'ctx> {
        builder
            .build_extract_value(string_struct, STRING_LENGTH_FIELD_INDEX, "string_length")
            .unwrap()
            .into_int_value()
    }

    fn string_data(
        &self,
        builder: &Builder<'ctx>,
        string_struct: StructValue<'ctx>,
    ) -> PointerValue<'ctx> {
        builder
            .build_extract_value(string_struct, STRING_DATA_FIELD_INDEX, "string_data")
            .unwrap()
            .into_pointer_value()
    }
}

struct LibcFunctions<'ctx> {
    printf: FunctionValue<'ctx>,
    exit: FunctionValue<'ctx>,
    memcmp: FunctionValue<'ctx>,
}

pub struct Codegen<'ctx> {
    ctx: &'ctx Context,
    pub module: Rc<TypedModule>,
    llvm_module: LlvmModule<'ctx>,
    llvm_machine: Option<TargetMachine>,
    builder: Builder<'ctx>,
    llvm_functions: HashMap<FunctionId, FunctionValue<'ctx>>,
    llvm_types: RefCell<HashMap<TypeId, LlvmType<'ctx>>>,
    /// The type of stored pointers here should be one level higher than the type of the
    /// value pointed to. This is so that it can be used reliably, without matching or
    /// checking, as a Pointer to the actual type
    variables: HashMap<VariableId, Pointer<'ctx>>,
    globals: HashMap<VariableId, GlobalValue<'ctx>>,
    libc_functions: LibcFunctions<'ctx>,
    builtin_globals: HashMap<String, GlobalValue<'ctx>>,
    builtin_types: BuiltinTypes<'ctx>,
    default_address_space: AddressSpace,
    debug: DebugContext<'ctx>,
}

struct DebugContext<'ctx> {
    file: DIFile<'ctx>,
    debug_builder: DebugInfoBuilder<'ctx>,
    #[allow(unused)]
    compile_unit: DICompileUnit<'ctx>,
    debug_stack: Vec<DebugStackEntry<'ctx>>,
    #[allow(unused)]
    scopes: HashMap<ScopeId, DIScope<'ctx>>,
    strip_debug: bool,
}

impl<'ctx> DebugContext<'ctx> {
    fn push_scope(&mut self, scope: DIScope<'ctx>) {
        self.debug_stack.push(DebugStackEntry { scope });
    }
    fn pop_scope(&mut self) {
        self.debug_stack.pop();
    }
    fn current_scope(&self) -> DIScope<'ctx> {
        self.debug_stack.last().unwrap().scope
    }
}

#[derive(Copy, Clone, Debug)]
struct Pointer<'ctx> {
    #[allow(unused)]
    pointee_type_id: TypeId,
    pointee_llvm_type: BasicTypeEnum<'ctx>,
    pointer: PointerValue<'ctx>,
}

impl<'ctx> Pointer<'ctx> {
    fn loaded_value(&self, builder: &Builder<'ctx>) -> BasicValueEnum<'ctx> {
        builder.build_load(self.pointee_llvm_type, self.pointer, "loaded_value")
    }
}

type CodegenResult<T> = Result<T>;

struct DebugStackEntry<'ctx> {
    scope: DIScope<'ctx>,
}

fn i8_array_from_str<'ctx>(ctx: &'ctx Context, value: &str) -> ArrayValue<'ctx> {
    let bytes = value.as_bytes();
    ctx.const_string(bytes, false)
}

impl<'ctx> Codegen<'ctx> {
    fn init_debug(
        ctx: &'ctx Context,
        llvm_module: &LlvmModule<'ctx>,
        module: &TypedModule,
        optimize: bool,
        debug: bool,
    ) -> DebugContext<'ctx> {
        let (debug_builder, compile_unit) = llvm_module.create_debug_info_builder(
            false,
            DWARFSourceLanguage::C,
            &module.ast.source.filename,
            &module.ast.source.directory,
            "bfl_compiler",
            optimize,
            "",
            0,
            "",
            DWARFEmissionKind::Full,
            0,
            false,
            false,
            "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk",
            "MacOSX.sdk",
        );
        let md0 = ctx.metadata_node(&[
            ctx.i32_type().const_int(2, false).into(),
            ctx.metadata_string("SDK Version").into(),
            ctx.i32_type()
                .const_array(&[
                    ctx.i32_type().const_int(14, false),
                    ctx.i32_type().const_int(0, false),
                ])
                .into(),
        ]);
        let md1 = ctx.metadata_node(&[
            ctx.i32_type().const_int(2, false).into(),
            ctx.metadata_string("Dwarf Version").into(),
            ctx.i32_type().const_int(4, false).into(),
        ]);
        let md2 = ctx.metadata_node(&[
            ctx.i32_type().const_int(2, false).into(),
            ctx.metadata_string("Debug Info Version").into(),
            ctx.i32_type().const_int(3, false).into(),
        ]);
        let md3 = ctx.metadata_node(&[
            ctx.i32_type().const_int(1, false).into(),
            ctx.metadata_string("PIC Level").into(),
            ctx.i32_type().const_int(2, false).into(),
        ]);
        let md4 = ctx.metadata_node(&[
            ctx.i32_type().const_int(1, false).into(),
            ctx.metadata_string("PIE Level").into(),
            ctx.i32_type().const_int(2, false).into(),
        ]);
        llvm_module.add_global_metadata("llvm.module.flags", &md0).unwrap();
        llvm_module.add_global_metadata("llvm.module.flags", &md1).unwrap();
        llvm_module.add_global_metadata("llvm.module.flags", &md2).unwrap();
        llvm_module.add_global_metadata("llvm.module.flags", &md3).unwrap();
        llvm_module.add_global_metadata("llvm.module.flags", &md4).unwrap();

        let di_file = compile_unit.get_file();
        let mut debug = DebugContext {
            file: di_file,
            debug_builder,
            compile_unit,
            debug_stack: Vec::new(),
            scopes: HashMap::new(),
            strip_debug: !debug,
        };
        debug.push_scope(di_file.as_debug_info_scope());
        debug
    }

    pub fn create(
        ctx: &'ctx Context,
        module: Rc<TypedModule>,
        debug: bool,
        optimize: bool,
    ) -> Codegen<'ctx> {
        let builder = ctx.create_builder();
        let char_type = ctx.i8_type();
        let llvm_module = ctx.create_module(&module.ast.name);
        llvm_module.set_source_file_name(&module.ast.source.filename);
        // Example of linking an LLVM module
        // let stdlib_module = ctx
        //     .create_module_from_ir(MemoryBuffer::create_from_file(Path::new("nxlib/llvm")).unwrap())
        //     .unwrap();
        // llvm_module.link_in_module(stdlib_module).unwrap();

        let debug_context = Codegen::init_debug(ctx, &llvm_module, &module, optimize, debug);

        let pointers = HashMap::new();
        let format_int_str = {
            let global = llvm_module.add_global(ctx.i8_type().array_type(3), None, "formatInt");
            global.set_constant(true);
            global.set_unnamed_addr(true);
            global.set_initializer(&i8_array_from_str(ctx, "%i\0"));
            global
        };
        let format_str_str = {
            let global = llvm_module.add_global(ctx.i8_type().array_type(5), None, "formatString");
            global.set_constant(true);
            global.set_unnamed_addr(true);
            global.set_initializer(&i8_array_from_str(ctx, "%.*s\0"));
            global
        };
        let globals = HashMap::new();
        let mut builtin_globals: HashMap<String, GlobalValue<'ctx>> = HashMap::new();
        builtin_globals.insert("formatInt".to_string(), format_int_str);
        builtin_globals.insert("formatString".to_string(), format_str_str);
        let string_struct = ctx.struct_type(
            &[
                ctx.i64_type().as_basic_type_enum(),
                char_type.ptr_type(AddressSpace::default()).as_basic_type_enum(),
            ],
            false,
        );

        let builtin_types = BuiltinTypes {
            ctx,
            i64: ctx.i64_type(),
            unit: ctx.bool_type(),
            unit_value: ctx.bool_type().const_int(0, false),
            boolean: ctx.bool_type(),
            true_value: ctx.bool_type().const_int(1, false),
            false_value: ctx.bool_type().const_int(0, false),
            char: char_type,
            c_str: char_type.ptr_type(AddressSpace::default()),
            string_struct,
        };

        let printf_type = ctx.i32_type().fn_type(&[builtin_types.c_str.into()], true);
        let printf = llvm_module.add_function("printf", printf_type, Some(LlvmLinkage::External));
        let exit = llvm_module.add_function(
            "exit",
            ctx.void_type().fn_type(&[builtin_types.i64.into()], false),
            Some(LlvmLinkage::External),
        );
        let byte_ptr = ctx.i8_type().ptr_type(AddressSpace::default());
        let memcmp = llvm_module.add_function(
            "memcmp",
            ctx.i32_type()
                .fn_type(&[byte_ptr.into(), byte_ptr.into(), ctx.i64_type().into()], false),
            Some(LlvmLinkage::External),
        );
        Codegen {
            ctx,
            module,
            llvm_module,
            llvm_machine: None,
            builder,
            variables: pointers,
            globals,
            llvm_functions: HashMap::new(),
            llvm_types: RefCell::new(HashMap::new()),
            libc_functions: LibcFunctions { printf, exit, memcmp },
            builtin_globals,
            builtin_types,
            default_address_space: AddressSpace::default(),
            debug: debug_context,
        }
    }

    fn set_debug_location(&self, span: Span) -> DILocation<'ctx> {
        let locn = self.debug.debug_builder.create_debug_location(
            self.ctx,
            self.module.get_line_number(span),
            1,
            self.debug.current_scope(),
            None,
        );
        self.builder.set_current_debug_location(locn);
        locn
    }

    fn get_ident_name(&self, id: IdentifierId) -> impl Deref<Target = str> + '_ {
        self.module.ast.get_ident_str(id)
    }

    fn build_print_int_call(&mut self, int_value: BasicValueEnum<'ctx>) -> BasicValueEnum<'ctx> {
        // TODO: Builtin globals could be a struct not a hashmap because its all static currently
        let format_str_global = self.builtin_globals.get("formatInt").unwrap();
        let call = self
            .builder
            .build_call(
                self.libc_functions.printf,
                &[format_str_global.as_pointer_value().into(), int_value.into()],
                "printf",
            )
            .try_as_basic_value()
            .left()
            .unwrap();
        call.set_name("print_int_res");
        call
    }

    fn build_print_string_call(&self, string_value: StructValue<'ctx>) -> BasicValueEnum<'ctx> {
        trace!("codegen print_string");
        let length = self.builtin_types.string_length(&self.builder, string_value);
        let data = self.builtin_types.string_data(&self.builder, string_value);

        let format_str = self.builtin_globals.get("formatString").unwrap();
        let call = self
            .builder
            .build_call(
                self.libc_functions.printf,
                &[format_str.as_pointer_value().into(), length.into(), data.into()],
                "printf",
            )
            .try_as_basic_value()
            .left()
            .unwrap();
        call.set_name("print_str_res");
        call
    }

    fn build_optional_type(&self, optional_type: &OptionalType) -> StructType<'ctx> {
        // Optional types are represented as a struct with a boolean
        // discriminant indicating whether the value is None, and the value itself
        let inner = self.codegen_type(optional_type.inner_type);
        self.ctx.struct_type(
            &[self.builtin_types.boolean.as_basic_type_enum(), inner.value_type()],
            false,
        )
    }

    fn get_debug_type(&self, type_id: TypeId) -> DIType<'ctx> {
        let _dw_ate_address = 0x01;
        let dw_ate_boolean = 0x02;
        let _dw_ate_complex_float = 0x03;
        let _dw_ate_float = 0x04;
        let dw_ate_signed = 0x05;
        let dw_ate_char = 0x06;
        let _dw_ate_unsigned = 0x07;
        let _dw_ate_unsigned_char = 0x08;

        match self.module.get_type(type_id) {
            Type::Unit => self
                .debug
                .debug_builder
                .create_basic_type("unit", 8, dw_ate_boolean, 0)
                .unwrap()
                .as_type(),
            Type::Bool => self
                .debug
                .debug_builder
                .create_basic_type("bool", 8, dw_ate_boolean, 0)
                .unwrap()
                .as_type(),
            Type::Char => self
                .debug
                .debug_builder
                .create_basic_type(
                    "char",
                    self.builtin_types.char.get_bit_width() as u64,
                    dw_ate_char,
                    0,
                )
                .unwrap()
                .as_type(),
            Type::Int => self
                .debug
                .debug_builder
                .create_basic_type(
                    "int",
                    self.builtin_types.i64.get_bit_width() as u64,
                    dw_ate_signed,
                    0,
                )
                .unwrap()
                .as_type(),
            Type::String => self
                .debug
                .debug_builder
                .create_struct_type(
                    self.debug.current_scope(),
                    "string",
                    self.debug.file,
                    0,
                    128,
                    128,
                    0,
                    None,
                    &[
                        self.debug
                            .debug_builder
                            .create_member_type(
                                self.debug.current_scope(),
                                "string_length",
                                self.debug.file,
                                0,
                                WORD_SIZE_BITS,
                                WORD_SIZE_BITS as u32,
                                0,
                                0,
                                self.get_debug_type(INT_TYPE_ID),
                            )
                            .as_type(),
                        self.debug
                            .debug_builder
                            .create_member_type(
                                self.debug.current_scope(),
                                "string_data",
                                self.debug.file,
                                0,
                                WORD_SIZE_BITS,
                                WORD_SIZE_BITS as u32,
                                WORD_SIZE_BITS,
                                0,
                                self.debug
                                    .debug_builder
                                    .create_pointer_type(
                                        "string_data",
                                        self.get_debug_type(CHAR_TYPE_ID),
                                        WORD_SIZE_BITS,
                                        WORD_SIZE_BITS as u32,
                                        self.default_address_space,
                                    )
                                    .as_type(),
                            )
                            .as_type(),
                    ],
                    0,
                    None,
                    "string",
                )
                .as_type(),
            Type::Record(record) => {
                // FIXME: We need to compute actual physical size at some point
                //        Probably here in codegen? But maybe in typecheck?
                //        What about a struct of our own, called CodegenedType or something,
                //        that includes the LLVM type, the Debug type, and the size, and other stuff
                //        we might want to know about a type
                let size = 0;
                let name = record
                    .name_if_named
                    .map(|ident| self.get_ident_name(ident).to_string())
                    .unwrap_or("<anon_record>".to_string());
                let mut offset = 0;
                self.debug
                    .debug_builder
                    .create_struct_type(
                        self.debug.current_scope(),
                        &name,
                        self.debug.file,
                        record.span.line_number(),
                        size,
                        size as u32,
                        0,
                        None,
                        &record
                            .fields
                            .iter()
                            .map(|f| {
                                let member_type = self.get_debug_type(f.type_id);
                                let t = self.debug.debug_builder.create_member_type(
                                    self.debug.current_scope(),
                                    &self.get_ident_name(f.name),
                                    self.debug.file,
                                    self.module.get_line_number(record.span),
                                    member_type.get_size_in_bits(),
                                    WORD_SIZE_BITS as u32,
                                    offset,
                                    0,
                                    member_type,
                                );
                                offset += member_type.get_size_in_bits();
                                t.as_type()
                            })
                            .collect::<Vec<_>>(),
                        0,
                        None,
                        &name,
                    )
                    .as_type()
            }
            Type::TypeVariable(v) => {
                println!("{}", self.module);
                panic!("codegen was asked to make debug info for a type variable {:?}", v)
            }
            Type::Array(array) => {
                let element_type = self.get_debug_type(array.element_type);
                let array_struct_ditype = self
                    .debug
                    .debug_builder
                    .create_struct_type(
                        self.debug.current_scope(),
                        &self.module.type_id_to_string(type_id),
                        self.debug.file,
                        0,
                        WORD_SIZE_BITS * 3,
                        WORD_SIZE_BITS as u32,
                        0,
                        None,
                        &[
                            self.debug
                                .debug_builder
                                .create_member_type(
                                    self.debug.current_scope(),
                                    "length",
                                    self.debug.file,
                                    0,
                                    WORD_SIZE_BITS,
                                    WORD_SIZE_BITS as u32,
                                    0,
                                    0,
                                    self.get_debug_type(INT_TYPE_ID),
                                )
                                .as_type(),
                            self.debug
                                .debug_builder
                                .create_member_type(
                                    self.debug.current_scope(),
                                    "capacity",
                                    self.debug.file,
                                    0,
                                    WORD_SIZE_BITS,
                                    WORD_SIZE_BITS as u32,
                                    WORD_SIZE_BITS,
                                    0,
                                    self.get_debug_type(INT_TYPE_ID),
                                )
                                .as_type(),
                            self.debug
                                .debug_builder
                                .create_member_type(
                                    self.debug.current_scope(),
                                    "data_ptr",
                                    self.debug.file,
                                    0,
                                    WORD_SIZE_BITS,
                                    WORD_SIZE_BITS as u32,
                                    128,
                                    0,
                                    self.debug
                                        .debug_builder
                                        .create_pointer_type(
                                            "array_data",
                                            element_type,
                                            WORD_SIZE_BITS,
                                            WORD_SIZE_BITS as u32,
                                            self.default_address_space,
                                        )
                                        .as_type(),
                                )
                                .as_type(),
                        ],
                        0,
                        None,
                        &self.module.type_id_to_string(type_id),
                    )
                    .as_type();
                self.debug
                    .debug_builder
                    .create_pointer_type(
                        &self.module.type_id_to_string(type_id),
                        array_struct_ditype,
                        WORD_SIZE_BITS,
                        WORD_SIZE_BITS as u32,
                        self.default_address_space,
                    )
                    .as_type()
            }
            Type::Optional(_optional) => {
                let name = format!("optional_{}", self.module.type_id_to_string(type_id));
                // TODO: We are punting on the debug type definition for optionals right now
                self.debug
                    .debug_builder
                    .create_struct_type(
                        self.debug.current_scope(),
                        &name,
                        self.debug.file,
                        0,
                        128,
                        128,
                        0,
                        None,
                        &[],
                        0,
                        None,
                        &name,
                    )
                    .as_type()
            }
            Type::Reference(reference_type) => {
                let name = format!("{}*", self.module.type_id_to_string(type_id));
                self.debug
                    .debug_builder
                    .create_pointer_type(
                        &name,
                        self.get_debug_type(reference_type.inner_type),
                        WORD_SIZE_BITS,
                        WORD_SIZE_BITS as u32,
                        self.default_address_space,
                    )
                    .as_type()
            }
        }
    }

    /// This needs to return pointee type if its a pointer, for convenience
    fn codegen_type(&self, type_id: TypeId) -> LlvmType<'ctx> {
        trace!("codegen for type {}", self.module.type_id_to_string(type_id));
        match type_id {
            UNIT_TYPE_ID => LlvmValueType {
                type_id: UNIT_TYPE_ID,
                basic_type: self.builtin_types.unit.as_basic_type_enum(),
            }
            .into(),
            CHAR_TYPE_ID => LlvmValueType {
                type_id: CHAR_TYPE_ID,
                basic_type: self.builtin_types.char.as_basic_type_enum(),
            }
            .into(),
            INT_TYPE_ID => LlvmValueType {
                type_id: INT_TYPE_ID,
                basic_type: self.builtin_types.i64.as_basic_type_enum(),
            }
            .into(),
            BOOL_TYPE_ID => LlvmValueType {
                type_id: BOOL_TYPE_ID,
                basic_type: self.builtin_types.boolean.as_basic_type_enum(),
            }
            .into(),
            STRING_TYPE_ID => {
                // FIXME: Worth declaring this as a named type
                //        for niceness in the IR?
                LlvmValueType {
                    type_id: STRING_TYPE_ID,
                    basic_type: self.builtin_types.string_struct.as_basic_type_enum(),
                }
                .into()
            }
            type_id => {
                let result = self.llvm_types.borrow().get(&type_id).copied();
                match result {
                    None => {
                        // Generate and store the type in here
                        let module = self.module.clone();
                        let ty = module.get_type(type_id);
                        let generated: LlvmType = match ty {
                            Type::Optional(optional) => LlvmValueType {
                                type_id,
                                basic_type: self.build_optional_type(optional).as_basic_type_enum(),
                            }
                            .into(),
                            Type::Record(record) => {
                                trace!("generating llvm type for record type {type_id}");
                                let field_types: Vec<_> = record
                                    .fields
                                    .iter()
                                    .map(|f| self.codegen_type(f.type_id).value_type())
                                    .collect();
                                let struct_type = self.ctx.struct_type(&field_types, false);
                                LlvmValueType {
                                    type_id,
                                    basic_type: struct_type.as_basic_type_enum(),
                                }
                                .into()
                            }
                            Type::TypeVariable(v) => {
                                println!("{}", self.module);
                                panic!("codegen was asked to codegen a type variable {:?}", v)
                            }
                            Type::Array(array) => {
                                let element_type = self.codegen_type(array.element_type);
                                let struct_type =
                                    self.builtin_types.array_struct(element_type.value_type());
                                LlvmPointerType {
                                    type_id,
                                    pointer_type: struct_type
                                        .ptr_type(AddressSpace::default())
                                        .as_basic_type_enum(),
                                    pointee_type: struct_type.as_basic_type_enum(),
                                }
                                .into()
                            }
                            Type::Reference(reference) => {
                                // Could also use any_ptr here
                                let inner_type = self.codegen_type(reference.inner_type);
                                LlvmPointerType {
                                    type_id,
                                    pointer_type: inner_type
                                        .value_type()
                                        .ptr_type(AddressSpace::default())
                                        .as_basic_type_enum(),
                                    pointee_type: inner_type.value_type(),
                                }
                                .into()
                            }
                            other => {
                                panic!(
                                    "get_llvm_type for type dropped through unexpectedly: {:?}",
                                    other
                                )
                            }
                        };
                        self.llvm_types.borrow_mut().insert(type_id, generated);
                        generated
                    }
                    Some(basic_ty) => basic_ty,
                }
            }
        }
    }

    fn codegen_val(&mut self, val: &ValDef) -> PointerValue<'ctx> {
        let value = self.codegen_expr_rvalue(&val.initializer);
        let variable_type = self.codegen_type(val.ty);
        let variable = self.module.get_variable(val.variable_id);
        let variable_ptr = self
            .builder
            .build_alloca(variable_type.value_type(), &self.get_ident_name(variable.name));
        trace!(
            "codegen_val {}: pointee_ty: {variable_type:?}",
            &*self.get_ident_name(variable.name)
        );
        // We're always storing a pointer
        // in self.variables that, when loaded, gives the actual type of the variable
        let store_instr = self.builder.build_store(variable_ptr, value.as_basic_value_enum());
        self.debug.debug_builder.insert_declare_before_instruction(
            variable_ptr,
            Some(self.debug.debug_builder.create_auto_variable(
                self.debug.current_scope(),
                &self.get_ident_name(variable.name),
                self.debug.file,
                self.module.get_line_number(val.span),
                self.get_debug_type(val.ty),
                true,
                0,
                WORD_SIZE_BITS as u32,
            )),
            None,
            self.builder.get_current_debug_location().unwrap(),
            store_instr,
        );
        let pointer = Pointer {
            pointer: variable_ptr,
            pointee_type_id: val.ty,
            pointee_llvm_type: variable_type.value_type(),
        };
        self.variables.insert(val.variable_id, pointer);
        variable_ptr
    }

    fn codegen_if_else(&mut self, ir_if: &TypedIf) -> BasicValueEnum<'ctx> {
        let typ = self.codegen_type(ir_if.ty);
        let start_block = self.builder.get_insert_block().unwrap();
        let current_fn = start_block.get_parent().unwrap();
        let consequent_block = self.ctx.append_basic_block(current_fn, "if_cons");
        let alternate_block = self.ctx.append_basic_block(current_fn, "if_alt");
        let merge_block = self.ctx.append_basic_block(current_fn, "if_merge");

        // Entry block
        let condition = self.codegen_expr_rvalue(&ir_if.condition);
        let condition_value = condition.into_int_value();
        self.builder.build_conditional_branch(condition_value, consequent_block, alternate_block);

        // Consequent Block
        // If any of these blocks have an early return, they'll return None, and we'll panic for
        // now
        self.builder.position_at_end(consequent_block);
        let (consequent_final_block, consequent_expr) =
            match self.codegen_block_statements(&ir_if.consequent) {
                None => {
                    // The consequent block does not return an expression, instead, it returns early
                    // Thus, a PHI makes no sense, and the type of this entire if 'expression' is
                    // Unit, and there's no need for a PHI.
                    (None, None)
                }
                Some(value) => {
                    let consequent_final_block = self.builder.get_insert_block().unwrap();
                    self.builder.build_unconditional_branch(merge_block);
                    (Some(consequent_final_block), Some(value))
                }
            };

        // Alternate Block
        self.builder.position_at_end(alternate_block);
        let (alternate_final_block, alternate_expr) =
            match self.codegen_block_statements(&ir_if.alternate) {
                None => (None, None),
                Some(value) => {
                    let alternate_final_block = self.builder.get_insert_block().unwrap();
                    self.builder.build_unconditional_branch(merge_block);
                    (Some(alternate_final_block), Some(value))
                }
            };

        // FIXME: What we are doing here is inferring whether this is an if expression or
        //       an if statement that can return early.
        //       If both sides result in an expression, we do the PHI thing
        //       If either side returns early, we can't, and we return unit
        //
        //       This should all be done and decided earlier during typechecking to make codegen
        //       less branchy and more mechanical.

        // Merge block
        self.builder.position_at_end(merge_block);
        match (consequent_expr, alternate_expr) {
            (Some(consequent_expr), Some(alternate_expr)) => {
                let phi_value = self.builder.build_phi(typ.value_type(), "if_phi");
                phi_value.add_incoming(&[
                    (&consequent_expr, consequent_final_block.unwrap()),
                    (&alternate_expr, alternate_final_block.unwrap()),
                ]);
                phi_value.as_basic_value().into()
            }
            _ => self.builtin_types.unit_value.as_basic_value_enum(),
        }
    }
    fn codegen_expr_rvalue(&mut self, expr: &TypedExpr) -> BasicValueEnum<'ctx> {
        self.codegen_expr(expr, false)
    }

    fn codegen_optional_value(
        &mut self,
        type_id: TypeId,
        optional_some: Option<&OptionalSome>,
    ) -> BasicValueEnum<'ctx> {
        let is_none = optional_some.is_none();
        let optional_llvm_type = self.codegen_type(type_id);
        let struct_type = optional_llvm_type.value_type().into_struct_type();
        let discriminator_value =
            if is_none { self.builtin_types.false_value } else { self.builtin_types.true_value };
        let value_value: BasicValueEnum<'ctx> = if let Some(opt_some) = optional_some {
            self.codegen_expr_rvalue(&opt_some.inner_expr)
        } else {
            struct_type.get_field_type_at_index(1).unwrap().const_zero()
        };
        let none_struct = self
            .builder
            .build_insert_value(
                struct_type.get_undef(),
                discriminator_value,
                0,
                &format!("opt_discrim_{}", type_id),
            )
            .unwrap();
        let struct_value = self
            .builder
            .build_insert_value(none_struct, value_value, 1, &format!("opt_value_{}", type_id))
            .unwrap();
        struct_value.as_basic_value_enum()
    }
    fn codegen_optional_has_value(
        &mut self,
        optional_value: StructValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let discriminator_value = self
            .builder
            .build_extract_value(optional_value, 0, "discrim_value")
            .unwrap()
            .into_int_value();
        let is_some = self.builder.build_int_compare(
            IntPredicate::NE,
            discriminator_value,
            self.builtin_types.false_value,
            "is_some",
        );
        is_some.as_basic_value_enum()
    }
    fn codegen_optional_get(&mut self, optional_value: StructValue<'ctx>) -> BasicValueEnum<'ctx> {
        // This is UNSAFE we don't check the discriminator. The actual unwrap method
        // or user-facing unwrap should check it and exit if it is None later
        self.builder
            .build_extract_value(optional_value, 1, "opt_value")
            .expect("optional should always have a 2nd field")
    }

    fn codegen_expr(&mut self, expr: &TypedExpr, is_lvalue: bool) -> BasicValueEnum<'ctx> {
        self.set_debug_location(expr.get_span());
        trace!("codegen expr\n{} (lvalue={is_lvalue})", self.module.expr_to_string(expr));
        match expr {
            TypedExpr::Variable(ir_var) => {
                if let Some(pointer) = self.variables.get(&ir_var.variable_id) {
                    trace!(
                        "codegen variable (lvalue) got pointee type {:?}",
                        pointer.pointee_llvm_type
                    );
                    if is_lvalue {
                        pointer.pointer.into()
                    } else {
                        let loaded = pointer.loaded_value(&self.builder);
                        trace!("codegen variable (rvalue) got loaded value {:?}", loaded);
                        loaded.into()
                    }
                } else if let Some(global) = self.globals.get(&ir_var.variable_id) {
                    let value = global.get_initializer().unwrap();
                    value.into()
                } else {
                    panic!(
                        "No pointer or global found for variable {}",
                        self.module.expr_to_string(expr)
                    )
                }
            }
            TypedExpr::None(type_id, _) => self.codegen_optional_value(*type_id, None),
            TypedExpr::OptionalSome(opt_some) => {
                self.codegen_optional_value(opt_some.type_id, Some(opt_some))
            }
            TypedExpr::OptionalHasValue(optional_expr) => {
                let optional_value = self.codegen_expr_rvalue(optional_expr);
                self.codegen_optional_has_value(optional_value.into_struct_value())
            }
            TypedExpr::OptionalGet(opt_get) => {
                let optional_value = self.codegen_expr_rvalue(&opt_get.inner_expr);
                self.codegen_optional_get(optional_value.into_struct_value())
            }
            TypedExpr::Unit(_) => self.builtin_types.unit_value.as_basic_value_enum(),
            TypedExpr::Char(byte, _) => {
                self.builtin_types.char.const_int(*byte as u64, false).as_basic_value_enum()
            }
            TypedExpr::Bool(b, _) => match b {
                true => self.builtin_types.true_value.as_basic_value_enum(),
                false => self.builtin_types.false_value.as_basic_value_enum(),
            },
            TypedExpr::Int(int_value, _) => {
                // LLVM only has unsigned values, the instructions are what provide the semantics
                // of signed vs unsigned
                let value = self.builtin_types.i64.const_int(*int_value as u64, false);
                value.as_basic_value_enum()
            }
            TypedExpr::Str(string_value, _) => {
                // We will make them records (structs) with an array pointer and a length for now
                let global_str_data = self.llvm_module.add_global(
                    self.builtin_types.char.array_type(string_value.len() as u32),
                    None,
                    "str_data",
                );
                global_str_data.set_initializer(&i8_array_from_str(self.ctx, string_value));
                global_str_data.set_constant(true);
                let global_value =
                    self.llvm_module.add_global(self.builtin_types.string_struct, None, "str");
                global_value.set_constant(true);
                let value = self.builtin_types.string_struct.const_named_struct(&[
                    self.builtin_types.i64.const_int(string_value.len() as u64, true).into(),
                    global_str_data.as_pointer_value().into(),
                ]);
                global_value.set_initializer(&value);
                let loaded = self.builder.build_load(
                    self.builtin_types.string_struct,
                    global_value.as_pointer_value(),
                    "str_struct",
                );
                loaded.into()
            }
            TypedExpr::Record(record) => {
                let record_llvm_type =
                    self.codegen_type(record.type_id).value_type().into_struct_type();
                let mut struct_value = record_llvm_type.get_undef();
                for (idx, field) in record.fields.iter().enumerate() {
                    let value = self.codegen_expr_rvalue(&field.expr);
                    struct_value = self
                        .builder
                        .build_insert_value(
                            struct_value,
                            value,
                            idx as u32,
                            &format!("record_init_{}", idx),
                        )
                        .unwrap()
                        .into_struct_value();
                }
                struct_value.as_basic_value_enum().into()
            }
            TypedExpr::RecordFieldAccess(field_access) => {
                if is_lvalue {
                    // Codegen a pointer to the field's storage location
                    let record_pointer =
                        self.codegen_expr(&field_access.base, false).into_pointer_value();
                    let record_type = self
                        .codegen_type(field_access.base.get_type())
                        .expect_pointer()
                        .pointee_type;
                    // let record_pointee = record_type.expect_pointer().pointee_type;
                    let field_ptr = self
                        .builder
                        .build_struct_gep(
                            record_type,
                            record_pointer,
                            field_access.target_field_index,
                            &format!(
                                "record.{}",
                                &*self.module.ast.get_ident_str(field_access.target_field)
                            ),
                        )
                        .unwrap();
                    field_ptr.as_basic_value_enum()
                } else {
                    // Codegen the field's loaded, dereferenced value
                    let record = self.codegen_expr_rvalue(&field_access.base);
                    let record_struct = record.into_struct_value();
                    let field_value = self
                        .builder
                        .build_extract_value(
                            record_struct,
                            field_access.target_field_index,
                            &format!(
                                "record.{}",
                                &*self.module.ast.get_ident_str(field_access.target_field)
                            ),
                        )
                        .unwrap();
                    field_value
                }
            }
            TypedExpr::Array(array) => {
                let Type::Array(array_type) = self.module.get_type(array.type_id) else {
                    panic!("expected array type for array");
                };
                let element_type = self.codegen_type(array_type.element_type).value_type();
                let array_len = self.builtin_types.i64.const_int(array.elements.len() as u64, true);

                let array_capacity = array_len.clone();
                let array_value = self.make_array(array_len, array_capacity, element_type, false);
                let array_data = self.builtin_types.array_data(&self.builder, array_value);
                // Store each element
                for (index, element_expr) in array.elements.iter().enumerate() {
                    let value = self.codegen_expr_rvalue(element_expr);
                    let index_value = self.ctx.i64_type().const_int(index as u64, true);
                    trace!("storing element {} of array literal: {:?}", index, element_expr);
                    let ptr_at_index = unsafe {
                        self.builder.build_gep(element_type, array_data, &[index_value], "elem")
                    };
                    self.builder.build_store(ptr_at_index, value);
                }
                let array_ptr = self.builder.build_alloca(array_value.get_type(), "array_ptr");
                self.builder.build_store(array_ptr, array_value);
                array_ptr.as_basic_value_enum()
            }
            TypedExpr::If(if_expr) => self.codegen_if_else(if_expr),
            TypedExpr::BinaryOp(bin_op) => match bin_op.ty {
                INT_TYPE_ID => {
                    let lhs_value = self.codegen_expr_rvalue(&bin_op.lhs).into_int_value();
                    let rhs_value = self.codegen_expr_rvalue(&bin_op.rhs).into_int_value();
                    let op_res = match bin_op.kind {
                        BinaryOpKind::Add => {
                            self.builder.build_int_add(lhs_value, rhs_value, "add")
                        }
                        BinaryOpKind::Subtract => {
                            self.builder.build_int_sub(lhs_value, rhs_value, "sub")
                        }
                        BinaryOpKind::Multiply => {
                            self.builder.build_int_mul(lhs_value, rhs_value, "mul")
                        }
                        BinaryOpKind::Divide => {
                            self.builder.build_int_signed_div(lhs_value, rhs_value, "sdiv")
                        }
                        BinaryOpKind::And => self.builder.build_and(lhs_value, rhs_value, "and"),
                        BinaryOpKind::Or => self.builder.build_or(lhs_value, rhs_value, "or"),
                        BinaryOpKind::Equals => self.builder.build_int_compare(
                            IntPredicate::EQ,
                            lhs_value,
                            rhs_value,
                            "eq",
                        ),
                        _ => {
                            panic!("Unsupported bin op kind returning int: {}", bin_op.kind)
                        }
                    };
                    op_res.as_basic_value_enum().into()
                }
                BOOL_TYPE_ID => match bin_op.kind {
                    BinaryOpKind::And | BinaryOpKind::Or => {
                        let lhs_int = self.codegen_expr_rvalue(&bin_op.lhs).into_int_value();
                        let rhs_int = self.codegen_expr_rvalue(&bin_op.rhs).into_int_value();
                        let op = match bin_op.kind {
                            BinaryOpKind::And => {
                                self.builder.build_and(lhs_int, rhs_int, "bool_and")
                            }
                            BinaryOpKind::Or => self.builder.build_or(lhs_int, rhs_int, "bool_or"),
                            BinaryOpKind::Equals => self.builder.build_int_compare(
                                IntPredicate::EQ,
                                lhs_int,
                                rhs_int,
                                "bool_eq",
                            ),
                            _ => panic!(),
                        };
                        op.as_basic_value_enum().into()
                    }
                    BinaryOpKind::Equals | BinaryOpKind::NotEquals => {
                        // I actually have no idea how I want to handle equality at this point
                        // Obviously we want some sort of value equality on user-defined types
                        // But here for builtin types maybe we just keep assuming everything
                        // is represented as an int value
                        let lhs_int = self.codegen_expr_rvalue(&bin_op.lhs).into_int_value();
                        let rhs_int = self.codegen_expr_rvalue(&bin_op.rhs).into_int_value();
                        self.builder
                            .build_int_compare(
                                if bin_op.kind == BinaryOpKind::Equals {
                                    IntPredicate::EQ
                                } else {
                                    IntPredicate::NE
                                },
                                lhs_int,
                                rhs_int,
                                &format!("{}", bin_op.kind),
                            )
                            .as_basic_value_enum()
                            .into()
                    }

                    BinaryOpKind::Less
                    | BinaryOpKind::LessEqual
                    | BinaryOpKind::Greater
                    | BinaryOpKind::GreaterEqual => {
                        let lhs_int = self.codegen_expr_rvalue(&bin_op.lhs).into_int_value();
                        let rhs_int = self.codegen_expr_rvalue(&bin_op.rhs).into_int_value();
                        let pred = match bin_op.kind {
                            BinaryOpKind::Equals => IntPredicate::EQ,
                            BinaryOpKind::Less => IntPredicate::SLT,
                            BinaryOpKind::LessEqual => IntPredicate::SLE,
                            BinaryOpKind::Greater => IntPredicate::SGT,
                            BinaryOpKind::GreaterEqual => IntPredicate::SGE,
                            _ => unreachable!("unexpected binop kind"),
                        };
                        self.builder
                            .build_int_compare(pred, lhs_int, rhs_int, &format!("{}", bin_op.kind))
                            .as_basic_value_enum()
                            .into()
                    }
                    other => panic!("Unsupported binary operation {other:?} returning Bool"),
                },
                STRING_TYPE_ID => panic!("No string-returning binary ops yet"),
                UNIT_TYPE_ID => panic!("No unit-returning binary ops"),
                CHAR_TYPE_ID => panic!("No char-returning binary ops"),
                other => todo!("codegen for binary ops on user-defined types: {other}"),
            },
            TypedExpr::UnaryOp(unary_op) => {
                let value = self.codegen_expr_rvalue(&unary_op.expr);
                match unary_op.kind {
                    UnaryOpKind::Dereference => {
                        let value_ptr = value.into_pointer_value();
                        let pointee_ty = self.codegen_type(unary_op.type_id);
                        let value =
                            self.builder.build_load(pointee_ty.value_type(), value_ptr, "deref");
                        value
                    }
                    UnaryOpKind::Reference => {
                        let value_location = self.builder.build_alloca(value.get_type(), "ref");
                        self.builder.build_store(value_location, value);
                        value_location.as_basic_value_enum()
                    }
                    UnaryOpKind::BooleanNegation => {
                        let int_value = value.into_int_value();
                        let int_value_negated = self.builder.build_not(int_value, "bool_not");
                        int_value_negated.as_basic_value_enum()
                    }
                }
            }
            TypedExpr::Block(block) => {
                // This is just a lexical scoping block, not a control-flow block, so doesn't need
                // to correspond to an LLVM basic block
                // We just need to codegen each statement and assign the return value to an alloca
                let block_value = self.codegen_block_statements(block).unwrap();
                block_value
            }
            TypedExpr::FunctionCall(call) => self.codegen_function_call(call),
            TypedExpr::ArrayIndex(index_op) => {
                let elem_ptr = self.codegen_array_index_operation(index_op);
                if is_lvalue {
                    elem_ptr.as_basic_value_enum()
                } else {
                    let elem_type = self.codegen_type(index_op.result_type);
                    let elem_value = self.builder.build_load(
                        elem_type.value_type(),
                        elem_ptr,
                        "array_index_rvalue",
                    );
                    elem_value
                }
            }
            TypedExpr::StringIndex(index_op) => {
                let elem_ptr = self.codegen_string_index_operation(index_op);
                if is_lvalue {
                    elem_ptr.as_basic_value_enum()
                } else {
                    let elem_type = self.codegen_type(index_op.result_type);
                    let elem_value = self.builder.build_load(
                        elem_type.value_type(),
                        elem_ptr,
                        "string_index_rvalue",
                    );
                    elem_value
                }
            }
            TypedExpr::For(typed_for_expr) => {
                eprintln!(
                    "codegen for expr: {}: {}",
                    self.module.expr_to_string(expr),
                    self.module.type_id_to_string(expr.get_type())
                );
                match typed_for_expr.for_expr_type {
                    ForExprType::Yield => unimplemented!("codegen for yield"),
                    ForExprType::Do => {
                        unimplemented!("codegen for do");
                    }
                }
            }
        }
    }
    fn codegen_function_call(&mut self, call: &Call) -> BasicValueEnum<'ctx> {
        let ir_module = self.module.clone();
        let callee = ir_module.get_function(call.callee_function_id);

        let function_value = self.codegen_function(call.callee_function_id, callee);

        let args: Vec<BasicMetadataValueEnum<'ctx>> = call
            .args
            .iter()
            .enumerate()
            .map(|(idx, arg_expr)| {
                let expected_type = callee.params[idx].type_id;
                let basic_value = self.codegen_expr_rvalue(arg_expr);
                trace!(
                    "codegen function call arg: {}: {} (expected: {}) \n {:?}",
                    self.module.expr_to_string(arg_expr),
                    self.module.type_id_to_string(arg_expr.get_type()),
                    self.module.type_id_to_string(expected_type),
                    basic_value
                );
                basic_value.into()
            })
            .collect();
        let callsite_value = self.builder.build_call(function_value, &args, "call_ret");
        // Call returns Right for void, and Left for values
        let result_value =
            callsite_value.try_as_basic_value().left().expect("function returned void");
        result_value
    }

    fn codegen_string_index_operation(&mut self, operation: &IndexOp) -> PointerValue<'ctx> {
        let string_value = self.codegen_expr_rvalue(&operation.base_expr);
        let index_value = self.codegen_expr_rvalue(&operation.index_expr);

        let string_value = string_value.as_basic_value_enum().into_struct_value();
        let data_ptr = self.builtin_types.string_data(&self.builder, string_value);

        let index_int_value = index_value.into_int_value();
        let character_pointer = unsafe {
            self.builder.build_gep(
                self.builtin_types.char,
                data_ptr,
                &[index_int_value],
                "string_index_ptr",
            )
        };
        character_pointer
    }

    fn codegen_array_index_operation(&mut self, operation: &IndexOp) -> PointerValue<'ctx> {
        let pointee_ty = self.codegen_type(operation.result_type);
        // TOOD: Once codegen_expr returns type info, we can ditch this extra work to get and generate types separately
        let array_type = self.builtin_types.array_struct(pointee_ty.value_type());
        let array_value = self.codegen_expr_rvalue(&operation.base_expr).into_pointer_value();
        let index_value = self.codegen_expr_rvalue(&operation.index_expr);

        let array_struct =
            self.builder.build_load(array_type, array_value, "array_value").into_struct_value();
        let index_int_value = index_value.into_int_value();
        // Likely we need one more level of dereferencing
        let array_data = self.builtin_types.array_data(&self.builder, array_struct);
        let gep_ptr = unsafe {
            self.builder.build_gep(
                pointee_ty.value_type(),
                array_data,
                &[index_int_value],
                "array_index_ptr",
            )
        };
        // We want to return a pointer to the element, not the element itself
        // This is usually so that we can assign to the element
        gep_ptr
    }

    fn make_string(&self, array_len: IntValue<'ctx>) -> StructValue<'ctx> {
        let data_ptr = self
            .builder
            .build_array_malloc(self.builtin_types.char, array_len, "string_data")
            .unwrap();
        let string_type = self.builtin_types.string_struct;
        let string_struct = self
            .builder
            .build_insert_value(
                string_type.get_undef(),
                array_len,
                STRING_LENGTH_FIELD_INDEX,
                "string_len",
            )
            .unwrap();
        let string_struct = self
            .builder
            .build_insert_value(string_struct, data_ptr, STRING_DATA_FIELD_INDEX, "string_data")
            .unwrap();
        // 63 = ascii '?' character to detect uninitialized memory
        self.builder
            .build_memset(data_ptr, 1, self.builtin_types.char.const_int(63, false), array_len)
            .unwrap();
        string_struct.as_basic_value_enum().into_struct_value()
    }

    #[allow(unused)]
    fn const_string(&self, string: &str) -> StructValue<'ctx> {
        let char_data = self.ctx.const_string(string.as_bytes(), false);
        let empty_str =
            self.make_string(self.builtin_types.i64.const_int(string.len() as u64, false));
        let string_data_ptr = self.builtin_types.string_data(&self.builder, empty_str);
        self.builder.build_store(string_data_ptr, char_data);
        empty_str
    }

    fn make_array(
        &mut self,
        array_len: IntValue<'ctx>,
        capacity: IntValue<'ctx>,
        element_type: BasicTypeEnum<'ctx>,
        zero_initialize: bool,
    ) -> StructValue<'ctx> {
        // First, we allocate memory somewhere not on the stack
        let data_ptr =
            self.builder.build_array_malloc(element_type, capacity, "array_data").unwrap();
        if zero_initialize {
            let size_bytes = self.builder.build_int_mul(
                capacity,
                element_type.size_of().unwrap(),
                "memset_bytes",
            );
            self.builder
                .build_memset(data_ptr, 1, self.builtin_types.char.const_zero(), size_bytes)
                .unwrap();
        }
        let array_type = self.builtin_types.array_struct(element_type);
        // insert_value returns a value and takes a value, doesn't modify memory at pointers
        // We can start building a struct by giving it an undefined struct first
        let array_struct = self
            .builder
            .build_insert_value(array_type.get_undef(), array_len, 0, "array_len")
            .unwrap();
        let array_struct =
            self.builder.build_insert_value(array_struct, capacity, 1, "array_cap").unwrap();
        let array_struct =
            self.builder.build_insert_value(array_struct, data_ptr, 2, "array_data").unwrap();
        array_struct.into_struct_value()
    }

    fn get_loaded_variable(&mut self, variable_id: VariableId) -> BasicValueEnum<'ctx> {
        self.variables.get(&variable_id).unwrap().loaded_value(&self.builder)
    }

    fn codegen_intrinsic(
        &mut self,
        intrinsic_type: IntrinsicFunctionType,
        function: &Function,
    ) -> BasicValueEnum<'ctx> {
        match intrinsic_type {
            IntrinsicFunctionType::Exit => {
                let first_arg = self.get_loaded_variable(function.params[0].variable_id);
                self.builder.build_call(self.libc_functions.exit, &[first_arg.into()], "exit");
                self.builtin_types.unit_value.as_basic_value_enum()
            }
            IntrinsicFunctionType::PrintInt => {
                let first_arg = self.get_loaded_variable(function.params[0].variable_id);
                self.build_print_int_call(first_arg);
                self.builtin_types.unit_value.as_basic_value_enum()
            }
            IntrinsicFunctionType::PrintString => {
                let string_arg =
                    self.get_loaded_variable(function.params[0].variable_id).into_struct_value();
                self.build_print_string_call(string_arg);
                self.builtin_types.unit_value.as_basic_value_enum()
            }
            IntrinsicFunctionType::StringLength => {
                let string_arg =
                    self.get_loaded_variable(function.params[0].variable_id).into_struct_value();
                let length = self.builtin_types.string_length(&self.builder, string_arg);
                length.as_basic_value_enum()
            }
            IntrinsicFunctionType::StringFromCharArray => {
                let array =
                    self.get_loaded_variable(function.params[0].variable_id).into_pointer_value();
                let array_type =
                    self.builtin_types.array_struct(self.builtin_types.char.as_basic_type_enum());
                let array_struct =
                    self.builder.build_load(array_type, array, "array_struct").into_struct_value();
                let array_len = self.builtin_types.array_length(&self.builder, array_struct);
                let string = self.make_string(array_len);
                let string_data = self.builtin_types.string_data(&self.builder, string);
                let array_data = self.builtin_types.array_data(&self.builder, array_struct);
                let _copied =
                    self.builder.build_memcpy(string_data, 1, array_data, 1, array_len).unwrap();
                string.as_basic_value_enum()
            }
            IntrinsicFunctionType::StringEquals => {
                let string1 =
                    self.get_loaded_variable(function.params[0].variable_id).into_struct_value();
                let string2 =
                    self.get_loaded_variable(function.params[1].variable_id).into_struct_value();
                // self.build_print_string_call(self.const_string("string_eq_fn\n"));
                let string1len = self.builtin_types.string_length(&self.builder, string1);
                let string2len = self.builtin_types.string_length(&self.builder, string2);
                let len_eq = self.builder.build_int_compare(
                    IntPredicate::EQ,
                    string1len,
                    string2len,
                    "len_eq",
                );
                let start_block = self.builder.get_insert_block().unwrap();
                let current_fn = start_block.get_parent().unwrap();
                let result = self.builder.build_alloca(self.builtin_types.boolean, "result");
                self.builder.build_store(result, self.builtin_types.false_value);

                let compare_block = self.ctx.append_basic_block(current_fn, "compare");
                let finish_block = self.ctx.append_basic_block(current_fn, "finish");

                // If lengths are equal, go to mem compare
                self.builder.build_conditional_branch(len_eq, compare_block, finish_block);

                self.builder.position_at_end(compare_block);
                let empty_strings_block =
                    self.ctx.append_basic_block(current_fn, "empty_strings_case");
                let memcmp_block = self.ctx.append_basic_block(current_fn, "memcmp_case");
                let len_is_zero = self.builder.build_int_compare(
                    IntPredicate::EQ,
                    string1len,
                    self.builtin_types.i64.const_zero(),
                    "len_is_zero",
                );
                self.builder.build_conditional_branch(
                    len_is_zero,
                    empty_strings_block,
                    memcmp_block,
                );

                self.builder.position_at_end(empty_strings_block);
                // self.build_print_string_call(self.const_string("empty strings case\n"));
                self.builder.build_store(result, self.builtin_types.true_value);
                self.builder.build_unconditional_branch(finish_block);

                self.builder.position_at_end(memcmp_block);
                // self.build_print_string_call(self.const_string("memcmp case\n"));
                let string1data = self.builtin_types.string_data(&self.builder, string1);
                let string2data = self.builtin_types.string_data(&self.builder, string2);
                // let memcmp_intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.memcmp").unwrap();
                // let memcmp_function = memcmp_intrinsic
                //     .get_declaration(
                //         &self.llvm_module,
                //         &[self
                //             .ctx
                //             .i8_type()
                //             .ptr_type(self.default_address_space)
                //             .as_basic_type_enum()],
                //     )
                //     .unwrap();
                let memcmp_result_i32 = self
                    .builder
                    .build_call(
                        self.libc_functions.memcmp,
                        &[string1data.into(), string2data.into(), string1len.into()],
                        "data_eq",
                    )
                    .try_as_basic_value()
                    .left()
                    .unwrap()
                    .into_int_value();
                let is_zero = self.builder.build_int_compare(
                    IntPredicate::EQ,
                    memcmp_result_i32,
                    self.ctx.i32_type().const_zero(),
                    "is_zero",
                );
                self.builder.build_store(result, is_zero);
                self.builder.build_unconditional_branch(finish_block);

                self.builder.position_at_end(finish_block);
                // self.build_print_string_call(self.const_string("finish_block"));
                self.builder
                    .build_load(self.builtin_types.boolean, result, "string_eq_result_value")
                    .as_basic_value_enum()
            }
            IntrinsicFunctionType::ArrayLength => {
                let array_arg =
                    self.get_loaded_variable(function.params[0].variable_id).into_pointer_value();
                let array_type: LlvmPointerType =
                    self.codegen_type(function.params[0].type_id).expect_pointer().clone();
                let array_struct = self
                    .builder
                    .build_load(
                        array_type.pointee_type.into_struct_type(),
                        array_arg,
                        "array_struct",
                    )
                    .into_struct_value();
                let length = self.builtin_types.array_length(&self.builder, array_struct).clone();
                length.as_basic_value_enum()
            }
            IntrinsicFunctionType::ArrayNew => {
                let array_type_id = function.ret_type;
                let array_type = self.module.get_type(array_type_id);
                let element_type = self.codegen_type(array_type.expect_array().element_type);
                let len = self.get_loaded_variable(function.params[0].variable_id).into_int_value();

                // TODO: Use ctlz intrinsic to count leading zeroes and get next highest
                //       power of 2 for capacity
                // let ctlz_intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.ctlz").unwrap();
                // let ctlz_function = ctlz_intrinsic.get_declaration(&self.llvm_module, &[self.ctx.i64_type().as_basic_type_enum()]).unwrap();
                // let capacity = self.builder.build_call(ctlz_function, &[], "capacity");
                let capacity = len.clone();
                let array_struct = self.make_array(len, capacity, element_type.value_type(), true);
                let array_ptr =
                    self.builder.build_malloc(array_struct.get_type(), "array_ptr").unwrap();
                self.builder.build_store(array_ptr, array_struct);
                array_ptr.as_basic_value_enum()
            }
            IntrinsicFunctionType::ArrayGrow => {
                // We need to resize the array and copy the elements into the new memory
                let self_param = &function.params[0];
                let array_ptr =
                    self.get_loaded_variable(self_param.variable_id).into_pointer_value();
                let array_type = self
                    .codegen_type(self_param.type_id)
                    .expect_pointer()
                    .pointee_type
                    .into_struct_type();
                let array =
                    self.builder.build_load(array_type, array_ptr, "array").into_struct_value();
                let cap = self.builtin_types.build_array_capacity(&self.builder, array);
                let cap_is_zero = self.builder.build_int_compare(
                    IntPredicate::EQ,
                    cap,
                    self.builtin_types.i64.const_zero(),
                    "cap_is_zero",
                );
                let cap_times_two = self.builder.build_int_mul(
                    cap,
                    self.builtin_types.i64.const_int(2, true),
                    "new_cap",
                );
                let new_cap = self
                    .builder
                    .build_select(
                        cap_is_zero,
                        self.builtin_types.i64.const_int(1, true),
                        cap_times_two,
                        "new_cap",
                    )
                    .into_int_value();
                // self.build_print_string_call(self.const_string("growing array\n"));
                let old_data = self.builtin_types.array_data(&self.builder, array);
                let element_type_id =
                    self.module.get_type(self_param.type_id).expect_array().element_type;
                let new_data_type = self.codegen_type(element_type_id);
                let new_data = self
                    .builder
                    .build_array_malloc(new_data_type.value_type(), new_cap, "new_data")
                    .unwrap();
                let memcpy_bytes = self.builder.build_int_mul(
                    cap,
                    new_data_type.value_type().size_of().unwrap(),
                    "memcpy_bytes",
                );
                let _copied =
                    self.builder.build_memcpy(new_data, 1, old_data, 1, memcpy_bytes).unwrap();
                let array = self
                    .builder
                    .build_insert_value(array, new_data, ARRAY_DATA_FIELD_INDEX, "new_array_data")
                    .unwrap();
                let array = self
                    .builder
                    .build_insert_value(array, new_cap, ARRAY_CAPACITY_FIELD_INDEX, "new_array_cap")
                    .unwrap();
                self.builder.build_store(array_ptr, array);
                self.builder.build_free(old_data);
                self.builtin_types.unit_value.as_basic_value_enum()
            }
            IntrinsicFunctionType::ArrayCapacity => {
                let array_arg =
                    self.get_loaded_variable(function.params[0].variable_id).into_pointer_value();
                let array_struct_type = self
                    .codegen_type(function.params[0].type_id)
                    .expect_pointer()
                    .pointee_type
                    .into_struct_type();
                let array_value = self
                    .builder
                    .build_load(array_struct_type, array_arg, "array_value")
                    .into_struct_value();
                let capacity_value = self
                    .builtin_types
                    .build_array_capacity(&self.builder, array_value)
                    .as_basic_value_enum();
                capacity_value
            }
            IntrinsicFunctionType::ArraySetLength => {
                let array_arg =
                    self.get_loaded_variable(function.params[0].variable_id).into_pointer_value();
                let array_struct_type = self
                    .codegen_type(function.params[0].type_id)
                    .expect_pointer()
                    .pointee_type
                    .into_struct_type();
                let array_value = self
                    .builder
                    .build_load(array_struct_type, array_arg, "array_value")
                    .into_struct_value();
                let new_len =
                    self.get_loaded_variable(function.params[1].variable_id).into_int_value();
                let updated_array = self
                    .builder
                    .build_insert_value(
                        array_value,
                        new_len,
                        ARRAY_LENGTH_FIELD_INDEX,
                        "set_length",
                    )
                    .unwrap();
                self.builder.build_store(array_arg, updated_array);
                self.builtin_types.unit_value.as_basic_value_enum()
            }
        }
    }
    // This needs to return either a basic value or an instruction value (in the case of early return)
    // Actually, early return is a big rabbit hole. We need to typecheck it earlier, and probably
    // store it on the block
    //
    // For now, I'm going to return an Option. If the block has an early return, we just return
    // None. We'll fix it when implementing early returns
    // Maybe we rename ReturnStmt to Early Return to separate it from tail returns, which have
    // pretty different semantics and implications for codegen, I am realizing
    fn codegen_block_statements(&mut self, block: &TypedBlock) -> Option<BasicValueEnum<'ctx>> {
        let mut last: Option<BasicValueEnum<'ctx>> = None;
        self.set_debug_location(block.span);
        for stmt in &block.statements {
            match stmt {
                TypedStmt::Expr(expr) => last = Some(self.codegen_expr_rvalue(expr)),
                TypedStmt::ValDef(val_def) => {
                    let _value = self.codegen_val(val_def);
                    last = Some(self.builtin_types.unit_value.as_basic_value_enum())
                }
                TypedStmt::Assignment(assignment) => {
                    match assignment.destination.deref() {
                        // ASSIGNMENT! We're in lvalue land. We need to get the pointer to the
                        // destination, and be sure to call the correct variant of codegen_expr
                        TypedExpr::Variable(v) => {
                            let destination_ptr =
                                *self.variables.get(&v.variable_id).expect("Missing variable");
                            let initializer = self.codegen_expr_rvalue(&assignment.value);
                            self.builder.build_store(destination_ptr.pointer, initializer);

                            //self.builder.build_store(des, value.loaded_value(&self.builder))
                            last = Some(self.builtin_types.unit_value.as_basic_value_enum())
                        }
                        TypedExpr::RecordFieldAccess(_field_access) => {
                            // We use codegen_expr with is_lvalue = true to get the pointer to the accessed field
                            let field_ptr = self
                                .codegen_expr(&assignment.destination, true)
                                .into_pointer_value();
                            let rhs = self.codegen_expr_rvalue(&assignment.value);
                            self.builder.build_store(field_ptr, rhs);
                            last = Some(self.builtin_types.unit_value.as_basic_value_enum())
                        }
                        TypedExpr::ArrayIndex(_array_index) => {
                            // We use codegen_expr with is_lvalue = true to get the pointer to the accessed element
                            let elem_ptr = self
                                .codegen_expr(&assignment.destination, true)
                                .into_pointer_value();
                            let rhs = self.codegen_expr_rvalue(&assignment.value);
                            self.builder.build_store(elem_ptr, rhs);
                            last = Some(self.builtin_types.unit_value.as_basic_value_enum())
                        }
                        _ => {
                            panic!("Invalid assignment lhs")
                        }
                    }
                }
                TypedStmt::WhileLoop(while_stmt) => {
                    let start_block = self.builder.get_insert_block().unwrap();
                    let current_fn = start_block.get_parent().unwrap();
                    let loop_entry_block = self.ctx.append_basic_block(current_fn, "while_cond");
                    let loop_body_block = self.ctx.append_basic_block(current_fn, "while_body");
                    let loop_end_block = self.ctx.append_basic_block(current_fn, "while_end");

                    // Go to the loop
                    self.builder.build_unconditional_branch(loop_entry_block);

                    self.builder.position_at_end(loop_entry_block);
                    let cond = self.codegen_expr_rvalue(&while_stmt.cond).into_int_value();

                    self.builder.build_conditional_branch(cond, loop_body_block, loop_end_block);

                    self.builder.position_at_end(loop_body_block);
                    self.codegen_block_statements(&while_stmt.block);
                    self.builder.build_unconditional_branch(loop_entry_block);

                    self.builder.position_at_end(loop_end_block);
                }
            }
        }
        last
    }

    fn get_function_debug_info(&mut self, function: &Function) -> DISubprogram<'ctx> {
        let return_type = self.get_debug_type(function.ret_type);
        let dbg_param_types = &function
            .params
            .iter()
            .map(|fn_param| self.get_debug_type(fn_param.type_id))
            .collect::<Vec<_>>();
        let dbg_fn_type = self.debug.debug_builder.create_subroutine_type(
            self.debug.file,
            Some(return_type),
            dbg_param_types,
            0,
        );
        self.debug.debug_builder.create_function(
            self.debug.current_scope(),
            &*self.module.ast.get_ident_str(function.name),
            None,
            self.debug.file,
            self.module.get_line_number(function.span),
            dbg_fn_type,
            false,
            true,
            self.module.get_line_number(function.span),
            0,
            false,
        )
    }

    fn codegen_function(
        &mut self,
        function_id: FunctionId,
        function: &Function,
    ) -> FunctionValue<'ctx> {
        trace!("codegen function\n{}", self.module.function_id_to_string(function_id, true));

        if let Some(function) = self.llvm_functions.get(&function_id) {
            return *function;
        }
        if function.is_generic() {
            panic!("Cannot codegen generic function: {}", &*self.get_ident_name(function.name));
        }

        let maybe_starting_block = self.builder.get_insert_block();
        let param_types: Vec<BasicMetadataTypeEnum> = function
            .params
            .iter()
            .map(|fn_arg| self.codegen_type(fn_arg.type_id).value_type().into())
            .collect();
        let ret_type: BasicMetadataTypeEnum<'ctx> =
            self.codegen_type(function.ret_type).value_type().into();
        let fn_ty = match ret_type {
            BasicMetadataTypeEnum::IntType(i) => i.fn_type(&param_types, false),
            BasicMetadataTypeEnum::PointerType(p) => p.fn_type(&param_types, false),
            BasicMetadataTypeEnum::StructType(s) => s.fn_type(&param_types, false),
            _ => panic!("Unexpected function llvm type"),
        };
        let llvm_linkage = match function.linkage {
            TyperLinkage::Standard => None,
            TyperLinkage::External => Some(LlvmLinkage::External),
            TyperLinkage::Intrinsic => None,
        };
        let fn_val = {
            let name = self.module.ast.get_ident_str(function.name);
            self.llvm_module.add_function(&name, fn_ty, llvm_linkage)
        };

        self.llvm_functions.insert(function_id, fn_val);

        if function.linkage == Linkage::External {
            return fn_val;
        }

        let di_subprogram = self.get_function_debug_info(function);
        self.debug.push_scope(di_subprogram.as_debug_info_scope());

        let entry_block = self.ctx.append_basic_block(fn_val, "entry");
        self.builder.position_at_end(entry_block);
        for (i, param) in fn_val.get_param_iter().enumerate() {
            let typed_param = &function.params[i];
            let ty = self.codegen_type(typed_param.type_id);
            let param_name = self.module.ast.get_ident_str(typed_param.name);
            trace!(
                "Got LLVM type for variable {}: {} (from {})",
                &*param_name,
                ty.value_type(),
                self.module.type_id_to_string(typed_param.type_id)
            );
            param.set_name(&param_name);
            self.set_debug_location(typed_param.span);
            let pointer = self.builder.build_alloca(ty.value_type(), &param_name);
            let arg_debug_type = self.get_debug_type(typed_param.type_id);
            let di_local_variable = self.debug.debug_builder.create_parameter_variable(
                self.debug.current_scope(),
                &param_name,
                typed_param.position,
                self.debug.file,
                self.module.get_line_number(function.span),
                arg_debug_type,
                true,
                0,
            );
            let store = self.builder.build_store(pointer, param);
            self.debug.debug_builder.insert_declare_before_instruction(
                pointer,
                Some(di_local_variable),
                None,
                self.set_debug_location(typed_param.span),
                store,
            );
            self.variables.insert(
                typed_param.variable_id,
                Pointer {
                    pointer,
                    pointee_type_id: typed_param.type_id,
                    pointee_llvm_type: ty.value_type(),
                },
            );
        }
        match function.intrinsic_type {
            Some(intrinsic_type) => {
                trace!("codegen intrinsic {:?} fn {:?}", intrinsic_type, function);
                let value = self.codegen_intrinsic(intrinsic_type, function).as_basic_value_enum();
                self.builder.build_return(Some(&value));
            }
            None => {
                // The plan is to separate the notion of "expression blocks" from "control flow
                // blocks" to make this all more reasonable
                // Rust rejects "unreachable expression"
                let value = self.codegen_block_statements(
                    function.block.as_ref().expect("functions must have blocks by codegen"),
                );
                let current_block = self.builder.get_insert_block().unwrap();
                if current_block.get_terminator().is_none() {
                    let return_value =
                        value.unwrap_or(self.builtin_types.unit_value.as_basic_value_enum());
                    self.builder.build_return(Some(&return_value));
                }
            }
        };
        if let Some(start_block) = maybe_starting_block {
            self.builder.position_at_end(start_block);
        }
        match function.intrinsic_type {
            // Workaround: ArrayNew returns a pointer to an alloca, so it needs to be inlined
            Some(IntrinsicFunctionType::ArrayNew) => {
                // Always inline
                fn_val.add_attribute(
                    AttributeLoc::Function,
                    self.ctx.create_string_attribute("alwaysinline", "true"),
                );
            }
            _ => {}
        }
        self.debug.pop_scope();
        fn_val.set_subprogram(di_subprogram);
        fn_val
    }

    pub fn codegen_module(&mut self) {
        for constant in &self.module.constants {
            match constant.expr {
                TypedExpr::Int(i64, _) => {
                    let llvm_ty = self.builtin_types.i64;
                    let llvm_val =
                        self.llvm_module.add_global(llvm_ty, Some(AddressSpace::default()), "");
                    llvm_val.set_constant(true);
                    llvm_val.set_initializer(&llvm_ty.const_int(i64 as u64, false));
                    self.globals.insert(constant.variable_id, llvm_val);
                }
                _ => todo!("constant must be int"),
            }
        }
        for (id, function) in self.module.clone().function_iter() {
            if !function.is_generic() {
                self.codegen_function(id, function);
            }
        }
    }

    pub fn name(&self) -> &str {
        self.module.name()
    }

    pub fn optimize(&mut self, optimize: bool) -> CodegenResult<()> {
        Target::initialize_aarch64(&InitializationConfig::default());
        // let triple = TargetMachine::get_default_triple();
        let triple = TargetTriple::create("arm64-apple-macosx14.0.0");
        let target = Target::from_triple(&triple).unwrap();
        if !self.debug.strip_debug {
            self.debug.debug_builder.finalize();
        } else {
            self.llvm_module.strip_debug_info();
        }
        self.llvm_module.verify().map_err(|err| {
            eprintln!("{}", self.llvm_module.to_string());
            anyhow::anyhow!("Module '{}' failed validation: {}", self.name(), err.to_string_lossy())
        })?;
        let machine = target
            .create_target_machine(
                &triple,
                "generic",
                "",
                OptimizationLevel::Aggressive,
                inkwell::targets::RelocMode::Default,
                inkwell::targets::CodeModel::Default,
            )
            .unwrap();

        self.llvm_module.set_data_layout(&machine.get_target_data().get_data_layout());
        self.llvm_module.set_triple(&triple);
        let pass_manager_b = PassManagerBuilder::create();
        pass_manager_b.set_optimization_level(OptimizationLevel::Aggressive);
        // pass_manager_b.populate_function_pass_manager(&function_pass_manager);
        // pass_manager_b.populate_module_pass_manager(&module_pass_manager);
        // The PassManager::create function works for Module and Function. If module,
        // the expected input is (). If function, the expected input is a Module
        // add_verifier_pass
        // let function_pass_manager: PassManager<FunctionValue<'ctx>> =
        //     PassManager::create(&self.llvm_module);
        // function_pass_manager.add_verifier_pass();

        let module_pass_manager: PassManager<LlvmModule<'ctx>> = PassManager::create(());
        if optimize {
            module_pass_manager.add_cfg_simplification_pass();
            module_pass_manager.add_promote_memory_to_register_pass();
            module_pass_manager.add_instruction_combining_pass();
        }

        // Workaround: We have to inline always because ArrayNew returns a pointer to an alloca
        module_pass_manager.add_function_inlining_pass();

        module_pass_manager.add_function_attrs_pass();
        module_pass_manager.add_verifier_pass();

        module_pass_manager.run_on(&self.llvm_module);

        machine.add_analysis_passes(&module_pass_manager);

        self.llvm_machine = Some(machine);

        Ok(())
    }

    pub fn emit_object_file(&self, rel_destination_dir: &str) -> Result<()> {
        let filename = format!("{}.o", self.name());
        let machine =
            self.llvm_machine.as_ref().expect("Cannot emit object file before optimizing");
        let path = Path::new(rel_destination_dir).join(Path::new(&filename));
        log::info!("Outputting object file to {}", path.to_str().unwrap());
        machine
            .write_to_file(&self.llvm_module, inkwell::targets::FileType::Object, &path)
            .unwrap();
        Ok(())
    }

    pub fn output_llvm_ir_text(&self) -> String {
        self.llvm_module.print_to_string().to_string()
    }

    pub fn interpret_module(&self) -> Result<u64> {
        let engine = self.llvm_module.create_jit_execution_engine(OptimizationLevel::None).unwrap();
        let return_value =
            unsafe { engine.run_function(self.llvm_module.get_last_function().unwrap(), &[]) };
        let res: u64 = return_value.as_int(true);
        Ok(res)
    }
}
