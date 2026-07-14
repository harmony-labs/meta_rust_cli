use std::io::Write;
use std::process::{Command, Output, Stdio};

fn invoke_plugin(request: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_meta-rust"))
        .arg("--meta-plugin-exec")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn capability_free_operations_fail_closed_but_help_remains_available() {
    let operation = invoke_plugin(
        r#"{
            "command": "cargo",
            "args": ["check"],
            "projects": [],
            "cwd": ".",
            "options": {}
        }"#,
    );
    assert!(!operation.status.success());
    assert!(String::from_utf8_lossy(&operation.stderr).contains("plan-execution-policy-v1"));

    let help = invoke_plugin(
        r#"{
            "command": "cargo",
            "args": ["--help"],
            "projects": [],
            "cwd": ".",
            "options": {}
        }"#,
    );
    assert!(
        help.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&help.stderr)
    );
    assert!(String::from_utf8_lossy(&help.stdout).contains("meta [META OPTIONS] cargo"));
}
