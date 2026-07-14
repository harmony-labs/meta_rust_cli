//! meta-rust library
//!
//! Provides Rust/Cargo commands for meta repositories.

use indexmap::IndexMap;
pub use meta_plugin_protocol::{
    output_execution_plan, CommandResult, ExecutionPlan, PlanExecutionPolicy, PlanResponse,
    PlannedCommand, PluginHelp, HOST_CAPABILITY_PLAN_EXECUTION_POLICY_V1,
};
#[cfg(not(windows))]
use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

/// Normalize project paths without requiring them to exist.
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    normalized
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectDirectory {
    execution_path: String,
    filter_paths: Vec<String>,
}

/// Preserve original path spellings while deduplicating canonical execution paths.
fn normalize_project_directories(paths: &[String], cwd: &Path) -> Vec<ProjectDirectory> {
    let base = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(cwd)
    };
    let mut normalized = IndexMap::<PathBuf, Vec<String>>::new();

    for path in paths {
        let path = Path::new(path);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        };
        let filter_path = absolute.to_string_lossy().into_owned();

        // Resolve the original path before lexically collapsing `..`.
        // Filesystem traversal through `symlink/..` is not necessarily
        // equivalent to removing both components textually.
        let execution_path = std::fs::canonicalize(&absolute).unwrap_or_else(|_| {
            let lexical = normalize_path(&absolute);
            std::fs::canonicalize(&lexical).unwrap_or(lexical)
        });

        let filter_paths = normalized.entry(execution_path).or_default();
        if !filter_paths.contains(&filter_path) {
            filter_paths.push(filter_path);
        }
    }

    normalized
        .into_iter()
        .map(|(execution_path, filter_paths)| ProjectDirectory {
            execution_path: execution_path.to_string_lossy().into_owned(),
            filter_paths,
        })
        .collect()
}

/// Get normalized project directories from the host or local Meta config.
///
/// If `provided_projects` is non-empty, that list is authoritative.
fn get_project_directories(
    provided_projects: &[String],
    cwd: &Path,
) -> anyhow::Result<Vec<ProjectDirectory>> {
    // A host-supplied project list already includes the Meta root and reflects
    // recursion, worktree selection, and tag filtering. Treat it as authoritative;
    // include/exclude filters are applied below before Rust-project detection.
    if !provided_projects.is_empty() {
        return Ok(normalize_project_directories(provided_projects, cwd));
    }

    // Use canonical config parsing (supports JSON + YAML)
    let tree = match meta_core::config::walk_meta_tree(cwd, Some(0)) {
        Ok(t) => t,
        Err(_) => {
            return Ok(normalize_project_directories(&[".".to_string()], cwd));
        }
    };
    let mut dirs = vec![".".to_string()];
    let mut paths: Vec<String> = tree.iter().map(|n| n.info.path.clone()).collect();
    paths.sort();
    dirs.extend(paths);
    Ok(normalize_project_directories(&dirs, cwd))
}

/// Filter directories to only those with Cargo.toml
fn filter_rust_projects(dirs: &[ProjectDirectory]) -> Vec<String> {
    dirs.iter()
        .filter(|dir| Path::new(&dir.execution_path).join("Cargo.toml").is_file())
        .map(|dir| dir.execution_path.clone())
        .collect()
}

#[cfg(any(windows, test))]
fn windows_filter_match_key(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let normalized = if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = normalized.strip_prefix("//?/") {
        rest.to_string()
    } else {
        normalized
    };

    normalized.trim_end_matches('/').to_ascii_lowercase()
}

#[cfg(any(windows, test))]
fn windows_filter_matches_path(path: &str, filter: &str) -> bool {
    windows_filter_match_key(path).contains(&windows_filter_match_key(filter))
}

fn filter_matches_path(path: &str, filter: &str) -> bool {
    #[cfg(windows)]
    {
        // canonicalize() returns verbatim paths on Windows and may also expand
        // short names or traverse junctions. Canonicalize an absolute filter
        // too when possible so it has the same spelling as project paths.
        let canonical_filter = Path::new(filter)
            .is_absolute()
            .then(|| std::fs::canonicalize(filter).ok())
            .flatten()
            .map(|path| path.to_string_lossy().into_owned());
        let filter = canonical_filter.as_deref().unwrap_or(filter);

        windows_filter_matches_path(path, filter)
    }

    #[cfg(not(windows))]
    {
        path.contains(filter.trim_end_matches('/'))
    }
}

