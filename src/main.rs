//! meta-rust subprocess plugin
//!
//! Provides Rust/Cargo commands for meta repositories.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Read};

/// Plugin info returned by --meta-plugin-info
#[derive(Debug, Serialize)]
struct PluginInfo {
    name: String,
    version: String,
    commands: Vec<String>,
    description: Option<String>,
    help: Option<PluginHelp>,
}

/// Help information for the plugin
#[derive(Debug, Serialize)]
struct PluginHelp {
    usage: String,
    commands: HashMap<String, String>,
    examples: Vec<String>,
    note: Option<String>,
}

/// Request received from meta CLI via --meta-plugin-exec
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PluginRequest {
    command: String,
    args: Vec<String>,
    #[serde(default)]
    projects: Vec<String>,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    options: PluginRequestOptions,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct PluginRequestOptions {
    #[serde(default)]
    json_output: bool,
    #[serde(default)]
    verbose: bool,
    #[serde(default)]
    parallel: bool,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    silent: bool,
    #[serde(default)]
    include_filters: Option<Vec<String>>,
    #[serde(default)]
    exclude_filters: Option<Vec<String>>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: meta-rust --meta-plugin-info | --meta-plugin-exec");
        std::process::exit(1);
    }

    match args[1].as_str() {
        "--meta-plugin-info" => {
            let mut help_commands = HashMap::new();
            help_commands.insert(
                "build".to_string(),
                "Build all Rust projects in the workspace".to_string(),
            );
            help_commands.insert(
                "test".to_string(),
                "Run tests across all Rust projects".to_string(),
            );

            let info = PluginInfo {
                name: "rust".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                commands: vec!["cargo build".to_string(), "cargo test".to_string()],
                description: Some("Rust/Cargo commands for meta repositories".to_string()),
                help: Some(PluginHelp {
                    usage: "meta cargo <command> [args...]".to_string(),
                    commands: help_commands,
                    examples: vec![
                        "meta cargo build".to_string(),
                        "meta cargo test".to_string(),
                        "meta cargo build --release".to_string(),
                    ],
                    note: Some(
                        "To run raw cargo commands: meta exec -- cargo <command>".to_string(),
                    ),
                }),
            };
            println!("{}", serde_json::to_string(&info)?);
        }
        "--meta-plugin-exec" => {
            use meta_rust_cli::{CommandResult, output_execution_plan};

            // Read JSON request from stdin
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;

            let request: PluginRequest = serde_json::from_str(&input)?;

            // Change to the specified working directory if provided
            if !request.cwd.is_empty() {
                std::env::set_current_dir(&request.cwd)?;
            }

            // Execute the command
            let result = meta_rust_cli::execute_command(
                &request.command,
                &request.args,
                request.options.parallel,
            );

            match result {
                CommandResult::Plan(commands, parallel) => {
                    // Output execution plan for the shim to execute via loop_lib
                    output_execution_plan(commands, parallel);
                }
                CommandResult::Message(msg) => {
                    // Just print the message
                    println!("{msg}");
                }
                CommandResult::Error(msg) => {
                    eprintln!("Error: {msg}");
                    std::process::exit(1);
                }
            }
        }
        "--help" | "-h" => {
            println!("{}", meta_rust_cli::get_help_text());
        }
        _ => {
            eprintln!("Unknown argument: {}", args[1]);
            eprintln!("Usage: meta-rust --meta-plugin-info | --meta-plugin-exec");
            std::process::exit(1);
        }
    }

    Ok(())
}
