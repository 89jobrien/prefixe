mod config;
mod payload;
mod post_hook;
mod predicate;

use std::io::Read;
use std::process;

use prefixe::{FileProbeStore, FileStatsStore, PrefixEngine};

fn print_usage() {
    eprintln!("Usage: prefixe rewrite [--dry-run] [--config <path>] <command...>");
    eprintln!("       prefixe post-hook   (reads Claude Code JSON from stdin)");
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
        "post-hook" => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).unwrap_or(0);
            let payload: payload::HookPayload =
                serde_json::from_str(&input).unwrap_or_else(|_| process::exit(0));
            if payload.tool_name.as_deref() != Some("Bash") {
                process::exit(0);
            }
            let command = match payload
                .tool_input
                .as_ref()
                .and_then(|i| i.command.as_deref())
            {
                Some(c) if !c.is_empty() => c.to_string(),
                _ => process::exit(0),
            };
            let exit_code = payload
                .tool_response
                .as_ref()
                .and_then(|r| r.exit_code)
                .unwrap_or(0);
            if exit_code != 0 {
                let prefix_store =
                    prefixe::FilePrefixStore::new(prefixe::FilePrefixStore::default_path());
                let probe_store = FileProbeStore::new(FileProbeStore::default_path());
                let stats_store = FileStatsStore::new(FileStatsStore::default_path());
                let out =
                    post_hook::handle_failure(&command, &prefix_store, &probe_store, &stats_store);
                println!("{}", serde_json::to_string(&out).unwrap_or_default());
            }
        }
        other => {
            eprintln!("prefixe: unknown subcommand '{other}'");
            print_usage();
            process::exit(1);
        }
    }
}
