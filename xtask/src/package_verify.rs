#![allow(
    clippy::branches_sharing_code,
    clippy::cognitive_complexity,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::{XtaskResult, root_dir, run_command, strip_separator};

const REQUIRED_PACKAGE_FILES: &[&str] = &[
    "README.md",
    "env.example",
    "install.sh",
    "tools/clash-tui-system-proxy-gnome-acceptance.sh",
    "tools/clash-tui-system-proxy-gnome-smoke.py",
    "tools/clash-tui-tun-linux-smoke.py",
    "systemd/clash-tui.service",
];

const REQUIRED_EXECUTABLE_FILES: &[&str] = &[
    "clash-tui",
    "resources/mihomo",
    "install.sh",
    "tools/clash-tui-system-proxy-gnome-acceptance.sh",
    "tools/clash-tui-system-proxy-gnome-smoke.py",
    "tools/clash-tui-tun-linux-smoke.py",
];

const REQUIRED_BOOTSTRAP_MARKERS: &[&str] = &[
    "CLASH_TUI_INSTALL_BASE_URL",
    "--base-url",
    "verify_archive",
    "delegating to package installer",
];

const REQUIRED_TEXT_MARKERS: &[(&str, &[&str])] = &[
    (
        "tools/clash-tui-system-proxy-gnome-acceptance.sh",
        &[
            "--verify-acceptance-dir",
            "gnome-acceptance-verified.json",
            "--archive",
            "gnome-acceptance-SHA256SUMS.txt",
            "CLASH_TUI_SYSTEM_PROXY_SMOKE=1",
        ],
    ),
    (
        "tools/clash-tui-system-proxy-gnome-smoke.py",
        &[
            "--preflight",
            "--output",
            "--verify-report",
            "--verify-acceptance-dir",
            "schemaVersion",
            "desktopSession",
            "reportHashes",
            "resolvedPath",
        ],
    ),
    (
        "tools/clash-tui-tun-linux-smoke.py",
        &["--preflight", "--output", "CLASH_TUI_TUN_SMOKE"],
    ),
];

#[derive(Debug, Default)]
struct Options {
    archive: PathBuf,
    manifest: Option<PathBuf>,
    bootstrap: Option<PathBuf>,
    expect_commit: Option<String>,
    require_clean_source: bool,
    json: bool,
}

struct Report {
    ok: bool,
    archive: Value,
    manifest: Value,
    checks: Vec<Value>,
}

pub(crate) fn run(args: &[String]) -> XtaskResult {
    let options = parse_args(strip_separator(args))?;
    let report = verify_package(&options)?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_value(&report))
                .map_err(|error| format!("failed to serialize report: {error}"))?
        );
    } else {
        print_human(&report);
    }
    if report.ok {
        Ok(())
    } else {
        Err("clash-tui package verify failed".to_owned())
    }
}

fn usage() -> &'static str {
    "usage: cargo xtask verify-package --archive <tar.gz> [options]

Options:
  --archive <file>       Package tar.gz to verify.
  --manifest <file>      Sidecar manifest JSON. Defaults to <package>.manifest.json when present.
  --bootstrap <file>     Online install bootstrap. Defaults to install.sh next to the archive.
  --expect-commit <sha>  Require manifest gitCommit to match.
  --require-clean-source Require manifest gitDirty=false.
  --json                 Print machine-readable report.
  -h, --help             Show this help.
"
}

fn parse_args(args: &[String]) -> XtaskResult<Options> {
    let root = root_dir()?;
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let read_value = |index: &mut usize| -> XtaskResult<String> {
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| format!("{arg} requires a value"))
        };
        match arg.as_str() {
            "--archive" => options.archive = resolve_path(&root, &read_value(&mut index)?),
            "--manifest" => options.manifest = Some(resolve_path(&root, &read_value(&mut index)?)),
            "--bootstrap" => options.bootstrap = Some(resolve_path(&root, &read_value(&mut index)?)),
            "--expect-commit" => options.expect_commit = Some(read_value(&mut index)?),
            "--require-clean-source" => options.require_clean_source = true,
            "--json" => options.json = true,
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown option: {unknown}")),
        }
        index += 1;
    }
    if options.archive.as_os_str().is_empty() {
        return Err("--archive is required".to_owned());
    }
    Ok(options)
}

