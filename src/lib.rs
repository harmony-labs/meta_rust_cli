//! meta-rust library
//!
//! Provides Rust/Cargo commands for meta repositories.

/// Execute a Rust/Cargo command
pub fn execute_command(command: &str, args: &[String]) -> anyhow::Result<()> {
    // Check if current directory has Cargo.toml
    if !std::path::Path::new("Cargo.toml").exists() {
        println!("Skipping: no Cargo.toml in this directory");
        return Ok(());
    }

    match command {
        "cargo build" => {
            let status = std::process::Command::new("cargo")
                .arg("build")
                .args(args)
                .status()?;
            if !status.success() {
                anyhow::bail!("cargo build failed");
            }
            Ok(())
        }
        "cargo test" => {
            let status = std::process::Command::new("cargo")
                .arg("test")
                .args(args)
                .status()?;
            if !status.success() {
                anyhow::bail!("cargo test failed");
            }
            Ok(())
        }
        _ => Err(anyhow::anyhow!("Unknown command: {}", command)),
    }
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

    #[test]
    fn test_unknown_command() {
        let result = execute_command("cargo unknown", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_help_text() {
        let help = get_help_text();
        assert!(help.contains("cargo build"));
        assert!(help.contains("cargo test"));
    }
}
