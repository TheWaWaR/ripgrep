#[macro_use]
extern crate ripgrep;

use std::process;
use std::sync::Arc;

use ripgrep::{
    Args, Result,
    run_files_one_thread,
    run_files_parallel,
    run_types,
    run_one_thread,
    run_parallel,
};

fn main() {
    match Args::parse().map(Arc::new).and_then(run) {
        Ok(0) => process::exit(1),
        Ok(_) => process::exit(0),
        Err(err) => {
            eprintln!("{}", err);
            process::exit(1);
        }
    }
}

fn run(args: Arc<Args>) -> Result<u64> {
    if args.never_match() {
        return Ok(0);
    }
    let threads = args.threads();
    if args.files() {
        if threads == 1 || args.is_one_path() {
            run_files_one_thread(args, None, None)
                .map(|files| files.len() as u64)
        } else {
            run_files_parallel(args, None, None)
        }
    } else if args.type_list() {
        run_types(args)
    } else if threads == 1 || args.is_one_path() {
        run_one_thread(args, None, None)
            .map(|matches| matches.len() as u64)
    } else {
        run_parallel(args, None, None)
            .map(|matches| matches.len() as u64)
    }
}