fn resolve_path(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() { path } else { root.join(path) }
}

fn verify_package(options: &Options) -> XtaskResult<Report> {
    let archive_path = options.archive.clone();
    let mut report = Report {
        ok: false,
        archive: json!({
            "path": archive_path.display().to_string(),
            "file": archive_path.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
            "sha256": "",
        }),
        manifest: json!({}),
        checks: Vec::new(),
    };

    add_check(
        &mut report.checks,
        archive_path.is_file(),
        "archive-exists",
        "archive file exists",
        None,
    );
    if !last_check_ok(&report.checks) {
        return Ok(report);
    }

    let tmp_dir = temp_dir("clash-tui-package-verify");
    fs::create_dir_all(&tmp_dir).map_err(|error| format!("failed to create {}: {error}", tmp_dir.display()))?;
    let result = verify_package_inner(options, &archive_path, &tmp_dir, &mut report);
    let _ = fs::remove_dir_all(&tmp_dir);
    result?;
    report.ok = report.checks.iter().all(check_ok);
    Ok(report)
}

fn verify_package_inner(options: &Options, archive_path: &Path, tmp_dir: &Path, report: &mut Report) -> XtaskResult {
    let archive_sha256 = sha256(archive_path)?;
    report.archive["sha256"] = json!(archive_sha256);

    run_command(
        "tar",
        &[
            OsString::from("-xzf"),
            archive_path.as_os_str().to_owned(),
            OsString::from("-C"),
            tmp_dir.as_os_str().to_owned(),
        ],
    )?;
    let package_dir = find_package_dir(tmp_dir)?;
    let internal_manifest_path = package_dir.join("manifest.json");
    add_check(
        &mut report.checks,
        internal_manifest_path.is_file(),
        "internal-manifest-exists",
        "archive contains manifest.json",
        None,
    );
    if !last_check_ok(&report.checks) {
        return Ok(());
    }

    let internal_manifest = read_json(&internal_manifest_path)?;
    report.manifest = json!({
        "packageName": internal_manifest.get("packageName").and_then(Value::as_str),
        "gitCommit": internal_manifest.get("gitCommit").and_then(Value::as_str),
        "gitDirty": internal_manifest.get("gitDirty").cloned().unwrap_or(Value::Null),
        "target": internal_manifest.get("target").and_then(Value::as_str),
        "dockerPlatform": internal_manifest.get("dockerPlatform").and_then(Value::as_str),
        "internalManifestHasArchive": internal_manifest.get("archive").is_some(),
    });

    if let Some(expect_commit) = &options.expect_commit {
        let actual = internal_manifest
            .get("gitCommit")
            .and_then(Value::as_str)
            .unwrap_or_default();
        add_check(
            &mut report.checks,
            actual == expect_commit,
            "git-commit",
            "manifest gitCommit matches expected commit",
            Some(json!({ "expected": expect_commit, "actual": actual })),
        );
    }
    if options.require_clean_source {
        let actual = internal_manifest.get("gitDirty").cloned().unwrap_or(Value::Null);
        add_check(
            &mut report.checks,
            actual == Value::Bool(false),
            "git-clean-source",
            "manifest gitDirty is false",
            Some(json!({ "actual": actual })),
        );
    }

    verify_sidecar_manifest(options, archive_path, &internal_manifest, &archive_sha256, report)?;
    verify_bootstrap(options, archive_path, report)?;
    verify_package_files(&package_dir, &internal_manifest, report)?;
    verify_hashes(&package_dir, &internal_manifest, report)?;
    Ok(())
}

