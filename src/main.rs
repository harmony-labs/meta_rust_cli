//! meta-rust subprocess plugin

use meta_plugin_protocol::{
    CommandResult, PluginDefinition, PluginHelp, PluginInfo, PluginRequest, run_plugin,
};
use std::collections::HashMap;
use std::path::PathBuf;

fn main() {
    let mut help_commands = HashMap::new();
    help_commands.insert(
        "build".to_string(),
        "Build all Rust projects in the workspace".to_string(),
    );
    help_commands.insert(
        "test".to_string(),
        "Run tests across all Rust projects".to_string(),
    );

    run_plugin(PluginDefinition {
        info: PluginInfo {
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
        },
        execute: execute,
    });
}

fn execute(request: PluginRequest) -> CommandResult {
    let cwd = if request.cwd.is_empty() {
        match std::env::current_dir() {
            Ok(d) => d,
            Err(e) => return CommandResult::Error(format!("Failed to get working directory: {e}")),
        }
    } else {
        PathBuf::from(&request.cwd)
    };

    meta_rust_cli::execute_command(
        &request.command,
        &request.args,
        request.options.parallel,
        &request.projects,
        &cwd,
    )
}
