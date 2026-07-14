//! meta-rust subprocess plugin

use meta_plugin_protocol::{
    run_plugin, CommandResult, PluginDefinition, PluginInfo, PluginRequest,
};
use std::path::PathBuf;

fn main() {
    run_plugin(PluginDefinition {
        info: PluginInfo {
            name: "rust".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commands: vec!["cargo".to_string(), "rust".to_string()],
            description: Some("Cargo command pass-through for Meta workspaces".to_string()),
            help: Some(meta_rust_cli::plugin_help()),
        },
        execute,
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

    meta_rust_cli::execute_command_with_filters(
        &request.command,
        &request.args,
        request.options.parallel,
        &request.projects,
        &cwd,
        request.options.include_filters.as_deref(),
        request.options.exclude_filters.as_deref(),
    )
}
