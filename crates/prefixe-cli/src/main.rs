mod config;
mod payload;
mod predicate;

use std::process;

use prefixe::{FileProbeStore, PrefixEngine};

fn print_usage() {
    eprintln!("Usage: prefixe rewrite [--dry-run] [--config <path>] <command...>");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        process::exit(1);
    }

    match args[0].as_str() {
        "rewrite" => {
            let mut dry_run = false;
            let mut config_path: Option<String> = None;
            let mut cmd_parts: Vec<String> = Vec::new();
            let mut i = 1usize;
            while i < args.len() {
                match args[i].as_str() {
                    "--dry-run" => {
                        dry_run = true;
                        i += 1;
                    }
                    "--config" => {
                        i += 1;
                        if i >= args.len() {
                            eprintln!("prefixe: --config requires a path argument");
                            process::exit(1);
                        }
                        config_path = Some(args[i].clone());
                        i += 1;
                    }
                    _ => {
                        cmd_parts.extend_from_slice(&args[i..]);
                        break;
                    }
                }
            }

            if cmd_parts.is_empty() {
                eprintln!("prefixe rewrite: no command given");
                print_usage();
                process::exit(1);
            }

            let cmd = cmd_parts.join(" ");
            let cfg_path = config::resolve_config_path(config_path.as_deref());
            let store = match config::load_store(cfg_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("prefixe: {e}");
                    process::exit(2);
                }
            };
            let probe_path = FileProbeStore::default_path();
            let probe_store = FileProbeStore::new(probe_path);
            let engine = PrefixEngine::new(store, probe_store);
            let result = engine.rewrite(&cmd);

            // --dry-run and normal mode both print; future versions may differ
            let _ = dry_run;
            println!("{}", result.rewritten);
        }
        other => {
            eprintln!("prefixe: unknown subcommand '{other}'");
            print_usage();
            process::exit(1);
        }
    }
}
