use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};

use bfl::compiler::Args;
use bfl::typer::TypedModule;
use bfl::{compiler, gui};
use clap::Parser;
use log::info;

fn main() {
    env_logger::init();
    let args = Args::parse();
    info!("{:#?}", args);

    info!("bfl Compiler v0.1.0");

    let out_dir = "bfl-out";

    // If gui mode:
    // - Create a new thread to compile the module
    // - Run gui loop from this thread
    // - On frame or channel push, get module snapshot and render it
    // - Put module inside a RwLock, just try to read it from the gui thread

    if !args.gui {
        let Ok(module) = compiler::compile_module(&args) else {
            std::process::exit(1);
        };
        let module_name = module.name();
        info!("done waiting on compile thread");
        let llvm_ctx = inkwell::context::Context::create();
        let _codegen = match compiler::codegen_module(&args, &llvm_ctx, &module, out_dir, true) {
            Ok(codegen) => codegen,
            Err(_err) => {
                std::process::exit(1);
            }
        };
        compiler::run_compiled_program(out_dir, module_name);
        std::process::exit(0);
    }

    let module_handle: Arc<RwLock<Option<TypedModule>>> = Arc::new(RwLock::new(None));

    // Some thoughts: instead of a RwLock, we could just use a channel to send the module to the gui thread
    // Or we could just spawn a thread whenever we need to compile a module, and then send the module back to the main thread
    // Lots of better ways to share the data but this is working for now
    let shared_module_clone = module_handle.clone();
    let args_clone = args.clone();
    let (compile_sender, compile_receiver) = std::sync::mpsc::sync_channel::<()>(16);
    compile_sender.send(()).unwrap();
    let _compile_thread: JoinHandle<()> = thread::Builder::new()
        .name("compile".to_string())
        .spawn(move || {
            while let Ok(()) = compile_receiver.recv() {
                let Ok(module) = compiler::compile_module(&args_clone) else {
                    return;
                };
                shared_module_clone.write().unwrap().replace(module);

                let module_read = shared_module_clone.read().unwrap();
                let module = module_read.as_ref().unwrap();
                let llvm_ctx = inkwell::context::Context::create();
                let _codegen =
                    match compiler::codegen_module(&args_clone, &llvm_ctx, module, out_dir, true) {
                        Ok(codegen) => codegen,
                        Err(err) => {
                            eprintln!("Codegen error: {}", err);
                            return;
                        }
                    };
            }
        })
        .unwrap();

    let (run_sender, run_receiver) = std::sync::mpsc::sync_channel::<()>(16);
    let run_module_handle = module_handle.clone();
    let _run_thread: JoinHandle<()> = thread::Builder::new()
        .name("run".to_string())
        .spawn(move || {
            while let Ok(()) = run_receiver.recv() {
                let module_read = run_module_handle.read().unwrap();
                let Some(module) = module_read.as_ref() else {
                    println!("Cannot run; no module");
                    continue;
                };
                compiler::run_compiled_program(out_dir, module.name());
            }
        })
        .unwrap();

    if args.run && !args.gui {
        info!("waiting on compile thread");
        loop {
            let module = module_handle.try_read();
            if module.is_ok() && module.unwrap().is_some() {
                break;
            } else {
                thread::sleep(std::time::Duration::from_millis(1000));
            }
        }
        let module_read = module_handle.read().unwrap();
        let module = module_read.as_ref().unwrap();
        if !module.errors.is_empty() {
        } else {
            let module_name = module.name();
            info!("done waiting on compile thread");
            compiler::run_compiled_program(out_dir, module_name);
        }
    } else if args.gui {
        let mut gui = gui::Gui::init(module_handle.clone(), compile_sender, run_sender);

        gui.run_loop();
    } else {
        info!("waiting on compile thread");
        loop {
            let module = module_handle.try_read();
            if module.is_ok() && module.unwrap().is_some() {
                break;
            } else {
                thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}
