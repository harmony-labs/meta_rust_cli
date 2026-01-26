//! meta-rust library
//!
//! Provides Rust/Cargo commands for meta repositories.

pub use meta_plugin_protocol::{
    CommandResult, ExecutionPlan, PlannedCommand, PlanResponse, output_execution_plan,
};
use std::path::Path;

/// Get all project directories from .meta config (including root ".")
/// If provided_projects is not empty, uses that list instead (for --recursive support)
fn get_project_directories(provided_projects: &[String], cwd: &Path) -> anyhow::Result<Vec<String>> {
    // If we have provided projects from meta_cli (e.g., when --recursive is used), use them
    if !provided_projects.is_empty() {
        // Include root "." plus all provided project paths
        let mut dirs = vec![".".to_string()];
        for p in provided_projects {
            dirs.push(p.clone());
        }
        return Ok(dirs);
    }

    // Use canonical config parsing (supports JSON + YAML)
    let tree = match meta_cli::config::walk_meta_tree(cwd, Some(0)) {
        Ok(t) => t,
        Err(_) => return Ok(vec![".".to_string()]),
    };
    let mut dirs = vec![".".to_string()];
    let mut paths: Vec<String> = tree.iter().map(|n| n.info.path.clone()).collect();
    paths.sort();
    dirs.extend(paths);
    Ok(dirs)
}

/// Filter directories to only those with Cargo.toml
fn filter_rust_projects(dirs: &[String], cwd: &Path) -> Vec<String> {
    dirs.iter()
        .filter(|dir| {
            let cargo_path = if *dir == "." {
                cwd.join("Cargo.toml")
            } else {
                cwd.join(dir).join("Cargo.toml")
            };
            cargo_path.exists()
        })
        .cloned()
        .collect()
}

/// Execute a Rust/Cargo command and return the result
///
/// If `provided_projects` is not empty, it will be used instead of reading from .meta file.
/// This allows meta_cli to pass in the full project list when --recursive is used.
pub fn execute_command(
    command: &str,
    args: &[String],
    parallel: bool,
    provided_projects: &[String],
    cwd: &Path,
) -> CommandResult {
    // Get all project directories
    let dirs = match get_project_directories(provided_projects, cwd) {
        Ok(d) => d,
        Err(e) => return CommandResult::Error(format!("Failed to get project directories: {e}")),
    };

    // Filter to Rust projects only
    let rust_dirs = filter_rust_projects(&dirs, cwd);

    if rust_dirs.is_empty() {
        return CommandResult::Message("No Rust projects found (no Cargo.toml files)".to_string());
    }

    // Build the cargo command
    let cargo_cmd = match command {
        "cargo build" => {
            let mut cmd = "cargo build".to_string();
            for arg in args {
                cmd.push(' ');
                cmd.push_str(arg);
            }
            cmd
        }
        "cargo test" => {
            let mut cmd = "cargo test".to_string();
            for arg in args {
                cmd.push(' ');
                cmd.push_str(arg);
            }
            cmd
        }
        _ => return CommandResult::ShowHelp(Some(format!("unrecognized command '{}'", command))),
    };

    // Build execution plan
    let commands: Vec<PlannedCommand> = rust_dirs
        .iter()
        .map(|dir| PlannedCommand {
            dir: dir.clone(),
            cmd: cargo_cmd.clone(),
        })
        .collect();

    CommandResult::Plan(commands, Some(parallel))
}

/// Get help text for the plugin
pub fn get_help_text() -> &'static str {
    r#"meta rust - Rust/Cargo Plugin

Commands:
  meta cargo build   Run cargo build across all Rust projects
  meta cargo test    Run cargo test across all Rust projects

This plugin detects Rust projects (by presence of Cargo.toml) and runs
the specified cargo command. Non-Rust directories are skipped.
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_unknown_command() {
        let result = execute_command("cargo unknown", &[], false, &[], Path::new("."));
        match result {
            CommandResult::ShowHelp(Some(msg)) => assert!(msg.contains("unrecognized command")),
            _ => panic!("Expected ShowHelp result"),
        }
    }

    #[test]
    fn test_get_help_text() {
        let help = get_help_text();
        assert!(help.contains("cargo build"));
        assert!(help.contains("cargo test"));
    }

    #[test]
    fn test_no_rust_projects() {
        let temp_dir = TempDir::new().unwrap();

        // Create .meta with no Rust projects
        std::fs::write(temp_dir.path().join(".meta"), r#"{"projects": {}}"#).unwrap();

        let result = execute_command("cargo build", &[], false, &[], temp_dir.path());

        match result {
            CommandResult::Message(msg) => assert!(msg.contains("No Rust projects")),
            _ => panic!("Expected Message result"),
        }
    }

    #[test]
    fn test_cargo_build_returns_plan() {
        let temp_dir = TempDir::new().unwrap();

        // Create a Cargo.toml in root
        std::fs::write(temp_dir.path().join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();
        std::fs::write(temp_dir.path().join(".meta"), r#"{"projects": {}}"#).unwrap();

        let result = execute_command("cargo build", &["--release".to_string()], true, &[], temp_dir.path());

        match result {
            CommandResult::Plan(commands, parallel) => {
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0].dir, ".");
                assert!(commands[0].cmd.contains("cargo build"));
                assert!(commands[0].cmd.contains("--release"));
                assert_eq!(parallel, Some(true));
            }
            _ => panic!("Expected Plan result"),
        }
    }

    #[test]
    fn test_execution_plan_serialization() {
        let commands = vec![
            PlannedCommand {
                dir: ".".to_string(),
                cmd: "cargo test".to_string(),
            },
        ];
        let plan = ExecutionPlan {
            commands,
            parallel: Some(true),
        };
        let response = PlanResponse { plan };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"plan\""));
        assert!(json.contains("\"commands\""));
        assert!(json.contains("cargo test"));
    }
}
