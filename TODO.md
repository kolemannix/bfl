# Up next maybe

- [x] Records
    - [x] Syntax decision ({} vs .{} vs Point {})
    - [x] Parsing
    - [x] Typechecking
    - [x] codegen static as LLVM structs
    - [x] Accessor syntax
    - [x] Accessor ir
    - [x] Accessor codegen
- [x] Identifier cleanup and interning
- [x] Actual scoping
- [x] Real prelude so we can more easily add runtime/stl functions (for array)
- [x] Heap memory (just using malloc)
- [x] Arrays (Fixed size but heap allocated)
- [x] Generic functions (no inference)
- [x] Remove / fix TokenIter type
- [x] parens around expressions
- [x] Binary infix operations
- [x] basic error reporting using spans
- [x] Array and Struct member assignment
- [x] While Loop
- [x] Assert (syscall exit? 'panic'?)
- [x] Replace .length on string and array with a MethodCall node (adding empty parens ())
- [x] Change Array repr to a struct w/ length so we have length
- [x] Strings
    - [x] String literals cant have spaces lol
    - [x] print(string)
    - [x] hardcoded via codegen
    - [x] string.length
    - [x] string[i] (int)
    - [x] char
    - [x] string[i] (char)
    - [x] add string.length function
    - [x] char.to_string()
- [x] Concatenate strings (in userland; slow)
- [x] Infer val types
- [x] Extern keyword, then
- [x] Link at build time not via LLVM module
- [x] Use ctx.const_string for printf format strings
- [x] Optionals
- [x] Improve printing output by fixing display functions
- [x] Implement builtin growable array
- [x] Use aggregates in codegen, not pointers, for optionals
- [x] Basic generic type inference in function calls
- [ ] Precedence of dereference (and i guess unary ops in general) should be higher
- [ ] Change our builtin int type to be 32 bits?
- [ ] Make codegen fail instead of panic
- [ ] Codegen 'variables' is broken (never cleared); use enter scope / exit scope w/ a stack of vecs or something
  instead (matters when a variable id is reused because the same function is called twice; I think we just overwrite it
  with the correct one but maybe nesting is a problem?)
- [ ] Support multiple files (module ID in all IDs)
- [ ] Builtin maps
- [x] Fix namespaces to require full paths (currently busted must be unique names globally)
- [x] Allow namespaces inside namespaces
- [ ] Type Ascriptions (literally can just pass expected_type into eval and do an addtl typecheck)
    - Any expression can be followed by `: <type expr>` to set the type hint / expected type of the expression
- [ ] Some debug info in LLVM IR (source snippets or line numbers?)
- [ ] Single-arm match syntax with `is`; `if x is Some(y)` instead of `if let Some(y) = x`; if x is not Err(y) (replaces
  let-else)
- [ ] Sum types (oneofs) (variants?) (tagged unions)
- [ ] Errors
- [ ] uint type
- [x] Generic type inference
- [ ] Type literals would be fun
- [ ] Syntax Shed
    - [ ] Function syntax change? (foo = fn (a, b, c): int { }
    - [ ] Ditch semicolons? (this technically introduces significant whitespace; need newline token)
    - [ ] from qualifier-focused to name -focused
    - [ ] Parse trailing commas
- [ ] Replace spans on typed ast with just ast node ids
- [ ] Tuples
- [ ] Pure lambdas with ->
- [ ] Full-on closures with =>

- [-] For iteration (we will just hardcode the iterable types)
    - [x] 'do' version
    - [x] 'yield' version on array
    - [x] 'yield' version on string
    - [x] Provide index var (it_index)
    - [ ] Allow customizing index var name

- [x] DEBUG info
    - [x] on or off
    - [x] flag types
    - [x] correct line nums
    - [ ] correct spans (depends on multifile)
    - [x] correct file path
    - [ ] Add lexical scopes for if and while

# Optionals

- [x] Optional types
- [x] None types
- [x] None value
- [x] Runtime repr for boolean
- [x] Runtime repr for unit
- [x] Runtime repr for int
- [x] Runtime repr for char
- [x] Runtime repr for string
- [x] record
- [x] array of optional records

# Optionals Round 2

- [x] move some-wrapping into a function
- [x] has value from userland (currently only happens by desugaring))
- [x] unwrap from userland (I think this can just be a prelude function not intrinsic)
- [x] test with optional array elements
- [x] test with optional record fields
- [x] test with optional function args
- [x] Explicit Some() expression

# Dogfood wishlist Oct23

- [x] I really need a way to write code in Zig or C and use it in my stl to move things along
- [x] Zig binding of my Array
- [ ] Early return
- [x] Implicit return of unit if no return statement
- [x] string.indexOf
    - charToString implemented in zig and linked
- [ ] array bounds checking
- [ ] Array.distinct in zig
- [x] Not equal != operator
- [x] Unary negation operator

# Fibonacci todos (Aug 20)

- [x] Equality binop
- [x] Precedence of binops; parens?
- [x] Recursion fix: process module decls first, then impls
- [x] Negative integer literals
- [x] PHI nested branch fix

# Hacks to fix

- [x] Fix line comments
- [x] Maybe eventually actually free some memory? lol
- [x] Rename IR to typed-ast, since it's a tree not instruction set. TAST?
- [x] Make intrinsics like arrayIndex a real function in the LLVM IR?
- [x] Fix line numbers to account for prelude
- [ ] main entrypoint properly. Typing of main? How to get main args?
- [ ] Replace vectors with smallvec where appropriate
- [x] Fix unnecessary load of function args
- [x] Proper println implementation. Fine to use printf internally for now but we should define our own func around it
- [x] Implement Display instead of relying on Debug

Design for

- optionality
- fallibility

# Dev ex

- [ ] Pretty-print AST
- [x] Error Spans
- [x] fancy output COLORS

# Maybe later

- [ ] Block-as-single-arg syntax (xs.map { y => } )
- [x] Type Params
- [ ] Unicode source
- [ ] multiple parameter lists
- [ ] Symbols like lisp or Clojure keywords would be really great; or just have string literal types?
- [ ] Symbols are always interned and very fast. Also signal intent to use a symbol vs use strings

# Resurrect project (March '23)

- [x] Implement typed IR
- [x] Implement FnCall
- [x] Implement Add
- [x] Implement Multiply
- [x] Use basicvalue not pointervalue as main IR type
- [x] Add booleans
- [x] Add boolean AND and OR
- [x] Parse line comments
- [x] implement "expected output" for test sources
- [x] Add spans to AST
- [x] Add spans to IR
- [x] Implement IF expressions

