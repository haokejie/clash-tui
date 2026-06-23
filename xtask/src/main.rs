use std::collections::BTreeSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

mod package;
mod package_verify;
mod script_tests;

type XtaskResult<T = ()> = Result<T, String>;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> XtaskResult {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        print_help();
        return Ok(());
    }

    let command = args.remove(0);
    match command.as_str() {
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        "format" => run_command_str("cargo", &["fmt", "--all"]),
        "format-check" => run_command_str("cargo", &["fmt", "--all", "--check"]),
        "clash-tui-check" => run_command_str("cargo", &["check", "-p", "clash-tui"]),
        "clash-tui-test" => run_command_str("cargo", &["test", "-p", "clash-tui"]),
        "workspace-check" => run_command_str("cargo", &["check", "--workspace", "--all-targets"]),
        "workspace-test" => run_command_str("cargo", &["test", "--workspace"]),
        "clippy" => run_command_str("cargo", &["clippy", "--workspace", "--all-targets", "--all-features"]),
        "policy-check" => policy_check(),
        "scripts-check" => scripts_check(),
        "scripts-test" => scripts_test(),
        "ci" => ci(),
        "package" | "clash-tui-package" => package::run(&args),
        "verify-package" | "clash-tui-verify-package" => package_verify::run(&args),
        "tui-remote-smoke" | "clash-tui-remote-smoke" => {
            forward_shell_script("scripts/clash-tui-remote-smoke.sh", &args)
        }
        other => Err(format!("unknown xtask command: {other}\n\n{}", help_text())),
    }
}

fn print_help() {
    println!("{}", help_text());
}

const fn help_text() -> &'static str {
    "usage: cargo xtask <command> [args...]

Commands:
  format                 Run cargo fmt --all
  format-check           Run cargo fmt --all --check
  clash-tui-check         Run cargo check -p clash-tui
  clash-tui-test          Run cargo test -p clash-tui
  workspace-check        Run cargo check --workspace --all-targets
  workspace-test         Run cargo test --workspace
  clippy                 Run cargo clippy --workspace --all-targets --all-features
  policy-check           Run metadata license checks and cargo deny check
  scripts-check          Check shell and Python script syntax
  scripts-test           Run script tests
  ci                     Run the full local CI gate
  package                Build clash-tui package
  verify-package         Verify package archive
  tui-remote-smoke       Run remote TUI smoke script; forwards args to scripts/clash-tui-remote-smoke.sh
"
}

fn ci() -> XtaskResult {
    run_command_str("cargo", &["fmt", "--all", "--check"])?;
    run_command_str("cargo", &["check", "--workspace", "--all-targets"])?;
    policy_check()?;
    run_command_str("cargo", &["clippy", "--workspace", "--all-targets", "--all-features"])?;
    scripts_check()?;
    scripts_test()?;
    run_command_str("cargo", &["test", "--workspace"])
}

pub(crate) fn root_dir() -> XtaskResult<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "failed to resolve workspace root".to_owned())
}

pub(crate) fn run_command_str(program: &str, args: &[&str]) -> XtaskResult {
    let args = args.iter().map(OsString::from).collect::<Vec<_>>();
    run_command(program, &args)
}

pub(crate) fn run_command(program: &str, args: &[OsString]) -> XtaskResult {
    let root = root_dir()?;
    eprintln!("+ {}", display_command(program, args));
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed: {}", display_command(program, args)))
    }
}

pub(crate) fn run_capture(program: &str, args: &[&str]) -> XtaskResult<Vec<u8>> {
    let root = root_dir()?;
    let args = args.iter().map(OsString::from).collect::<Vec<_>>();
    eprintln!("+ {}", display_command(program, &args));
    let output = Command::new(program)
        .args(&args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run {program}: {error}"))?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "command failed: {}\n{}",
            display_command(program, &args),
            stderr.trim()
        ))
    }
}

pub(crate) fn display_command(program: &str, args: &[OsString]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(program.to_owned());
    parts.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
    parts.join(" ")
}

pub(crate) fn strip_separator(args: &[String]) -> &[String] {
    if args.first().is_some_and(|arg| arg == "--") {
        &args[1..]
    } else {
        args
    }
}

