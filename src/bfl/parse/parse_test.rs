use crate::parse::*;
use std::fs;

fn make_test_module() -> ParsedModule {
    ParsedModule::make("unit_test".to_string())
}

fn set_up<'module>(input: &str, module: &'module mut ParsedModule) -> Parser<'static, 'module> {
    let source =
        Source::make(0, "unit_test".to_string(), "unit_test.bfl".to_string(), input.to_string());
    let mut lexer = Lexer::make(&source.content, &mut module.spans, 0);
    let token_vec: &'static mut [Token] = lexer.run().unwrap().leak();
    println!("{:#?}", token_vec);
    let parser = Parser::make(token_vec, source, module);
    parser
}

fn test_single_expr(input: &str) -> Result<(ParsedModule, ParsedExpression), ParseError> {
    test_single_expr_with_id(input).map(|(m, e, _)| (m, e))
}

fn test_single_type_expr(input: &str) -> Result<(ParsedModule, ParsedTypeExpression), ParseError> {
    let mut module = make_test_module();
    let mut parser = set_up(input, &mut module);
    let type_expr_id = parser.expect_type_expression()?;
    let expr = (*module.type_expressions.get(type_expr_id)).clone();
    Ok((module, expr))
}

fn test_single_expr_with_id(
    input: &str,
) -> Result<(ParsedModule, ParsedExpression, ParsedExpressionId), ParseError> {
    let mut module = make_test_module();
    let mut parser = set_up(input, &mut module);
    let expr_id = parser.expect_expression()?;
    let expr = (*module.expressions.get_expression(expr_id)).clone();
    Ok((module, expr, expr_id))
}

#[test]
fn basic_fn() -> Result<(), ParseError> {
    let src = r#"
    fn basic(x: int, y: int): int {
      println(42, 42, 42);
      val x: int = 0;
      mut y: int = 1;
      y = { 1; 2; 3 };
      y = add(42, 42);
      add(x, y)
    }"#;
    let source =
        Source::make(0, "test_src".to_string(), "test_case.bfl".to_string(), src.to_string());
    let mut module = test_parse_module(source)?;
    let fndef = module.functions.first().unwrap();
    assert_eq!(fndef.name, module.identifiers.intern("basic"));
    Ok(())
}

