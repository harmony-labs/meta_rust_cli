use meta_plugin_api::{Plugin, PluginError};

pub struct RustPlugin;

impl Plugin for RustPlugin {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn commands(&self) -> Vec<&'static str> {
        vec!["cargo build", "cargo test"]
    }

    fn execute(&self, command: &str, args: &[String]) -> anyhow::Result<()> {
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
            _ => Err(PluginError::CommandNotFound(command.to_string()).into()),
        }
    }
}

#[no_mangle]
pub extern "C" fn _plugin_create() -> *mut dyn Plugin {
    Box::into_raw(Box::new(RustPlugin))
}