fn forward_shell_script(script: &str, args: &[String]) -> XtaskResult {
    let mut command_args = vec![OsString::from(script)];
    command_args.extend(strip_separator(args).iter().map(OsString::from));
    run_command("bash", &command_args)
}

fn scripts_check() -> XtaskResult {
    for script in script_files("sh", None)? {
        run_command("bash", &[OsString::from("-n"), script.into_os_string()])?;
    }
    for script in packaging_shell_scripts()? {
        run_command("bash", &[OsString::from("-n"), script.into_os_string()])?;
    }
    for script in script_files("py", None)? {
        run_command(
            "python3",
            &[
                OsString::from("-m"),
                OsString::from("py_compile"),
                script.into_os_string(),
            ],
        )?;
    }
    Ok(())
}

fn scripts_test() -> XtaskResult {
    run_command_str("cargo", &["test", "-p", "xtask"])?;
    run_command_str(
        "python3",
        &["-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"],
    )
}

fn script_files(extension: &str, prefix: Option<&str>) -> XtaskResult<Vec<PathBuf>> {
    let scripts_dir = root_dir()?.join("scripts");
    let mut files = Vec::new();
    for entry in
        fs::read_dir(&scripts_dir).map_err(|error| format!("failed to read {}: {error}", scripts_dir.display()))?
    {
        let path = entry
            .map_err(|error| format!("failed to read scripts entry: {error}"))?
            .path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(OsStr::to_str) != Some(extension) {
            continue;
        }
        if let Some(required_prefix) = prefix {
            let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if !file_name.starts_with(required_prefix) {
                continue;
            }
        }
        files.push(path);
    }
    files.sort();
    Ok(files)
}

fn packaging_shell_scripts() -> XtaskResult<Vec<PathBuf>> {
    let root = root_dir()?;
    let candidates = [
        root.join("packaging").join("install.sh"),
        root.join("packaging").join("clash-tui").join("install.sh"),
    ];
    let mut files = Vec::new();
    for candidate in candidates {
        if candidate.is_file() {
            files.push(candidate);
        }
    }
    Ok(files)
}

fn policy_check() -> XtaskResult {
    check_metadata_licenses()?;
    check_cargo_deny()
}

