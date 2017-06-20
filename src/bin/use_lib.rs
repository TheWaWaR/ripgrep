extern crate clap;
#[macro_use]
extern crate ripgrep;

use std::process;
use std::sync::Arc;

use ripgrep::{
    Args, FileMatch, LineMatch,
    app, get_matches
};

fn main() {
    let arg_vec = vec!["xxx", "home", "-i", "-j", "1", "--no-printer"];
    let args = Args::from(app::app().get_matches_from(arg_vec));
    println!("Args: {:?}", args);
    match args.map(Arc::new).and_then(get_matches) {
        Ok((grep, file_matchs)) => {
            if file_matchs.is_empty() {
                process::exit(1);
            }
            println!("====================");
            for FileMatch{ path, lines } in file_matchs {
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