fn project_matches_filter(project: &ProjectDirectory, filter: &str) -> bool {
    project
        .filter_paths
        .iter()
        .any(|path| filter_matches_path(path, filter))
        || filter_matches_path(&project.execution_path, filter)
}

/// Apply the host's directory selection before deciding whether the scope has
/// any Rust projects. loop_lib applies the same filters during execution, but
/// planning must see the selected scope to produce a clear empty result.
fn filter_selected_projects(
    dirs: &[ProjectDirectory],
    include_filters: Option<&[String]>,
    exclude_filters: Option<&[String]>,
) -> Vec<ProjectDirectory> {
    let mut selected = dirs.to_vec();

    if let Some(includes) = include_filters.filter(|filters| !filters.is_empty()) {
        selected.retain(|path| {
            includes
                .iter()
                .any(|filter| project_matches_filter(path, filter))
        });
    }

    if let Some(excludes) = exclude_filters.filter(|filters| !filters.is_empty()) {
        selected.retain(|path| {
            !excludes
                .iter()
                .any(|filter| project_matches_filter(path, filter))
        });
    }

    selected
}

fn help_requested(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
}

#[cfg(any(windows, test))]
const WINDOWS_NEWLINE_ERROR: &str =
    "Cargo arguments containing carriage returns or newlines cannot be transported safely through cmd.exe";
#[cfg(any(windows, test))]
const WINDOWS_NUL_ERROR: &str =
    "Cargo arguments containing NUL bytes cannot be transported through cmd.exe";
#[cfg(any(windows, test))]
const WINDOWS_LITERAL_PERCENT_ENV: &str = "META_RUST_PCT";
#[cfg(any(windows, test))]
const WINDOWS_LITERAL_QUOTE_ENV: &str = "META_RUST_Q";

#[cfg(any(windows, test))]
fn windows_cmd_token_needs_quotes(token: &str) -> bool {
    const UNQUOTED: &str = r"#$*+-./:?@\_";

    token.is_empty()
        || token.ends_with('\\')
        || token.chars().any(|ch| {
            let ascii_needs_quotes =
                ch.is_ascii() && !(ch.is_ascii_alphanumeric() || UNQUOTED.contains(ch));
            ascii_needs_quotes || ch.is_control()
        })
}

#[cfg(any(windows, test))]
fn quote_windows_cmd_token(token: &str) -> Result<String, &'static str> {
    // This follows Rust's hardened batch-argument encoding. cmd.exe does not
    // understand the CRT-style `\\\"` quote emitted by generic Windows argv
    // serializers, so embedded quotes must instead be doubled. Quote a broad
    // denylist of ASCII syntax to keep cmd metacharacters inside the argument.
    //
    // cmd has no direct escape for a literal `%` on a `/C` command line. Use a
    // controlled environment-variable expansion whose value is one percent.
    // Expansion is a single pass, so a reconstructed `%NAME%` remains literal
    // instead of being reinterpreted as another environment-variable reference.
    if token.contains('\0') {
        return Err(WINDOWS_NUL_ERROR);
    }
    if token.contains(['\r', '\n']) {
        return Err(WINDOWS_NEWLINE_ERROR);
    }

    let quote = windows_cmd_token_needs_quotes(token);

    let mut escaped = String::with_capacity(token.len() + 2);
    if quote {
        escaped.push('%');
        escaped.push_str(WINDOWS_LITERAL_QUOTE_ENV);
        escaped.push('%');
    }

    let mut backslashes = 0;
    for ch in token.chars() {
        if ch == '\\' {
            backslashes += 1;
            escaped.push(ch);
            continue;
        }

        if ch == '"' {
            // Add n backslashes to the n already emitted, then add the first
            // of a doubled quote pair. Fixed environment references defer the
            // actual quote characters until cmd parses the command string.
            escaped.extend(std::iter::repeat_n('\\', backslashes));
            for _ in 0..2 {
                escaped.push('%');
                escaped.push_str(WINDOWS_LITERAL_QUOTE_ENV);
                escaped.push('%');
            }
            backslashes = 0;
            continue;
        } else if ch == '%' {
            escaped.push('%');
            escaped.push_str(WINDOWS_LITERAL_PERCENT_ENV);
            escaped.push('%');
            backslashes = 0;
            continue;
        }
        backslashes = 0;
        escaped.push(ch);
    }

    if quote {
        // A quoted argument's trailing backslashes must be doubled so they do
        // not consume the closing quote in the receiving CRT argv parser.
        escaped.extend(std::iter::repeat_n('\\', backslashes));
        escaped.push('%');
        escaped.push_str(WINDOWS_LITERAL_QUOTE_ENV);
        escaped.push('%');
    }
    Ok(escaped)
}

