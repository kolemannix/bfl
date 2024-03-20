use crate::parse::*;

#[cfg(test)]
fn set_up(input: &str) -> Parser<'static> {
    let mut lexer = Lexer::make(input);
    let token_vec: &'static mut [Token] = lexer.run().unwrap().leak();
    print_tokens(input, token_vec);
    let parser = Parser::make(token_vec, input.to_string(), ".".to_string(), input.to_string());
    parser
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
    let module = parse_text(src.to_string(), ".".to_string(), "basic_fn.nx".to_string(), false)?;
    if let Some(Definition::FnDef(fndef)) = module.defs.first() {
        assert_eq!(*module.get_ident_str(fndef.name), *"basic")
    } else {
        println!("defs {:?}", module.defs);
        panic!("no definitions for basic_fn")
    }
    Ok(())
}

#[test]
fn string_literal() -> ParseResult<()> {
    let mut parser = set_up(r#""hello world""#);
    let result = parser.expect_expression()?;
    let ExpressionValue::Literal(Literal::String(s, span)) = result.expr else { panic!() };
    assert_eq!(&s, "hello world");
    assert_eq!(span.start, 1);
    assert_eq!(span.end, 12);
    Ok(())
}

#[test]
fn infix() -> Result<(), ParseError> {
    let mut parser = set_up("val x = a + b * doStuff(1, 2)");
    let result = parser.parse_statement()?;
    if let Some(BlockStmt::ValDef(ValDef {
        value: Expression { expr: ExpressionValue::BinaryOp(op), .. },
        ..
    })) = &result
    {
        assert_eq!(op.op_kind, BinaryOpKind::Add);
        assert!(matches!(op.lhs.expr, ExpressionValue::Variable(_)));
        if let ExpressionValue::BinaryOp(BinaryOp {
            op_kind: operation,
            lhs: operand1,
            rhs: operand2,
            ..
        }) = &op.rhs.expr
        {
            assert_eq!(*operation, BinaryOpKind::Multiply);
            assert!(matches!(operand1.expr, ExpressionValue::Variable(_)));
            assert!(matches!(operand2.expr, ExpressionValue::FnCall(_)));
        } else {
            panic!("Expected nested infix ops; got {:?}", result);
        }
    } else {
        panic!("Expected nested infix ops; got {:?}", result);
    }
    Ok(())
}

#[test]
fn record() -> Result<(), ParseError> {
    let mut parser = set_up("{ a: 4, b: x[42], c: true }");
    let result = parser.parse_expression()?.unwrap();
    assert!(matches!(result.expr, ExpressionValue::Record(_)));
    Ok(())
}

#[test]
fn parse_eof() -> Result<(), ParseError> {
    let mut parser = set_up("");
    let result = parser.parse_expression()?;
    assert!(result.is_none());
    Ok(())
}

#[test]
fn fn_args_literal() -> Result<(), ParseError> {
    let input = "f(myarg = 42,42,\"abc\")";
    let mut parser = set_up(input);
    let result = parser.parse_expression()?.unwrap();
    if let ExpressionValue::FnCall(fn_call) = result.expr {
        let args = &fn_call.args;
        let idents = parser.identifiers.clone();
        assert_eq!(idents.borrow().get_name(fn_call.name), "f");
        assert_eq!(idents.borrow().get_name(args[0].name.unwrap()), "myarg");
        assert!(ExpressionValue::is_literal(&args[0].value.expr));
        assert!(ExpressionValue::is_literal(&args[1].value.expr));
        assert!(ExpressionValue::is_literal(&args[2].value.expr));
        assert_eq!(args.len(), 3);
    } else {
        panic!("fail");
    }

    Ok(())
}

#[test]
fn if_no_else() -> ParseResult<()> {
    let input = "if x a";
    let mut parser = set_up(input);
    let result = parser.parse_expression()?.unwrap();
    println!("{result:?}");
    Ok(())
}

#[test]
fn dot_accessor() -> ParseResult<()> {
    let input = "a.b.c";
    let mut parser = set_up(input);
    let result = parser.parse_expression()?.unwrap();
    let ExpressionValue::FieldAccess(acc) = result.expr else { panic!() };
    assert_eq!(acc.target.0.to_usize(), 2);
    let ExpressionValue::FieldAccess(acc2) = acc.base.expr else { panic!() };
    assert_eq!(acc2.target.0.to_usize(), 1);
    let ExpressionValue::Variable(v) = acc2.base.expr else { panic!() };
    assert_eq!(v.name.0.to_usize(), 0);
    Ok(())
}

#[test]
fn type_parameter_single() -> ParseResult<()> {
    let input = "Array<int>";
    let mut parser = set_up(input);
    let result = parser.parse_type_expression();
    assert!(matches!(result, Ok(Some(ParsedTypeExpression::TypeApplication(_)))));
    Ok(())
}

#[test]
fn type_parameter_multi() -> ParseResult<()> {
    let input = "Map<int, Array<int>>";
    let mut parser = set_up(input);
    let result = parser.parse_type_expression();
    let Ok(Some(ParsedTypeExpression::TypeApplication(app))) = result else {
        panic!("Expected type application")
    };
    assert_eq!(app.params.len(), 2);
    let ParsedTypeExpression::TypeApplication(inner_app) = &app.params[1] else {
        panic!("Expected second param to be a type application");
    };
    assert!(matches!(inner_app.params[0], ParsedTypeExpression::Int(_)));
    Ok(())
}

#[test]
fn prelude_only() -> Result<(), ParseError> {
    env_logger::init();
    let module = parse_text("".to_string(), ".".to_string(), "prelude_only.nx".to_string(), true)?;
    assert_eq!(&module.name, "prelude_only");
    assert_eq!(&module.source.filename, "prelude_only.nx");
    assert_eq!(&module.source.directory, ".");
    if let Some(Definition::FnDef(fndef)) = module.defs.first() {
        assert_eq!(*module.get_ident_str(fndef.name), *"_bfl_charToString")
    } else {
        println!("{module:?}");
        panic!("no definitions in prelude");
    }
    Ok(())
}

#[test]
fn precedence() -> Result<(), ParseError> {
    let input = "2 * 1 + 3";
    let mut parser = set_up(input);
    let result = parser.parse_expression()?.unwrap();
    println!("{result}");
    let ExpressionValue::BinaryOp(bin_op) = result.expr else { panic!() };
    let ExpressionValue::BinaryOp(lhs) = bin_op.lhs.expr else { panic!() };
    let ExpressionValue::Literal(rhs) = bin_op.rhs.expr else { panic!() };
    assert_eq!(bin_op.op_kind, BinaryOpKind::Add);
    assert_eq!(lhs.op_kind, BinaryOpKind::Multiply);
    assert!(matches!(rhs, Literal::Numeric(_, _)));
    Ok(())
}

#[test]
fn paren_expression() -> Result<(), ParseError> {
    let input = "(1 + 2[i][i + 4]) * 3";
    let mut parser = set_up(input);
    let result = parser.parse_expression()?.unwrap();
    println!("{}", result);
    if let ExpressionValue::BinaryOp(bin_op) = result.expr {
        assert_eq!(bin_op.op_kind, BinaryOpKind::Multiply);
        return Ok(());
    }
    panic!()
}

#[test]
fn while_loop_1() -> Result<(), ParseError> {
    let input = "while true { (); (); 42 }";
    let mut parser = set_up(input);
    let result = parser.parse_statement()?.unwrap();
    println!("{:?}", result);
    if let BlockStmt::While(while_stmt) = result {
        assert_eq!(while_stmt.block.stmts.len(), 3);
        return Ok(());
    }
    panic!()
}

#[test]
fn cmp_operators() -> Result<(), ParseError> {
    let input = "a < b <= c > d >= e";
    let mut parser = set_up(input);
    let result = parser.parse_expression()?.unwrap();
    println!("{:?}", result);
    Ok(())
}

#[test]
fn generic_fn_call() -> Result<(), ParseError> {
    let input = "square<int>(42)";
    let mut parser = set_up(input);
    let result = parser.parse_expression()?.unwrap();
    assert!(matches!(result.expr, ExpressionValue::FnCall(_)));
    Ok(())
}

#[test]
fn generic_method_call_lhs_expr() -> Result<(), ParseError> {
    let input = "getFn().baz<int>(42)";
    let mut parser = set_up(input);
    let result = parser.parse_expression()?.unwrap();
    let ExpressionValue::MethodCall(call) = result.expr else { panic!() };
    let ExpressionValue::FnCall(fn_call) = call.base.expr else { panic!() };
    assert_eq!(fn_call.name, parser.ident_id("getFn"));
    assert_eq!(call.call.name, parser.ident_id("baz"));
    assert!(call.call.type_args.unwrap()[0].type_expr.is_int());
    assert!(matches!(call.call.args[0].value.expr, ExpressionValue::Literal(_)));
    Ok(())
}

#[test]
fn char_type() -> ParseResult<()> {
    let input = "char";
    let mut parser = set_up(input);
    let result = parser.parse_type_expression()?.unwrap();
    assert!(matches!(result, ParsedTypeExpression::Char(_)));
    Ok(())
}

#[test]
fn char_value() -> ParseResult<()> {
    let input = "'x'";
    let mut parser = set_up(input);
    let result = parser.expect_expression()?;
    let x_byte = b'x';
    assert!(matches!(result.expr, ExpressionValue::Literal(Literal::Char(b, _)) if b == x_byte));
    Ok(())
}

#[test]
fn namespaced_fncall() -> ParseResult<()> {
    let input = "foo::bar::baz()";
    let mut parser = set_up(input);
    let result = parser.expect_expression()?;
    let ExpressionValue::FnCall(fn_call) = result.expr else {
        dbg!(result);
        panic!("not fncall")
    };
    assert_eq!(fn_call.namespaces[0], parser.ident_id("foo"));
    assert_eq!(fn_call.namespaces[1], parser.ident_id("bar"));
    assert_eq!(fn_call.name, parser.ident_id("baz"));
    assert!(fn_call.args.is_empty());
    Ok(())
}
#[test]
fn namespaced_val() -> ParseResult<()> {
    let input = "foo::bar::baz";
    let mut parser = set_up(input);
    let result = parser.expect_expression()?;
    let ExpressionValue::Variable(variable) = result.expr else {
        dbg!(result);
        panic!("not variable")
    };
    assert_eq!(variable.namespaces[0], parser.ident_id("foo"));
    assert_eq!(variable.namespaces[1], parser.ident_id("bar"));
    assert_eq!(variable.name, parser.ident_id("baz"));
    Ok(())
}

#[test]
fn type_hint() -> ParseResult<()> {
    let input = "None: int?";
    let mut parser = set_up(input);
    let result = parser.expect_expression()?;
    let type_hint = result.type_hint.unwrap();
    let ParsedTypeExpression::Optional(ParsedOptional { base, .. }) = type_hint else { panic!() };
    let ParsedTypeExpression::Int(_) = *base else {
        panic!();
    };
    Ok(())
}

#[test]
fn type_hint_binop() -> ParseResult<()> {
    let input = "(3!: int + 4: Array<bool>): int";
    let mut parser = set_up(input);
    let result = parser.expect_expression()?;
    let type_hint = &result.type_hint.as_ref().unwrap();
    eprintln!("{}", result);

    Ok(())
}
