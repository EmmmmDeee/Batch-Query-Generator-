//! Oathnet Batch Query Generator — CLI entry point
//!
//! Usage:
//!   batch-query-generator                   # read from stdin
//!   batch-query-generator <file> [file...]  # read from one or more files
//!
//! Each non-blank, non-symbol-only line (or semicolon-separated segment) of
//! the input is turned into a structured Oathnet batch query and printed to
//! stdout.

use std::{
    env,
    fs,
    io::{self, Read},
    process,
};

use batch_query_generator::{format_batch, generate_batch_queries};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    let input = if args.is_empty() {
        // Read from stdin
        let mut buf = String::new();
        if let Err(e) = io::stdin().read_to_string(&mut buf) {
            eprintln!("Error reading stdin: {e}");
            process::exit(1);
        }
        buf
    } else {
        // Read from each file argument and concatenate
        let mut buf = String::new();
        for path in &args {
            match fs::read_to_string(path) {
                Ok(contents) => {
                    buf.push_str(&contents);
                    buf.push('\n');
                }
                Err(e) => {
                    eprintln!("Error reading file '{path}': {e}");
                    process::exit(1);
                }
            }
        }
        buf
    };

    let queries = generate_batch_queries(&input);

    if queries.is_empty() {
        eprintln!("No valid queries found in the provided input.");
        process::exit(1);
    }

    println!("Generated {} Oathnet batch {}:",
        queries.len(),
        if queries.len() == 1 { "query" } else { "queries" }
    );
    println!("{}", format_batch(&queries));
}