#[cfg(any(windows, test))]
fn windows_shell_transport_environment(
    tokens: &[String],
) -> Option<std::collections::HashMap<String, String>> {
    let needs_percent = tokens.iter().any(|token| token.contains('%'));
    let needs_quote = tokens
        .iter()
        .any(|token| windows_cmd_token_needs_quotes(token));

    (needs_percent || needs_quote).then(|| {
        let mut environment = std::collections::HashMap::new();
        if needs_percent {
            environment.insert(WINDOWS_LITERAL_PERCENT_ENV.to_string(), "%".to_string());
        }
        if needs_quote {
            environment.insert(WINDOWS_LITERAL_QUOTE_ENV.to_string(), "\"".to_string());
        }
        environment
    })
}

fn shell_transport_environment(
    tokens: &[String],
) -> Option<std::collections::HashMap<String, String>> {
    #[cfg(windows)]
    {
        windows_shell_transport_environment(tokens)
    }

    #[cfg(not(windows))]
    {
        let _ = tokens;
        None
    }
}

fn quote_shell_token(token: &str) -> Result<String, &'static str> {
    #[cfg(windows)]
    {
        quote_windows_cmd_token(token)
    }

    #[cfg(not(windows))]
    {
        Ok(shell_escape::unix::escape(Cow::Borrowed(token)).into_owned())
    }
}

fn serialize_shell_command(tokens: &[String]) -> Result<String, &'static str> {
    let quoted = tokens
        .iter()
        .map(|token| quote_shell_token(token))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(quoted.join(" "))
}

/// Execute a Rust/Cargo command and return the result
///
/// If `provided_projects` is not empty, it will be used instead of reading from .meta file.
/// This allows meta_cli to pass in the full project list when --recursive is used.
/// Operational commands fail closed because this legacy entry point cannot
/// negotiate the host's plan execution policy; plugin help remains available.
pub fn execute_command(
    command: &str,
    args: &[String],
    parallel: bool,
    provided_projects: &[String],
    cwd: &Path,
) -> CommandResult {
    execute_command_with_filters(
        command,
        args,
        parallel,
        provided_projects,
        cwd,
        None,
        None,
        &[],
    )
}

/// Execute a Rust/Cargo command after negotiating host execution behavior.
pub fn execute_command_with_host_capabilities(
    command: &str,
    args: &[String],
    parallel: bool,
    provided_projects: &[String],
    cwd: &Path,
    host_capabilities: &[String],
) -> CommandResult {
    execute_command_with_filters(
        command,
        args,
        parallel,
        provided_projects,
        cwd,
        None,
        None,
        host_capabilities,
    )
}