fn verify_sidecar_manifest(
    options: &Options,
    archive_path: &Path,
    internal_manifest: &Value,
    archive_sha256: &str,
    report: &mut Report,
) -> XtaskResult {
    let package_name = internal_manifest
        .get("packageName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sidecar_manifest_path = find_sidecar_manifest(options, archive_path, package_name);
    report.archive["sidecarManifest"] = sidecar_manifest_path
        .as_ref()
        .map(|path| json!(path.display().to_string()))
        .unwrap_or(Value::Null);
    let Some(sidecar_manifest_path) = sidecar_manifest_path else {
        add_check(
            &mut report.checks,
            false,
            "sidecar-manifest",
            "sidecar manifest is required to verify archive metadata",
            None,
        );
        return Ok(());
    };

    let sidecar_manifest = read_json(&sidecar_manifest_path)?;
    let sidecar_archive = sidecar_manifest.get("archive").unwrap_or(&Value::Null);
    let sidecar_sha = sidecar_archive
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default();
    add_check(
        &mut report.checks,
        sidecar_sha == archive_sha256,
        "sidecar-archive-sha",
        "sidecar manifest archive SHA matches tar.gz",
        Some(json!({ "expected": archive_sha256, "actual": sidecar_sha })),
    );
    let archive_file = archive_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let sidecar_file = sidecar_archive.get("file").and_then(Value::as_str).unwrap_or_default();
    add_check(
        &mut report.checks,
        sidecar_file.is_empty() || sidecar_file == archive_file,
        "sidecar-archive-file",
        "sidecar manifest archive file matches tar.gz basename",
        Some(
            json!({ "expected": archive_file, "actual": if sidecar_file.is_empty() { Value::Null } else { json!(sidecar_file) } }),
        ),
    );
    add_check(
        &mut report.checks,
        clone_without_archive(&sidecar_manifest) == clone_without_archive(internal_manifest),
        "sidecar-internal-manifest",
        "sidecar manifest matches archive manifest except archive metadata",
        None,
    );
    if let Some(internal_archive) = internal_manifest.get("archive") {
        let internal_sha = internal_archive
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        add_check(
            &mut report.checks,
            internal_sha == archive_sha256,
            "internal-archive-sha",
            "internal manifest archive SHA matches tar.gz when present",
            Some(
                json!({ "expected": archive_sha256, "actual": if internal_sha.is_empty() { Value::Null } else { json!(internal_sha) } }),
            ),
        );
    }
    Ok(())
}

fn verify_bootstrap(options: &Options, archive_path: &Path, report: &mut Report) -> XtaskResult {
    let bootstrap_path = find_bootstrap_installer(options, archive_path);
    report.archive["bootstrapInstaller"] = bootstrap_path
        .as_ref()
        .map(|path| json!(path.display().to_string()))
        .unwrap_or(Value::Null);
    let has_bootstrap = bootstrap_path.as_ref().is_some_and(|path| path.is_file());
    add_check(
        &mut report.checks,
        has_bootstrap,
        "bootstrap-installer",
        "online bootstrap install.sh exists next to archive or via --bootstrap",
        None,
    );
    let Some(bootstrap_path) = bootstrap_path.filter(|path| path.is_file()) else {
        return Ok(());
    };
    add_check(
        &mut report.checks,
        file_is_executable(&bootstrap_path),
        "bootstrap-installer-executable",
        "online bootstrap install.sh is executable",
        None,
    );
    for marker in REQUIRED_BOOTSTRAP_MARKERS {
        add_check(
            &mut report.checks,
            file_contains_marker(&bootstrap_path, marker)?,
            &format!("bootstrap-installer-marker:{marker}"),
            &format!("online bootstrap install.sh contains required marker {marker}"),
            None,
        );
    }
    Ok(())
}

fn verify_package_files(package_dir: &Path, internal_manifest: &Value, report: &mut Report) -> XtaskResult {
    let package_files = internal_manifest
        .get("packageFiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for file in REQUIRED_PACKAGE_FILES {
        add_check(
            &mut report.checks,
            package_files.iter().any(|value| value.as_str() == Some(file)),
            &format!("package-files:{file}"),
            &format!("manifest packageFiles includes {file}"),
            None,
        );
        add_check(
            &mut report.checks,
            package_dir.join(file).is_file(),
            &format!("file-exists:{file}"),
            &format!("{file} exists in archive"),
            None,
        );
    }

    for file in REQUIRED_EXECUTABLE_FILES {
        add_check(
            &mut report.checks,
            file_is_executable(&package_dir.join(file)),
            &format!("file-executable:{file}"),
            &format!("{file} is executable"),
            None,
        );
    }

    for (file, markers) in REQUIRED_TEXT_MARKERS {
        for marker in *markers {
            add_check(
                &mut report.checks,
                file_contains_marker(&package_dir.join(file), marker)?,
                &format!("file-marker:{file}:{marker}"),
                &format!("{file} contains required marker {marker}"),
                None,
            );
        }
    }
    Ok(())
}

fn verify_hashes(package_dir: &Path, internal_manifest: &Value, report: &mut Report) -> XtaskResult {
    let clash_tui = internal_manifest.get("clashTui").unwrap_or(&Value::Null);
    let clash_tui_binary = clash_tui.get("binary").and_then(Value::as_str).unwrap_or("clash-tui");
    let clash_tui_expected = clash_tui.get("sha256").and_then(Value::as_str).unwrap_or_default();
    add_check(
        &mut report.checks,
        clash_tui_expected == sha256(&package_dir.join(clash_tui_binary))?,
        "clash-tui-sha",
        "clash-tui binary SHA matches manifest",
        Some(
            json!({ "expected": if clash_tui_expected.is_empty() { Value::Null } else { json!(clash_tui_expected) } }),
        ),
    );

    let mihomo = internal_manifest.get("mihomo").unwrap_or(&Value::Null);
    let mihomo_binary = mihomo
        .get("binary")
        .and_then(Value::as_str)
        .unwrap_or("resources/mihomo");
    let mihomo_expected = mihomo.get("sha256").and_then(Value::as_str).unwrap_or_default();
    add_check(
        &mut report.checks,
        mihomo_expected == sha256(&package_dir.join(mihomo_binary))?,
        "mihomo-sha",
        "mihomo binary SHA matches manifest",
        Some(json!({ "expected": if mihomo_expected.is_empty() { Value::Null } else { json!(mihomo_expected) } })),
    );

    for resource in internal_manifest
        .get("geoResources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let file = resource.get("file").and_then(Value::as_str).unwrap_or_default();
        let expected = resource.get("sha256").and_then(Value::as_str).unwrap_or_default();
        add_check(
            &mut report.checks,
            expected == sha256(&package_dir.join("resources").join(file))?,
            &format!("geo-sha:{file}"),
            &format!("{file} SHA matches manifest"),
            Some(json!({ "expected": expected })),
        );
    }
    Ok(())
}

fn find_package_dir(extract_dir: &Path) -> XtaskResult<PathBuf> {
    let dirs = fs::read_dir(extract_dir)
        .map_err(|error| format!("failed to read {}: {error}", extract_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    if dirs.len() == 1 {
        Ok(dirs[0].clone())
    } else {
        Err(format!(
            "expected one package directory in archive, found {}",
            dirs.len()
        ))
    }
}

fn find_sidecar_manifest(options: &Options, archive_path: &Path, package_name: &str) -> Option<PathBuf> {
    if let Some(manifest) = &options.manifest {
        return Some(manifest.clone());
    }
    let archive_dir = archive_path.parent()?;
    let exact = archive_dir.join(format!("{package_name}.manifest.json"));
    if exact.is_file() {
        return Some(exact);
    }
    let generic = archive_dir.join("manifest.json");
    if generic.is_file() {
        return Some(generic);
    }
    None
}

fn find_bootstrap_installer(options: &Options, archive_path: &Path) -> Option<PathBuf> {
    if let Some(bootstrap) = &options.bootstrap {
        return Some(bootstrap.clone());
    }
    let candidate = archive_path.parent()?.join("install.sh");
    candidate.is_file().then_some(candidate)
}

fn read_json(file: &Path) -> XtaskResult<Value> {
    let content = fs::read_to_string(file).map_err(|error| format!("failed to read {}: {error}", file.display()))?;
    serde_json::from_str(&content).map_err(|error| format!("failed to parse {}: {error}", file.display()))
}

fn clone_without_archive(value: &Value) -> Value {
    let mut cloned = value.clone();
    if let Some(object) = cloned.as_object_mut() {
        object.remove("archive");
    }
    cloned
}

fn add_check(checks: &mut Vec<Value>, ok: bool, code: &str, message: &str, details: Option<Value>) {
    let mut check = json!({
        "ok": ok,
        "code": code,
        "message": message,
    });
    if let Some(details) = details {
        check["details"] = details;
    }
    checks.push(check);
}

fn last_check_ok(checks: &[Value]) -> bool {
    checks
        .last()
        .and_then(|check| check.get("ok"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn check_ok(check: &Value) -> bool {
    check.get("ok").and_then(Value::as_bool).unwrap_or(false)
}

fn report_value(report: &Report) -> Value {
    json!({
        "ok": report.ok,
        "archive": report.archive,
        "manifest": report.manifest,
        "checks": report.checks,
    })
}

fn print_human(report: &Report) {
    println!("clash-tui package verify: {}", if report.ok { "ok" } else { "failed" });
    println!(
        "archive: {}",
        report.archive.get("file").and_then(Value::as_str).unwrap_or_default()
    );
    if let Some(sha) = report
        .archive
        .get("sha256")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        println!("archive sha256: {sha}");
    }
    if let Some(package) = report.manifest.get("packageName").and_then(Value::as_str) {
        println!("package: {package}");
        println!(
            "git commit: {}",
            report
                .manifest
                .get("gitCommit")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
        println!("git dirty: {}", report.manifest.get("gitDirty").unwrap_or(&Value::Null));
    }
    if let Some(sidecar) = report.archive.get("sidecarManifest").and_then(Value::as_str) {
        println!("sidecar manifest: {sidecar}");
    }
    if let Some(bootstrap) = report.archive.get("bootstrapInstaller").and_then(Value::as_str) {
        println!("online bootstrap: {bootstrap}");
    }
    for check in &report.checks {
        if !check_ok(check) {
            eprintln!(
                "FAIL {}: {}",
                check.get("code").and_then(Value::as_str).unwrap_or_default(),
                check.get("message").and_then(Value::as_str).unwrap_or_default()
            );
        }
    }
}

fn file_is_executable(file: &Path) -> bool {
    let metadata = match fs::metadata(file) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn file_contains_marker(file: &Path, marker: &str) -> XtaskResult<bool> {
    if !file.is_file() {
        return Ok(false);
    }
    let content = fs::read_to_string(file).map_err(|error| format!("failed to read {}: {error}", file.display()))?;
    Ok(content.contains(marker))
}

fn sha256(file: &Path) -> XtaskResult<String> {
    for (program, args) in [
        ("sha256sum", vec![file.as_os_str().to_owned()]),
        (
            "shasum",
            vec![OsString::from("-a"), OsString::from("256"), file.as_os_str().to_owned()],
        ),
    ] {
        let output = Command::new(program)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let Ok(output) = output else {
            continue;
        };
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(hash) = text.split_whitespace().next() {
                return Ok(hash.to_owned());
            }
        }
    }
    Err(format!("failed to calculate sha256 for {}", file.display()))
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{temp_dir as package_temp_dir, write_executable};

    #[test]
    fn file_executable_detects_script_mode() {
        let tmp_dir = package_temp_dir("clash-tui-verify-executable");
        let script = tmp_dir.join("script.sh");
        write_executable(&script, "#!/usr/bin/env sh\nexit 0\n");
        assert!(file_is_executable(&script));
        let _ = fs::remove_dir_all(tmp_dir);
    }
}
