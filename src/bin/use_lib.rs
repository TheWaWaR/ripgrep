extern crate clap;
#[macro_use]
extern crate ripgrep;

use std::process;

use ripgrep::{
    Args, FileMatch, LineMatch,
    app, get_matches
};
use ripgrep::PredicateState::*;

fn main() {
    let arg_vec = vec![
        "xxx", "home",
        "-i",
        "-j", "1",
        "--no-printer",
        "--max-count", "3",
    ];
    let args = Args::from(app::app().get_matches_from(arg_vec)).unwrap();
    println!("Args: {:?}", args);
    let predicate = |count, _| {
        if count >= 5 { Quit } else { Nothing }
    };
    match get_matches(args, predicate) {
        Ok((grep, file_matches)) => {
            if file_matches.is_empty() {
                process::exit(1);
            }
            println!("====================");
            for FileMatch{ path, lines } in file_matches {
                if !lines.is_empty() {
                    println!(">[Path]: {:?}", path);
                    for LineMatch{ line_number, buf } in lines {
                        let current_line = String::from_utf8(buf.clone()).unwrap();
                        println!("   [{}]: {:?}",
                                 line_number.unwrap_or(0), current_line);
                        for m in grep.regex().find_iter(&buf) {
                            println!(
                                "     [Match]: start={:?}, end={}, content={:?}",
                                m.start(), m.end(),
                                String::from_utf8(buf[m.start()..m.end()].to_vec().clone()).unwrap()
                            );
                        }
                    }
                    println!("");
                }
            }
        }
        Err(err) => {
            eprintln!("{}", err);
            process::exit(1);
        }
    }
}
