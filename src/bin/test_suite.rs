use std::path::Path;

use anyhow::{bail, Result};
use bfl::compiler;
use inkwell::context::Context;
use std::os::unix::prelude::ExitStatusExt;

#[derive(Debug)]
enum TestExpectation {
    ExitCode(i32),
    CompileErrorMessage { message: String },
    CompileErrorLine { line: u32 },
}

fn get_test_expectation(test_file: &Path) -> TestExpectation {
    let path = test_file.canonicalize().unwrap();
    let src = std::fs::read_to_string(path).expect("could not read source file for test {}");

    let last_line = src.lines().rev().find(|l| !l.is_empty()).expect("last line");
    // We want expected output but we can't intercept or read what goes to stdout, so we just make
    // it expected return value for now
    let error_message_prefix = "//errmsg: ";
    let exit_code_prefix = "//exitcode: ";
    if last_line.starts_with(error_message_prefix) {
        let expected_error: String = last_line.chars().skip(error_message_prefix.len()).collect();
        TestExpectation::CompileErrorMessage { message: expected_error }
    } else if last_line.starts_with(exit_code_prefix) {
        let s: String = last_line.chars().skip(exit_code_prefix.len()).collect();
        let as_i32: i32 = s.parse().unwrap();
        TestExpectation::ExitCode(as_i32)
    } else {
        TestExpectation::ExitCode(0)
    }
}

fn test_file<P: AsRef<Path>>(ctx: &Context, path: P) -> Result<()> {
    let out_dir = "bfl-out/test_suite";
    let filename = path.as_ref().file_name().unwrap().to_str().unwrap();
    let args = bfl::compiler::Args {
        no_llvm_opt: true,
        debug: true,
        no_prelude: false,
        write_llvm: true,
        dump_module: false,
        run: false,
        file: path.as_ref().to_owned(),
        gui: false,
    };
    let compile_result = compiler::compile_module(&args);
    let expectation = get_test_expectation(path.as_ref());
    match compile_result {
        Err(err) => match err.module.as_ref() {
            Some(module) => {
                let err = &module.errors[0];
                match expectation {
                    TestExpectation::CompileErrorMessage { message } => {
                        // Check for message!
                        if !err.to_string().contains(&message) {
                            bail!(
                                "{}: Failed with unexpected message: {}",
                                filename,
                                err.to_string()
                            )
                        }
                    }
                    TestExpectation::CompileErrorLine { .. } => {
                        unimplemented!("error line test")
                    }
                    TestExpectation::ExitCode(expected_code) => bail!(
                        "{}: Expected exit code {} but got compile error",
                        filename,
                        expected_code,
                    ),
                }
            }
            None => {
                bail!("{} Failed during parsing, probably", filename)
            }
        },
        Ok(typed_module) => {
            let name = typed_module.name();
            if let TestExpectation::ExitCode(code) = expectation {
                let _codegen = compiler::codegen_module(&args, ctx, &typed_module, out_dir)?;

                let mut run_cmd = std::process::Command::new(format!("{}/{}.out", out_dir, name));
                let run_status = run_cmd.status().unwrap();
                if let Some(signal) = run_status.signal() {
                    if signal == 5 {
                        bail!("TEST CASE {} TERMINATED BY TRAP SIGNAL: {}", name, signal);
                    } else {
                        bail!("TEST CASE {} TERMINATED BY SIGNAL: {}", name, signal);
                    }
                };
                if run_status.code() != Some(code) {
                    bail!(
                        "TEST CASE {} FAILED WRONG EXIT CODE: exp {}, actual {}",
                        name,
                        code,
                        run_status.code().unwrap(),
                    );
                }
            } else {
                bail!("Expected failed compilation but actually succeeded")
            }
        }
    };
    Ok(())
}

pub fn main() -> Result<()> {
    let ctx = Context::create();
    let test_dir = "test_src";
    let mut failures: Vec<String> = Vec::new();
    let mut all_tests = Vec::new();
    for dir_entry in std::fs::read_dir(test_dir)? {
        let dir_entry = dir_entry?;
        let metadata = dir_entry.metadata()?;
        let path = dir_entry.path();
        if metadata.is_file() {
            let extension = path.extension().unwrap();
            if extension == "bfl" {
                all_tests.push(path.to_path_buf())
            }
        }
    }
    let mut total = 0;
    let mut success = 0;
    for test in all_tests.iter() {
        let result = test_file(&ctx, test.as_path());
        if result.is_ok() {
            success += 1;
        } else {
            failures.push(test.as_path().file_name().unwrap().to_str().unwrap().to_string());
            eprintln!("Test failed: {}", result.unwrap_err());
        }
        total += 1;
    }
    if success != total {
        eprintln!("Failed tests:\n{}", failures.join("\n"));
        bail!("{} tests failed", total - success);
    } else {
        eprintln!("Ran {} tests, {} succeeded", total, success);
    }
    Ok(())
}