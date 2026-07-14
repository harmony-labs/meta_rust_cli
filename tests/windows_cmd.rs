#![cfg(windows)]

use meta_rust_cli::{execute_command, CommandResult};
use std::ffi::OsString;
use std::process::Command;
use tempfile::TempDir;

fn encode_utf16(value: &str) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    let encoded = units
        .iter()
        .map(|unit| format!("{unit:04x}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{}:{encoded}", units.len())
}

fn compile_fake_cargo(temp: &TempDir) -> std::path::PathBuf {
    let bin = temp.path().join("probe bin");
    std::fs::create_dir_all(&bin).unwrap();
    let source = temp.path().join("argv_probe.rs");
    std::fs::write(
        &source,
        r#"
use std::os::windows::ffi::OsStrExt;

fn main() {
    for arg in std::env::args_os().skip(1) {
        let units: Vec<u16> = arg.encode_wide().collect();
        let encoded = units
            .iter()
            .map(|unit| format!("{unit:04x}"))
            .collect::<Vec<_>>()
            .join(",");
        println!("{}:{encoded}", units.len());
    }
}
"#,
    )
    .unwrap();

    let cargo = bin.join("cargo.exe");
    let status = Command::new("rustc")
        .args(["--edition=2021", "-o"])
        .arg(&cargo)
        .arg(&source)
        .status()
        .expect("rustc must be available while running Rust tests");
    assert!(
        status.success(),
        "failed to compile the fake cargo argv probe"
    );
    cargo
}

#[test]
fn planned_command_survives_the_real_cmd_boundary_without_injection() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='windows-cmd-probe'\nversion='0.0.0'\n",
    )
    .unwrap();
    let cargo = compile_fake_cargo(&temp);
    let marker = temp.path().join("injected marker");
    let injection = format!("quoted\" & type nul > \"{}\" & rem \"", marker.display());
    let args = vec![
        "probe".to_string(),
        "value with spaces".to_string(),
        "%PATH%".to_string(),
        "prefix%PATH%suffix".to_string(),
        "%hello".to_string(),
        "%%cd:~,%".to_string(),
        "%META_RUST_PCT%".to_string(),
        "%CD%".to_string(),
        "%0".to_string(),
        "%%%%".to_string(),
        "%PATH%PATH%".to_string(),
        "amp&ersand".to_string(),
        "pipe|value".to_string(),
        "redirect>value".to_string(),
        "caret^value".to_string(),
        "paren(value)".to_string(),
        "quoted\"&echo injected".to_string(),
        "trailing\\".to_string(),
        "bang!PATH!".to_string(),
        "semi;colon".to_string(),
        "unicode-λ".to_string(),
        String::new(),
        injection.clone(),
    ];
    let projects = vec![temp.path().to_string_lossy().into_owned()];

    let planned = match execute_command("cargo", &args, false, &projects, temp.path()) {
        CommandResult::Plan(commands, _) => commands.into_iter().next().unwrap(),
        _ => panic!("expected a Cargo execution plan"),
    };

    let mut path_entries = vec![cargo.parent().unwrap().to_path_buf()];
    path_entries.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path: OsString = std::env::join_paths(path_entries).unwrap();
    let mut process = Command::new("cmd.exe");
    process
        .arg("/c")
        .arg(&planned.cmd)
        .current_dir(temp.path())
        .env("PATH", path)
        .env("META_RUST_PCT", &injection);
    if let Some(environment) = &planned.env {
        process.envs(environment);
    }
    let output = process.output().unwrap();

    assert!(
        output.status.success(),
        "cmd failed:\nstdout: {}\nstderr: {}\ncommand: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        planned.cmd
    );
    assert!(!marker.exists(), "hostile argument escaped into cmd syntax");

    let actual: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let expected: Vec<String> = args.iter().map(|arg| encode_utf16(arg)).collect();
    assert_eq!(actual, expected);
}

#[test]
fn planned_command_rejects_cmd_line_breaks() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='windows-cmd-probe'\nversion='0.0.0'\n",
    )
    .unwrap();
    let projects = vec![temp.path().to_string_lossy().into_owned()];

    let result = execute_command(
        "cargo",
        &["check".to_string(), "line\nbreak".to_string()],
        false,
        &projects,
        temp.path(),
    );
    assert!(matches!(result, CommandResult::Error(message) if message.contains("cmd.exe")));
}