fn check_metadata_licenses() -> XtaskResult {
    let metadata = run_capture("cargo", &["metadata", "--format-version=1"])?;
    let metadata: Value =
        serde_json::from_slice(&metadata).map_err(|error| format!("failed to parse cargo metadata JSON: {error}"))?;
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata JSON is missing packages array".to_owned())?;
    let allowed_licenses = allowed_licenses()?;
    let mut missing_license = Vec::new();
    let mut disallowed_atoms = Vec::<(String, String)>::new();
    let mut expressions = BTreeSet::new();

    for package in packages {
        let package_id = package_id(package);
        let Some(license) = package.get("license").and_then(Value::as_str) else {
            missing_license.push(package_id);
            continue;
        };
        expressions.insert(license.to_owned());
        for atom in license_atoms(license, &allowed_licenses) {
            if !allowed_licenses.contains(&atom) {
                disallowed_atoms.push((atom, package_id.clone()));
            }
        }
    }

    if !missing_license.is_empty() {
        return Err(format!(
            "packages missing license metadata:\n{}",
            missing_license.join("\n")
        ));
    }
    if !disallowed_atoms.is_empty() {
        let details = disallowed_atoms
            .into_iter()
            .map(|(license, package)| format!("{license}: {package}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("license atoms not allowed by deny.toml:\n{details}"));
    }

    println!(
        "supply-chain check: metadata licenses ok ({} packages, {} expressions)",
        packages.len(),
        expressions.len()
    );
    Ok(())
}

fn package_id(package: &Value) -> String {
    let name = package.get("name").and_then(Value::as_str).unwrap_or("<unknown>");
    let version = package.get("version").and_then(Value::as_str).unwrap_or("<unknown>");
    format!("{name}@{version}")
}

fn allowed_licenses() -> XtaskResult<BTreeSet<String>> {
    let deny_toml_path = root_dir()?.join("deny.toml");
    let deny_toml = fs::read_to_string(&deny_toml_path)
        .map_err(|error| format!("failed to read {}: {error}", deny_toml_path.display()))?;
    extract_allowed_licenses(&deny_toml)
}

fn extract_allowed_licenses(config: &str) -> XtaskResult<BTreeSet<String>> {
    let marker = "[licenses]";
    let section_start = config
        .find(marker)
        .ok_or_else(|| "deny.toml is missing [licenses]".to_owned())?
        + marker.len();
    let rest = &config[section_start..];
    let section_end = rest.find("\n[").unwrap_or(rest.len());
    let section = &rest[..section_end];
    let allow_start = section
        .find("allow")
        .ok_or_else(|| "deny.toml [licenses] is missing allow list".to_owned())?;
    let list_start = section[allow_start..]
        .find('[')
        .ok_or_else(|| "deny.toml [licenses].allow is missing opening bracket".to_owned())?
        + allow_start
        + 1;
    let list_rest = &section[list_start..];
    let list_end = list_rest
        .find(']')
        .ok_or_else(|| "deny.toml [licenses].allow is missing closing bracket".to_owned())?;
    let allowed = quoted_strings(&list_rest[..list_end]);
    if allowed.is_empty() {
        Err("deny.toml [licenses].allow is empty".to_owned())
    } else {
        Ok(allowed)
    }
}

fn quoted_strings(input: &str) -> BTreeSet<String> {
    let mut output = BTreeSet::new();
    let mut current = String::new();
    let mut in_quote = false;

    for character in input.chars() {
        if character == '"' {
            if in_quote {
                output.insert(current.clone());
                current.clear();
            }
            in_quote = !in_quote;
        } else if in_quote {
            current.push(character);
        }
    }

    output
}

fn license_atoms(expression: &str, allowed_licenses: &BTreeSet<String>) -> BTreeSet<String> {
    let mut atoms = BTreeSet::new();
    let mut remaining = expression.to_owned();
    let mut allowed = allowed_licenses.iter().collect::<Vec<_>>();
    allowed.sort_by_key(|license| std::cmp::Reverse(license.len()));

    for allowed_license in allowed {
        let mut search_start = 0;
        while let Some(relative_index) = remaining[search_start..].find(allowed_license) {
            let start = search_start + relative_index;
            let end = start + allowed_license.len();
            if is_license_boundary(&remaining, start) && is_license_boundary(&remaining, end) {
                atoms.insert(allowed_license.to_owned());
                remaining.replace_range(start..end, &" ".repeat(allowed_license.len()));
                search_start = end;
            } else {
                search_start = start + 1;
            }
        }
    }

    for token in remaining
        .split(|character: char| character.is_whitespace() || character == '(' || character == ')' || character == '/')
    {
        if token.is_empty() || token == "AND" || token == "OR" || token == "WITH" {
            continue;
        }
        atoms.insert(token.to_owned());
    }

    atoms
}

fn is_license_boundary(input: &str, index: usize) -> bool {
    if index == 0 || index >= input.len() {
        return true;
    }
    let previous = input[..index].chars().next_back();
    let next = input[index..].chars().next();
    previous.is_some_and(is_boundary_char) || next.is_some_and(is_boundary_char)
}

const fn is_boundary_char(character: char) -> bool {
    character.is_whitespace() || character == '(' || character == ')' || character == '/'
}

fn check_cargo_deny() -> XtaskResult {
    let root = root_dir()?;
    let version = Command::new("cargo")
        .args(["deny", "--version"])
        .current_dir(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let require_cargo_deny = env::var("CLASH_TUI_REQUIRE_CARGO_DENY").is_ok_and(|value| value == "1");

    match version {
        Ok(output) if output.status.success() => {
            println!("supply-chain check: {}", String::from_utf8_lossy(&output.stdout).trim());
            run_command_str("cargo", &["deny", "check"])
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            handle_missing_cargo_deny(require_cargo_deny, stderr.trim())
        }
        Err(error) => handle_missing_cargo_deny(require_cargo_deny, &error.to_string()),
    }
}

fn handle_missing_cargo_deny(require_cargo_deny: bool, detail: &str) -> XtaskResult {
    let message = if detail.is_empty() {
        "supply-chain check: cargo-deny is not installed".to_owned()
    } else {
        format!("supply-chain check: cargo-deny is not installed ({detail})")
    };
    if require_cargo_deny {
        Err(format!(
            "{message}; install cargo-deny or unset CLASH_TUI_REQUIRE_CARGO_DENY"
        ))
    } else {
        eprintln!("{message}; skipping cargo deny check");
        Ok(())
    }
}
