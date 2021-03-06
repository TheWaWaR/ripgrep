extern crate atty;
extern crate bytecount;
#[macro_use]
extern crate clap;
extern crate encoding_rs;
extern crate env_logger;
extern crate grep;
extern crate ignore;
#[macro_use]
extern crate lazy_static;
extern crate libc;
#[macro_use]
extern crate log;
extern crate memchr;
extern crate memmap;
extern crate num_cpus;
extern crate regex;
extern crate same_file;
extern crate termcolor;

use std::path::{PathBuf};
use std::error::Error;
use std::result;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

pub use grep::{Grep, GrepBuilder};

pub use args::Args;
pub use search_stream::LineMatch;
pub use ignore::WalkState;
use worker::Work;

macro_rules! errored {
    ($($tt:tt)*) => {
        return Err(From::from(format!($($tt)*)));
    }
}

#[macro_export]
macro_rules! eprintln {
    ($($tt:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(&mut ::std::io::stderr(), $($tt)*);
    }}
}

pub mod app;
pub mod args;
mod decoder;
mod pathutil;
mod printer;
mod search_buffer;
mod search_stream;
mod unescape;
mod worker;

pub type Result<T> = result::Result<T, Box<Error + Send + Sync>>;


#[derive(Eq, PartialEq)]
pub enum PredicateState {
    Nothing,
    Continue,
    Quit
}
pub struct FileMatch {
    pub path: PathBuf,
    pub lines: Vec<LineMatch>,
}

pub fn get_files<P>(
    args: Args,
    predicate: P,
) -> Result<Vec<PathBuf>>
    where P: Fn(usize, PathBuf) -> PredicateState + 'static
{
    let args = Arc::new(args);
    let predicate = Arc::new(predicate);
    run_files_one_thread(args, predicate)
}

pub fn get_matches<P>(
    args: Args,
    predicate: P,
) -> Result<(Grep, Vec<FileMatch>)>
    where P: Fn(usize, PathBuf) -> PredicateState + Send + Sync + 'static
{
    let args = Arc::new(args);
    let predicate = Arc::new(predicate);
    let args_grep = args.grep();
    let matches = if args.threads() == 1 || args.is_one_path() {
        run_one_thread(args, predicate)
    } else {
        run_parallel(args, predicate)
    };
    matches.map(|matches|(args_grep, matches))
}