#[test]
fn string_literal() -> ParseResult<()> {
    let (module, result) = test_single_expr(r#""hello world""#)?;
    let ParsedExpression::Literal(Literal::String(s, span_id)) = result else { panic!() };
    let span = module.spans.get(span_id);
    // TODO: We need to store 2 spans, one for the whole literal construct including quotes and one for the string itself
    assert_eq!(&s, "hello world");
    assert_eq!(span.start, 1);
    assert_eq!(span.len, 11);
    assert_eq!(span.end(), 12);
    Ok(())
}

#[test]
fn infix() -> Result<(), ParseError> {
    let mut module = make_test_module();
    let mut parser = set_up("val x = a + b * doStuff(1, 2)", &mut module);
    let result = parser.parse_statement()?;
    if let Some(BlockStmt::ValDef(ValDef { value: expr_id, .. })) = result {
        let ParsedExpression::BinaryOp(op) = module.expressions.get_expression(expr_id) else {
            panic!()
        };
        assert_eq!(op.op_kind, BinaryOpKind::Add);
        assert!(matches!(
            *module.expressions.get_expression(op.lhs),
            ParsedExpression::Variable(_)
        ));
        if let ParsedExpression::BinaryOp(BinaryOp {
            op_kind: operation,
            lhs: operand1,
            rhs: operand2,
            ..
        }) = &*module.expressions.get_expression(op.rhs)
        {
            assert_eq!(*operation, BinaryOpKind::Multiply);
            assert!(matches!(
                *module.expressions.get_expression(*operand1),
                ParsedExpression::Variable(_)
            ));
            assert!(matches!(
                *module.expressions.get_expression(*operand2),
                ParsedExpression::FnCall(_)
            ));
        } else {
            panic!("Expected nested infix ops; got {:?}", result);
        }
    } else {
        panic!("Expected nested infix ops; got {:?}", result);
    }
    Ok(())
}

#[test]
fn struct_1() -> Result<(), ParseError> {
    let (_parser, result) = test_single_expr("{ a: 4, b: x[42], c: true }")?;
    assert!(matches!(result, ParsedExpression::Struct(_)));
    Ok(())
}

#[test]
fn parse_eof() -> Result<(), ParseError> {
    let mut module = make_test_module();
    let mut parser = set_up("", &mut module);
    let result = parser.parse_expression()?;
    assert!(result.is_none());
    Ok(())
}

#[test]
fn fn_args_literal() -> Result<(), ParseError> {
    let input = "f(myarg = 42,42,\"abc\")";
    let (module, result) = test_single_expr(input)?;
    if let ParsedExpression::FnCall(fn_call) = result {
        let args = &fn_call.args;
        let idents = &module.identifiers;
        assert_eq!(idents.get_name(fn_call.name), "f");
        assert_eq!(idents.get_name(args[0].name.unwrap()), "myarg");
        assert!(ParsedExpression::is_literal(&module.expressions.get_expression(args[0].value)));
        assert!(ParsedExpression::is_literal(&module.expressions.get_expression(args[1].value)));
        assert!(ParsedExpression::is_literal(&module.expressions.get_expression(args[2].value)));
        assert_eq!(args.len(), 3);
    } else {
        panic!("fail");
    }

    Ok(())
}

#[test]
fn if_no_else() -> ParseResult<()> {
    let input = "if x a";
    let mut module = make_test_module();
    let mut parser = set_up(input, &mut module);
    let _result = parser.parse_expression()?.unwrap();
    Ok(())
}

#[test]
fn dot_accessor() -> ParseResult<()> {
    let input = "a.b.c";
    let (module, result) = test_single_expr(input)?;
    let ParsedExpression::FieldAccess(access_op) = result else { panic!() };
    assert_eq!(module.identifiers.get_name(access_op.target), "c");
    let ParsedExpression::FieldAccess(acc2) = module.expressions.get_expression(access_op.base)
    else {
        panic!()
    };
    assert_eq!(module.identifiers.get_name(acc2.target), "b");
    let ParsedExpression::Variable(v) = module.expressions.get_expression(acc2.base) else {
        panic!()
    };
    assert_eq!(module.identifiers.get_name(v.name), "a");
    Ok(())
}

#[test]
fn type_parameter_single() -> ParseResult<()> {
    let (mut _module, type_expr) = test_single_type_expr("Array<int>")?;
    assert!(matches!(type_expr, ParsedTypeExpression::TypeApplication(_)));
    Ok(())
}

#[test]
fn type_parameter_multi() -> ParseResult<()> {
    let (module, type_expr) = test_single_type_expr("Map<int, Array<int>>")?;
    let ParsedTypeExpression::TypeApplication(app) = type_expr else {
        panic!("Expected type application")
    };
    assert_eq!(app.params.len(), 2);
    let ParsedTypeExpression::TypeApplication(inner_app) =
        module.type_expressions.get(app.params[1])
    else {
        panic!("Expected second param to be a type application");
    };
    assert!(matches!(
        module.type_expressions.get(inner_app.params[0]),
        ParsedTypeExpression::Int(_)
    ));
    Ok(())
}

#[test]
fn prelude_only() -> Result<(), ParseError> {
    env_logger::init();
    let prelude_source = Source::make(
        0,
        "builtins".to_string(),
        "prelude.bfl".to_string(),
        fs::read_to_string("builtins/prelude.bfl").unwrap(),
    );
    let module = test_parse_module(prelude_source)?;
    assert_eq!(&module.name, "prelude");
    assert_eq!(&module.sources.get_main().filename, "prelude.bfl");
    assert_eq!(&module.sources.get_main().directory, "builtins");
    assert!(!module.get_root_namespace().definitions.is_empty());
    Ok(())
}

#[test]
fn precedence() -> Result<(), ParseError> {
    let input = "2 * 1 + 3";
    let (module, result) = test_single_expr(input)?;
    let ParsedExpression::BinaryOp(bin_op) = result else { panic!() };
    let ParsedExpression::BinaryOp(lhs) = module.expressions.get_expression(bin_op.lhs) else {
        panic!()
    };
    let ParsedExpression::Literal(rhs) = module.expressions.get_expression(bin_op.rhs) else {
        panic!()
    };
    assert_eq!(bin_op.op_kind, BinaryOpKind::Add);
    assert_eq!(lhs.op_kind, BinaryOpKind::Multiply);
    assert!(matches!(rhs, Literal::Numeric(_, _)));
    Ok(())
}

#[test]
fn paren_expression() -> Result<(), ParseError> {
    let input = "(1 + 2[i][i + 4]) * 3";
    let (_parser, result) = test_single_expr(input)?;
    if let ParsedExpression::BinaryOp(bin_op) = &result {
        assert_eq!(bin_op.op_kind, BinaryOpKind::Multiply);
        return Ok(());
    }
    panic!()
}

#[test]
fn while_loop_1() -> Result<(), ParseError> {
    let input = "while true { (); (); 42 }";
    let mut module = make_test_module();
    let mut parser = set_up(input, &mut module);
    let result = parser.parse_statement()?.unwrap();
    if let BlockStmt::While(while_stmt) = result {
        assert_eq!(while_stmt.block.stmts.len(), 3);
        return Ok(());
    }
    panic!()
}

#[test]
fn cmp_operators() -> Result<(), ParseError> {
    let input = "a < b <= c > d >= e";
    let mut module = make_test_module();
    let mut parser = set_up(input, &mut module);
    let _result = parser.parse_expression()?.unwrap();
    Ok(())
}

#[test]
fn generic_fn_call() -> Result<(), ParseError> {
    let input = "square<int>(42)";
    let (_parser, result) = test_single_expr(input)?;
    assert!(matches!(result, ParsedExpression::FnCall(_)));
    Ok(())
}

#[test]
fn generic_method_call_lhs_expr() -> Result<(), ParseError> {
    let input = "getFn().baz<int>(42)";
    let (mut parser, result) = test_single_expr(input)?;
    let ParsedExpression::MethodCall(call) = result else { panic!() };
    let ParsedExpression::FnCall(fn_call) = parser.expressions.get_expression(call.base).clone()
    else {
        panic!()
    };
    assert_eq!(fn_call.name, parser.identifiers.intern("getFn"));
    assert_eq!(call.call.name, parser.identifiers.intern("baz"));
    let type_arg = parser.type_expressions.get(call.call.type_args.unwrap()[0].type_expr);
    assert!(type_arg.is_int());
    assert!(matches!(
        &*parser.expressions.get_expression(call.call.args[0].value),
        ParsedExpression::Literal(_)
    ));
    Ok(())
}

#[test]
fn char_type() -> ParseResult<()> {
    let (_module, result) = test_single_type_expr("char")?;
    assert!(matches!(result, ParsedTypeExpression::Char(_)));
    Ok(())
}

#[test]
fn char_value() -> ParseResult<()> {
    let input = "'x'";
    let (_parser, result) = test_single_expr(input)?;
    assert!(matches!(result, ParsedExpression::Literal(Literal::Char(b, _)) if b == b'x'));
    Ok(())
}

#[test]
fn namespaced_fncall() -> ParseResult<()> {
    let input = "foo::bar::baz()";
    let (mut parser, result) = test_single_expr(input)?;
    let ParsedExpression::FnCall(fn_call) = result else {
        dbg!(result);
        panic!("not fncall")
    };
    assert_eq!(fn_call.namespaces[0], parser.identifiers.intern("foo"));
    assert_eq!(fn_call.namespaces[1], parser.identifiers.intern("bar"));
    assert_eq!(fn_call.name, parser.identifiers.intern("baz"));
    assert!(fn_call.args.is_empty());
    Ok(())
}
#[test]
fn namespaced_val() -> ParseResult<()> {
    let input = "foo::bar::baz";
    let (mut parser, result) = test_single_expr(input)?;
    let ParsedExpression::Variable(variable) = result else {
        dbg!(result);
        panic!("not variable")
    };
    assert_eq!(variable.namespaces[0], parser.identifiers.intern("foo"));
    assert_eq!(variable.namespaces[1], parser.identifiers.intern("bar"));
    assert_eq!(variable.name, parser.identifiers.intern("baz"));
    Ok(())
}

#[test]
fn type_hint() -> ParseResult<()> {
    let input = "None: int?";
    let (module, _expr, expr_id) = test_single_expr_with_id(input)?;
    let type_hint = module.get_expression_type_hint(expr_id).unwrap();
    assert_eq!(module.type_expression_to_string(type_hint), "int?");
    Ok(())
}

#[test]
fn type_hint_binop() -> ParseResult<()> {
    let input = "(3!: int + 4: Array<bool>): int";
    let (module, _expr, expr_id) = test_single_expr_with_id(input)?;
    let type_hint = module.get_expression_type_hint(expr_id).unwrap();
    assert_eq!(module.type_expression_to_string(type_hint), "int");
    Ok(())
}

#[test]
fn tag_literals() -> ParseResult<()> {
    let input = ".Foo: .Foo";
    let (module, _expr, expr_id) = test_single_expr_with_id(input)?;
    let type_hint = module.get_expression_type_hint(expr_id).unwrap();
    assert_eq!(&module.expression_to_string(expr_id), ".Foo");
    assert_eq!(module.type_expression_to_string(type_hint), ".Foo");
    Ok(())
}

#[test]
fn when_pattern() -> ParseResult<()> {
    let input = r#"
        when x {
           | { x: 1, y: 2 } -> 1
           | { x: 2, y: 2 } -> { 2 }
           | _ -> { 3 }
        };
"#;
    let (module, _expr, expr_id) = test_single_expr_with_id(input)?;
    println!("{}", &module.expression_to_string(expr_id));
    Ok(())
}