/// Execute a Rust/Cargo command with the host's negotiated behavior and filters.
#[allow(clippy::too_many_arguments)]
pub fn execute_command_with_filters(
    command: &str,
    args: &[String],
    parallel: bool,
    provided_projects: &[String],
    cwd: &Path,
    include_filters: Option<&[String]>,
    exclude_filters: Option<&[String]>,
    host_capabilities: &[String],
) -> CommandResult {
    let mut command_parts = command.split_whitespace();
    let namespace = command_parts.next().unwrap_or_default();
    if !matches!(namespace, "cargo" | "rust") {
        return CommandResult::ShowHelp(Some(format!(
            "unrecognized Rust plugin namespace '{command}'"
        )));
    }

    // Older hosts may send a multi-word matched command. New hosts advertise
    // only the namespace and send every following token in `args`.
    let mut cargo_args: Vec<String> = command_parts.map(str::to_owned).collect();
    cargo_args.extend(args.iter().cloned());

    // Help is Meta-aware and deliberately side-effect-free. Cargo/test-binary
    // payload after `--` is opaque and must never be interpreted here.
    if cargo_args.is_empty() || help_requested(&cargo_args) {
        return CommandResult::ShowHelp(None);
    }

    if !host_capabilities
        .iter()
        .any(|capability| capability == HOST_CAPABILITY_PLAN_EXECUTION_POLICY_V1)
    {
        return CommandResult::Error(format!(
            "Cargo operations require host capability '{HOST_CAPABILITY_PLAN_EXECUTION_POLICY_V1}'"
        ));
    }

    // Get all project directories
    let dirs = match get_project_directories(provided_projects, cwd) {
        Ok(d) => d,
        Err(e) => return CommandResult::Error(format!("Failed to get project directories: {e}")),
    };

    // Apply the host-selected scope before filtering to Rust projects so an
    // include/exclude selection containing no Cargo.toml gets a clear result.
    let selected_dirs = filter_selected_projects(&dirs, include_filters, exclude_filters);
    let rust_dirs = filter_rust_projects(&selected_dirs);

    if rust_dirs.is_empty() {
        return CommandResult::Message("No Rust projects found (no Cargo.toml files)".to_string());
    }

    // Cargo owns subcommand validation, aliases, and installed cargo-* tools.
    // Serialize each argv token independently because loop_lib executes the
    // string plan through the platform shell.
    let mut cargo_tokens = Vec::with_capacity(cargo_args.len() + 1);
    cargo_tokens.push("cargo".to_string());
    cargo_tokens.extend(cargo_args);
    let cargo_cmd = match serialize_shell_command(&cargo_tokens) {
        Ok(command) => command,
        Err(error) => return CommandResult::Error(error.to_string()),
    };
    let cargo_env = shell_transport_environment(&cargo_tokens);

    // Build execution plan
    let commands: Vec<PlannedCommand> = rust_dirs
        .iter()
        .map(|dir| PlannedCommand {
            dir: dir.clone(),
            cmd: cargo_cmd.clone(),
            env: cargo_env.clone(),
        })
        .collect();

    CommandResult::PlanWithPolicy(
        commands,
        Some(parallel),
        PlanExecutionPolicy {
            expand_loop_aliases: false,
            apply_host_filters: false,
        },
    )
}