pub fn run_parallel<P>(
    args: Arc<Args>,
    predicate: Arc<P>,
) -> Result<Vec<FileMatch>>
    where P: Fn(usize, PathBuf) -> PredicateState + Send + Sync + 'static
{
    let bufwtr = Arc::new(args.buffer_writer());
    let quiet_matched = args.quiet_matched();
    let paths_searched = Arc::new(AtomicUsize::new(0));
    let line_count = Arc::new(AtomicUsize::new(0));
    let file_count = Arc::new(AtomicUsize::new(0));

    let (matches_tx, matches_rx) = std::sync::mpsc::channel::<Option<FileMatch>>();
    let matches_handler = std::thread::spawn(move || {
        let mut matches = Vec::<FileMatch>::new();
        loop {
            if let Ok(current_match) = matches_rx.recv() {
                if let Some(file_match) = current_match {
                    matches.push(file_match);
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        matches
    });

    args.walker_parallel().run(|| {
        let matches_tx = matches_tx.clone();
        let args = args.clone();
        let quiet_matched = quiet_matched.clone();
        let paths_searched = paths_searched.clone();
        let line_count = line_count.clone();
        let file_count = file_count.clone();
        let predicate = predicate.clone();
        let bufwtr = bufwtr.clone();
        let mut buf = bufwtr.buffer();
        let mut worker = args.worker();
        Box::new(move |result| {
            use ignore::WalkState::*;

            if quiet_matched.has_match() {
                return Quit;
            }
            let dent = match get_or_log_dir_entry(
                result,
                args.stdout_handle(),
                args.no_messages(),
            ) {
                None => return Continue,
                Some(dent) => dent,
            };
            let state = predicate(file_count.load(Ordering::SeqCst)+1, dent.path().to_owned());
            if state == PredicateState::Continue {
                return Continue;
            }
            let path = dent.path().to_owned();

            paths_searched.fetch_add(1, Ordering::SeqCst);
            buf.clear();
            {
                // This block actually executes the search and prints the
                // results into outbuf.
                let mut printer = args.printer(&mut buf);
                let line_matches =
                    if dent.is_stdin() {
                        worker.run(printer.as_mut(), Work::Stdin)
                    } else {
                        worker.run(printer.as_mut(), Work::DirEntry(dent))
                    };
                let line_match_count = line_matches.len();
                if line_match_count > 0 {
                    line_count.fetch_add(1, Ordering::SeqCst);
                    line_count.fetch_add(line_match_count, Ordering::SeqCst);
                    let _ = matches_tx.send(Some(FileMatch {
                        path,
                        lines: line_matches
                    }));
                    if state == PredicateState::Quit {
                        return Quit;
                    }
                }
                if quiet_matched.set_match(line_match_count > 0) {
                    return Quit;
                }
            }
            // BUG(burntsushi): We should handle this error instead of ignoring
            // it. See: https://github.com/BurntSushi/ripgrep/issues/200
            let _ = bufwtr.print(&buf);
            Continue
        })
    });

    if !args.paths().is_empty() && paths_searched.load(Ordering::SeqCst) == 0 {
        if !args.no_messages() {
            if !args.no_printer() {
                eprint_nothing_searched();
            }
        }
    }
    // Ok(match_count.load(Ordering::SeqCst) as u64)
    let _ = matches_tx.send(None);
    Ok(matches_handler.join().unwrap())
}

pub fn run_one_thread<P>(
    args: Arc<Args>,
    predicate: Arc<P>,
) -> Result<Vec<FileMatch>>
    where P: Fn(usize, PathBuf) -> PredicateState
{
    let stdout = args.stdout();
    let mut stdout = stdout.lock();
    let mut worker = args.worker();
    let mut paths_searched: u64 = 0;
    let mut file_matches = Vec::new();
    let mut line_count = 0;
    let mut file_count = 0;
    for result in args.walker() {
        let dent = match get_or_log_dir_entry(
            result,
            args.stdout_handle(),
            args.no_messages(),
        ) {
            None => continue,
            Some(dent) => dent,
        };

        let state = predicate(file_count+1, dent.path().to_owned());
        if state == PredicateState::Continue {
            continue;
        }
        let path = dent.path().to_owned();

        let mut printer = args.printer(&mut stdout);
        if line_count > 0 {
            if args.quiet() {
                break;
            }
            if let Some(sep) = args.file_separator() {
                if let Some(p) = printer {
                    printer = Some(p.file_separator(sep));
                }
            }
        }
        paths_searched += 1;
        let line_matches =
            if dent.is_stdin() {
                worker.run(printer.as_mut(), Work::Stdin)
            } else {
                worker.run(printer.as_mut(), Work::DirEntry(dent))
            };

        if !line_matches.is_empty() {
            file_count += 1;
            line_count += line_matches.len() as u64;
            file_matches.push(FileMatch {
                path,
                lines: line_matches,
            });
            if state == PredicateState::Quit {
                break;
            }
        }
    }
    if !args.paths().is_empty() && paths_searched == 0 {
        if !args.no_messages() {
            if !args.no_printer() {
                eprint_nothing_searched();
            }
        }
    }
    Ok(file_matches)
}

pub fn run_files_parallel(
    args: Arc<Args>,
    _find: Option<Grep>,
    _limit: Option<usize>,
) -> Result<u64> {
    let print_args = args.clone();
    let (tx, rx) = mpsc::channel::<ignore::DirEntry>();
    let print_thread = thread::spawn(move || {
        let stdout = print_args.stdout();
        let mut printer = print_args.printer(stdout.lock());
        let mut file_count = 0;
        for dent in rx.iter() {
            if !print_args.quiet() {
                if let Some(ref mut p) = printer {
                    p.path(dent.path());
                }
            }
            file_count += 1;
        }
        file_count
    });
    args.walker_parallel().run(move || {
        let args = args.clone();
        let tx = tx.clone();
        Box::new(move |result| {
            if let Some(dent) = get_or_log_dir_entry(
                result,
                args.stdout_handle(),
                args.no_messages(),
            ) {
                tx.send(dent).unwrap();
            }
            ignore::WalkState::Continue
        })
    });
    Ok(print_thread.join().unwrap())
}

pub fn run_files_one_thread<P>(
    args: Arc<Args>,
    predicate: Arc<P>,
) -> Result<Vec<PathBuf>>
    where P: Fn(usize, PathBuf) -> PredicateState + 'static
{
    let stdout = args.stdout();
    let mut printer = args.printer(stdout.lock());
    let mut file_count = 0;
    let mut files = Vec::new();
    for result in args.walker() {
        let dent = match get_or_log_dir_entry(
            result,
            args.stdout_handle(),
            args.no_messages(),
        ) {
            None => continue,
            Some(dent) => dent,
        };
        let state = predicate(file_count+1, dent.path().to_owned());
        if state == PredicateState::Continue {
            continue;
        }
        let path = dent.path().to_owned();

        files.push(path);
        if !args.quiet() {
            if let Some(ref mut p) = printer {
                p.path(dent.path());
            }
        }
        file_count += 1;
        if state == PredicateState::Quit {
            break;
        }
    }
    Ok(files)
}

pub fn run_types(args: Arc<Args>) -> Result<u64> {
    let stdout = args.stdout();
    let mut printer = args.printer(stdout.lock());
    let mut ty_count = 0;
    for def in args.type_defs() {
        if let Some(ref mut p) = printer {
            p.type_def(def);
        }
        ty_count += 1;
    }
    Ok(ty_count)
}

fn get_or_log_dir_entry(
    result: result::Result<ignore::DirEntry, ignore::Error>,
    stdout_handle: Option<&same_file::Handle>,
    no_messages: bool,
) -> Option<ignore::DirEntry> {
    match result {
        Err(err) => {
            if !no_messages {
                eprintln!("{}", err);
            }
            None
        }
        Ok(dent) => {
            if let Some(err) = dent.error() {
                if !no_messages {
                    eprintln!("{}", err);
                }
            }
            let ft = match dent.file_type() {
                None => return Some(dent), // entry is stdin
                Some(ft) => ft,
            };
            // A depth of 0 means the user gave the path explicitly, so we
            // should always try to search it.
            if dent.depth() == 0 && !ft.is_dir() {
                return Some(dent);
            } else if !ft.is_file() {
                return None;
            }
            // If we are redirecting stdout to a file, then don't search that
            // file.
            if is_stdout_file(&dent, stdout_handle, no_messages) {
                return None;
            }
            Some(dent)
        }
    }
}

fn is_stdout_file(
    dent: &ignore::DirEntry,
    stdout_handle: Option<&same_file::Handle>,
    no_messages: bool,
) -> bool {
    let stdout_handle = match stdout_handle {
        None => return false,
        Some(stdout_handle) => stdout_handle,
    };
    // If we know for sure that these two things aren't equal, then avoid
    // the costly extra stat call to determine equality.
    if !maybe_dent_eq_handle(dent, stdout_handle) {
        return false;
    }
    match same_file::Handle::from_path(dent.path()) {
        Ok(h) => stdout_handle == &h,
        Err(err) => {
            if !no_messages {
                eprintln!("{}: {}", dent.path().display(), err);
            }
            false
        }
    }
}

#[cfg(unix)]
fn maybe_dent_eq_handle(
    dent: &ignore::DirEntry,
    handle: &same_file::Handle,
) -> bool {
    dent.ino() == Some(handle.ino())
}

#[cfg(not(unix))]
fn maybe_dent_eq_handle(_: &ignore::DirEntry, _: &same_file::Handle) -> bool {
    true
}

fn eprint_nothing_searched() {
    eprintln!("No files were searched, which means ripgrep probably \
               applied a filter you didn't expect. \
               Try running again with --debug.");
}
