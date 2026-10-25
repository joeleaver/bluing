//! `blu`, the Bluing command-line tool.

use std::{fs, process::ExitCode};

use bluing_syntax::{LineIndex, Options};

const USAGE: &str = "\
usage: blu check [--allow-unsafe] <file.blu>...
       blu --version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => check(&args[1..]),
        Some("--version" | "-V") => {
            println!("blu {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn check(args: &[String]) -> ExitCode {
    let mut options = Options::default();
    let mut paths = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--allow-unsafe" => options.allow_unsafe = true,
            flag if flag.starts_with('-') => {
                eprintln!("blu: unknown option `{flag}`\n{USAGE}");
                return ExitCode::from(2);
            }
            path => paths.push(path),
        }
    }
    if paths.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let mut errors = 0;
    for path in paths {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("blu: cannot read {path}: {err}");
                return ExitCode::from(2);
            }
        };
        let index = LineIndex::new(&text);
        for diagnostic in bluing_syntax::check(&text, &options) {
            let pos = index.line_col(diagnostic.range.start());
            eprintln!(
                "{path}:{}:{}: error[{}]: {}",
                pos.line, pos.col, diagnostic.code, diagnostic.message
            );
            errors += 1;
        }
    }
    if errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