/// Build the structured runtime help advertised by the plugin.
pub fn plugin_help() -> PluginHelp {
    let mut commands = IndexMap::new();
    commands.insert(
        "cargo".to_string(),
        "Run any Cargo command across selected Rust projects".to_string(),
    );
    commands.insert(
        "rust".to_string(),
        "Alias for the cargo namespace".to_string(),
    );

    PluginHelp {
        usage: "meta [META OPTIONS] cargo <command> [cargo args...]\n       meta [META OPTIONS] rust <command> [cargo args...]".to_string(),
        commands,
        command_sections: IndexMap::new(),
        examples: vec![
            "meta cargo clean".to_string(),
            "meta --dry-run cargo clean --recursive".to_string(),
            "meta cargo check --all-targets".to_string(),
            "meta cargo clippy --all-targets -- -D warnings".to_string(),
            "meta cargo nextest run".to_string(),
        ],
        note: Some(
            "The Rust plugin selects, filters, and deduplicates directories containing Cargo.toml; Cargo validates the command and its arguments. Put Meta controls before cargo/rust. Use `cargo help <command>` for command-specific Cargo options."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_rust_project(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"test-project\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn plan_execution_policy_capability() -> Vec<String> {
        vec![HOST_CAPABILITY_PLAN_EXECUTION_POLICY_V1.to_string()]
    }

    fn execute_capable_command(
        command: &str,
        args: &[String],
        parallel: bool,
        provided_projects: &[String],
        cwd: &Path,
    ) -> CommandResult {
        execute_command_with_host_capabilities(
            command,
            args,
            parallel,
            provided_projects,
            cwd,
            &plan_execution_policy_capability(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_capable_command_with_filters(
        command: &str,
        args: &[String],
        parallel: bool,
        provided_projects: &[String],
        cwd: &Path,
        include_filters: Option<&[String]>,
        exclude_filters: Option<&[String]>,
    ) -> CommandResult {
        execute_command_with_filters(
            command,
            args,
            parallel,
            provided_projects,
            cwd,
            include_filters,
            exclude_filters,
            &plan_execution_policy_capability(),
        )
    }

    fn expect_plan(result: CommandResult) -> (Vec<PlannedCommand>, Option<bool>) {
        match result {
            CommandResult::PlanWithPolicy(commands, parallel, execution_policy) => {
                assert_eq!(
                    execution_policy,
                    PlanExecutionPolicy {
                        expand_loop_aliases: false,
                        apply_host_filters: false,
                    }
                );
                (commands, parallel)
            }
            _ => panic!("Expected Plan result"),
        }
    }

    #[test]
    fn test_non_cargo_namespace_is_rejected() {
        let result = execute_command("carg", &strings(&["clean"]), false, &[], Path::new("."));
        match result {
            CommandResult::ShowHelp(Some(msg)) => assert!(msg.contains("unrecognized")),
            _ => panic!("Expected ShowHelp result"),
        }
    }

    #[test]
    fn test_plugin_help_describes_arbitrary_commands() {
        let help = plugin_help();
        assert!(help.usage.contains("cargo <command>"));
        assert!(help.usage.contains("rust <command>"));
        assert_eq!(
            help.commands.get("rust").map(String::as_str),
            Some("Alias for the cargo namespace")
        );
        for example in ["clean", "check", "clippy", "nextest"] {
            assert!(help.examples.iter().any(|line| line.contains(example)));
        }
        assert!(help.note.as_deref().unwrap().contains("Cargo validates"));
        assert!(!help.note.as_deref().unwrap().contains("meta exec"));
    }

    #[test]
    fn test_bare_namespace_and_help_do_not_discover_projects() {
        let missing = Path::new("/path/that/does/not/exist");
        assert!(matches!(
            execute_command("cargo", &[], false, &[], missing),
            CommandResult::ShowHelp(None)
        ));
        assert!(matches!(
            execute_command("rust", &strings(&["clean", "--help"]), false, &[], missing),
            CommandResult::ShowHelp(None)
        ));
    }

    #[test]
    fn test_operational_commands_require_plan_execution_policy_capability() {
        let temp_dir = TempDir::new().unwrap();
        create_rust_project(temp_dir.path());
        let projects = vec![temp_dir.path().to_string_lossy().into_owned()];

        let result = execute_command(
            "cargo",
            &strings(&["check"]),
            false,
            &projects,
            temp_dir.path(),
        );

        assert!(matches!(
            result,
            CommandResult::Error(message)
                if message.contains(HOST_CAPABILITY_PLAN_EXECUTION_POLICY_V1)
        ));
    }

    #[test]
    fn test_no_rust_projects() {
        let temp_dir = TempDir::new().unwrap();

        // Create .meta with no Rust projects
        std::fs::write(temp_dir.path().join(".meta"), r#"{"projects": {}}"#).unwrap();

        let result =
            execute_capable_command("cargo", &strings(&["build"]), false, &[], temp_dir.path());

        match result {
            CommandResult::Message(msg) => assert!(msg.contains("No Rust projects")),
            _ => panic!("Expected Message result"),
        }
    }

    #[test]
    fn test_arbitrary_cargo_commands_need_no_catalog() {
        let temp_dir = TempDir::new().unwrap();
        create_rust_project(temp_dir.path());
        let projects = vec![temp_dir.path().to_string_lossy().into_owned()];

        for expected in [
            strings(&["cargo", "build", "--release"]),
            strings(&["cargo", "test", "--workspace"]),
            strings(&["cargo", "check"]),
            strings(&["cargo", "clippy", "--all-targets", "--", "-D", "warnings"]),
            strings(&["cargo", "nextest", "run"]),
            strings(&["cargo", "definitely-not-a-built-in"]),
        ] {
            let (commands, parallel) = expect_plan(execute_capable_command(
                "cargo",
                &expected[1..],
                true,
                &projects,
                temp_dir.path(),
            ));
            assert_eq!(commands.len(), 1);
            #[cfg(not(windows))]
            assert_eq!(shell_words::split(&commands[0].cmd).unwrap(), expected);
            #[cfg(windows)]
            assert_eq!(commands[0].cmd, expected.join(" "));
            assert_eq!(parallel, Some(true));
        }
    }

    #[test]
    fn test_rust_alias_produces_canonical_cargo_plan() {
        let temp_dir = TempDir::new().unwrap();
        create_rust_project(temp_dir.path());
        let projects = vec![temp_dir.path().to_string_lossy().into_owned()];
        let args = strings(&["check", "--all-targets"]);

        let cargo = expect_plan(execute_capable_command(
            "cargo",
            &args,
            false,
            &projects,
            temp_dir.path(),
        ));
        let rust = expect_plan(execute_capable_command(
            "rust",
            &args,
            false,
            &projects,
            temp_dir.path(),
        ));

        assert_eq!(cargo.0[0].cmd, "cargo check --all-targets");
        assert_eq!(cargo.0[0].cmd, rust.0[0].cmd);
        assert_eq!(cargo.0[0].dir, rust.0[0].dir);
    }

    #[test]
    fn test_host_projects_are_authoritative_normalized_and_deduplicated() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let child = root.join("child");
        let non_rust = root.join("docs");
        create_rust_project(root);
        create_rust_project(&child);
        std::fs::create_dir_all(&non_rust).unwrap();

        let projects = vec![
            child.join(".").to_string_lossy().into_owned(),
            root.join(".").to_string_lossy().into_owned(),
            root.to_string_lossy().into_owned(),
            root.join("other/../child").to_string_lossy().into_owned(),
            non_rust.to_string_lossy().into_owned(),
        ];
        let (commands, _) = expect_plan(execute_capable_command(
            "cargo",
            &strings(&["clean"]),
            false,
            &projects,
            root,
        ));

        assert_eq!(commands.len(), 2);
        assert_eq!(
            PathBuf::from(&commands[0].dir),
            std::fs::canonicalize(child).unwrap()
        );
        assert_eq!(
            PathBuf::from(&commands[1].dir),
            std::fs::canonicalize(root).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_normalization_resolves_symlink_parent_components_before_lexical_cleanup() {
        use std::os::unix::fs::symlink;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join("root");
        let external = temp_dir.path().join("external");
        let anchor = external.join("anchor");
        let rust_project = external.join("crate");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&anchor).unwrap();
        create_rust_project(&rust_project);
        symlink(&anchor, root.join("link")).unwrap();

        let apparent = root.join("link/../crate").to_string_lossy().into_owned();
        let normalized = normalize_project_directories(std::slice::from_ref(&apparent), &root);

        assert_eq!(normalized.len(), 1);
        assert_eq!(
            PathBuf::from(&normalized[0].execution_path),
            std::fs::canonicalize(rust_project).unwrap()
        );
        assert_eq!(normalized[0].filter_paths, vec![apparent]);
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_alias_filters_survive_canonical_deduplication() {
        use std::os::unix::fs::symlink;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let project = root.join("project");
        let alias = root.join("project-alias");
        create_rust_project(&project);
        symlink(&project, &alias).unwrap();

        // Put the canonical spelling first so the alias would be lost by
        // first-seen canonical deduplication unless match spellings are grouped.
        let projects = vec![
            project.to_string_lossy().into_owned(),
            alias.to_string_lossy().into_owned(),
        ];
        let alias_filter = vec![alias.to_string_lossy().into_owned()];

        let (commands, _) = expect_plan(execute_capable_command_with_filters(
            "cargo",
            &strings(&["check"]),
            false,
            &projects,
            root,
            Some(&alias_filter),
            None,
        ));
        assert_eq!(commands.len(), 1);
        assert_eq!(
            PathBuf::from(&commands[0].dir),
            std::fs::canonicalize(&project).unwrap()
        );

        let excluded = execute_capable_command_with_filters(
            "cargo",
            &strings(&["check"]),
            false,
            &projects,
            root,
            None,
            Some(&alias_filter),
        );
        assert!(matches!(
            excluded,
            CommandResult::Message(message) if message.contains("No Rust projects found")
        ));
    }

    #[test]
    fn test_host_filters_define_scope_before_rust_detection() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let docs = root.join("docs");
        create_rust_project(root);
        std::fs::create_dir_all(&docs).unwrap();
        let projects = vec![
            root.to_string_lossy().into_owned(),
            docs.to_string_lossy().into_owned(),
        ];

        let include_filters = strings(&["docs"]);
        let include_docs = execute_capable_command_with_filters(
            "cargo",
            &strings(&["check"]),
            false,
            &projects,
            root,
            Some(&include_filters),
            None,
        );
        assert!(matches!(
            include_docs,
            CommandResult::Message(message) if message.contains("No Rust projects found")
        ));

        let exclude_filters = vec![root.to_string_lossy().into_owned()];
        let exclude_all = execute_capable_command_with_filters(
            "cargo",
            &strings(&["check"]),
            false,
            &projects,
            root,
            None,
            Some(&exclude_filters),
        );
        assert!(matches!(
            exclude_all,
            CommandResult::Message(message) if message.contains("No Rust projects found")
        ));
    }

    #[test]
    fn test_windows_filter_match_key_equates_verbatim_and_ordinary_paths() {
        assert_eq!(
            windows_filter_match_key(r"\\?\C:\Users\Runner\crate"),
            windows_filter_match_key(r"c:\users\runner\crate")
        );
        assert_eq!(
            windows_filter_match_key(r"\\?\UNC\server\share\crate"),
            windows_filter_match_key(r"\\server\share\crate")
        );
        assert_eq!(
            windows_filter_match_key(r"C:\Users\Runner\crate\"),
            windows_filter_match_key(r"C:\Users\Runner\crate")
        );
        assert!(windows_filter_matches_path(
            r"C:\Users\Runner\crate",
            r"runner\crate\"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_trailing_backslash_filters_select_projects() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let child = root.join("child");
        create_rust_project(root);
        create_rust_project(&child);
        let projects = vec![
            root.to_string_lossy().into_owned(),
            child.to_string_lossy().into_owned(),
        ];
        let child_filter = strings(&["child\\"]);

        let (included, _) = expect_plan(execute_capable_command_with_filters(
            "cargo",
            &strings(&["check"]),
            false,
            &projects,
            root,
            Some(&child_filter),
            None,
        ));
        assert_eq!(included.len(), 1);
        assert_eq!(
            PathBuf::from(&included[0].dir),
            std::fs::canonicalize(&child).unwrap()
        );

        let (excluded, _) = expect_plan(execute_capable_command_with_filters(
            "cargo",
            &strings(&["check"]),
            false,
            &projects,
            root,
            None,
            Some(&child_filter),
        ));
        assert_eq!(excluded.len(), 1);
        assert_eq!(
            PathBuf::from(&excluded[0].dir),
            std::fs::canonicalize(root).unwrap()
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_shell_serialization_preserves_argument_boundaries() {
        let temp_dir = TempDir::new().unwrap();
        create_rust_project(temp_dir.path());
        let projects = vec![temp_dir.path().to_string_lossy().into_owned()];
        let args = strings(&[
            "clippy",
            "--",
            "value with spaces",
            "$(touch injected)",
            "semi;colon",
            "amp&ersand",
            "single'quote",
            "",
        ]);

        let (commands, _) = expect_plan(execute_capable_command(
            "cargo",
            &args,
            false,
            &projects,
            temp_dir.path(),
        ));
        let mut expected = vec!["cargo".to_string()];
        expected.extend(args);

        assert_eq!(shell_words::split(&commands[0].cmd).unwrap(), expected);
    }

    #[test]
    fn test_windows_cmd_serialization_uses_cmd_native_escaping() {
        assert_eq!(quote_windows_cmd_token("plain").unwrap(), "plain");
        assert_eq!(
            quote_windows_cmd_token("amp&ersand").unwrap(),
            "%META_RUST_Q%amp&ersand%META_RUST_Q%"
        );
        assert_eq!(
            quote_windows_cmd_token("pipe|value").unwrap(),
            "%META_RUST_Q%pipe|value%META_RUST_Q%"
        );
        assert_eq!(
            quote_windows_cmd_token("trailing&\\").unwrap(),
            "%META_RUST_Q%trailing&\\\\%META_RUST_Q%"
        );

        // cmd.exe does not use backslash to escape a quote. Doubling keeps the
        // quote in the argument and leaves the following operator quoted.
        assert_eq!(
            quote_windows_cmd_token("quoted\"&echo injected").unwrap(),
            "%META_RUST_Q%quoted%META_RUST_Q%%META_RUST_Q%&echo injected%META_RUST_Q%"
        );

        // Literal percent pairs are reconstructed from one controlled
        // expansion and cannot become arbitrary environment-variable syntax.
        let percent = quote_windows_cmd_token("%PATH%").unwrap();
        assert_eq!(
            percent,
            "%META_RUST_Q%%META_RUST_PCT%PATH%META_RUST_PCT%%META_RUST_Q%"
        );
        assert_ne!(percent, "%META_RUST_Q%%PATH%%META_RUST_Q%");

        let tokens = vec!["cargo".to_string(), "%PATH%".to_string()];
        let environment = windows_shell_transport_environment(&tokens).unwrap();
        assert_eq!(
            environment
                .get(WINDOWS_LITERAL_PERCENT_ENV)
                .map(String::as_str),
            Some("%")
        );
        assert_eq!(
            environment
                .get(WINDOWS_LITERAL_QUOTE_ENV)
                .map(String::as_str),
            Some("\"")
        );

        #[cfg(windows)]
        assert_eq!(
            serialize_shell_command(&["cargo".to_string(), "value with spaces".to_string()])
                .unwrap(),
            "cargo %META_RUST_Q%value with spaces%META_RUST_Q%"
        );
    }

    #[test]
    fn test_windows_cmd_serialization_rejects_line_breaks() {
        for token in ["line\nfeed", "carriage\rreturn", "both\r\n"] {
            assert_eq!(quote_windows_cmd_token(token), Err(WINDOWS_NEWLINE_ERROR));
        }
        assert_eq!(quote_windows_cmd_token("nul\0byte"), Err(WINDOWS_NUL_ERROR));
    }

    #[test]
    fn test_help_and_meta_like_tokens_after_separator_are_cargo_payload() {
        let temp_dir = TempDir::new().unwrap();
        create_rust_project(temp_dir.path());
        let projects = vec![temp_dir.path().to_string_lossy().into_owned()];
        let args = strings(&["test", "--", "--recursive", "--help"]);

        let (commands, _) = expect_plan(execute_capable_command(
            "cargo",
            &args,
            false,
            &projects,
            temp_dir.path(),
        ));

        #[cfg(not(windows))]
        assert_eq!(
            shell_words::split(&commands[0].cmd).unwrap(),
            strings(&["cargo", "test", "--", "--recursive", "--help"])
        );
        #[cfg(windows)]
        assert_eq!(commands[0].cmd, "cargo test -- --recursive --help");
        assert!(commands[0].env.is_none());
    }

    #[test]
    fn test_multi_word_command_shape_remains_compatible() {
        let temp_dir = TempDir::new().unwrap();
        create_rust_project(temp_dir.path());
        let projects = vec![temp_dir.path().to_string_lossy().into_owned()];

        let (commands, _) = expect_plan(execute_capable_command(
            "rust build",
            &strings(&["--release"]),
            false,
            &projects,
            temp_dir.path(),
        ));

        assert_eq!(commands[0].cmd, "cargo build --release");
    }

    #[test]
    fn test_execution_plan_serialization() {
        let commands = vec![PlannedCommand {
            dir: ".".to_string(),
            cmd: "cargo test".to_string(),
            env: None,
        }];
        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands,
            post_commands: vec![],
            parallel: Some(true),
            max_parallel: None,
            spawn_stagger_ms: None,
        };
        let response = PlanResponse {
            plan,
            execution_policy: PlanExecutionPolicy {
                expand_loop_aliases: false,
                apply_host_filters: false,
            },
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"plan\""));
        assert!(json.contains("\"commands\""));
        assert!(json.contains("cargo test"));
        assert!(json.contains("\"expand_loop_aliases\":false"));
        assert!(json.contains("\"apply_host_filters\":false"));
    }
}
