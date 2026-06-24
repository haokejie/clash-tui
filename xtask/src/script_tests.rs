#![cfg(test)]
#![allow(
    clippy::expect_used,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::package::{temp_dir, write_executable};
use crate::{package, package_verify, root_dir};

const REAL_PACKAGE_NAME: &str = "clash-tui-linux-x86_64";

struct CommandOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

fn run_local(program: &str, args: &[OsString], cwd: &Path, envs: &[(&str, String)]) -> CommandOutput {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output().expect("run command");
    CommandOutput {
        status: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn write_file(file: &Path, content: &str) {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(file, content).expect("write file");
}

fn sha256(file: &Path) -> Result<String, String> {
    for (program, args) in [
        ("sha256sum", vec![file.as_os_str().to_owned()]),
        (
            "shasum",
            vec![OsString::from("-a"), OsString::from("256"), file.as_os_str().to_owned()],
        ),
    ] {
        match Command::new(program).args(args).stdout(Stdio::piped()).output() {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Some(hash) = text.split_whitespace().next().filter(|hash| !hash.is_empty()) {
                    return Ok(hash.to_owned());
                }
            }
            _ => {}
        }
    }
    Err("sha256 command unavailable".to_owned())
}

fn marker_regex_literal(marker: &str) -> String {
    marker.to_owned()
}

#[test]
fn dependabot_covers_cargo_and_github_actions_updates() {
    let root = root_dir().expect("root");
    let content = fs::read_to_string(root.join(".github/dependabot.yml")).expect("read dependabot");
    for marker in [
        "version: 2",
        "package-ecosystem: cargo",
        "package-ecosystem: github-actions",
        "interval: weekly",
        "open-pull-requests-limit: 5",
        "cargo-minor-and-patch",
        "github-actions:",
    ] {
        assert!(content.contains(marker), "missing {marker}");
    }
}

#[test]
fn remote_tui_smoke_script_keeps_required_acceptance_safeguards() {
    let root = root_dir().expect("root");
    let script = root.join("scripts/clash-tui-remote-smoke.sh");
    let syntax = run_local(
        "bash",
        &[OsString::from("-n"), script.as_os_str().to_owned()],
        &root,
        &[],
    );
    assert_eq!(syntax.status, 0, "{}", syntax.stderr);
    let source = fs::read_to_string(script).expect("read smoke script");
    for marker in [
        "ssh -tt",
        "TERM=xterm-256color",
        "stty rows \"$ROWS\" cols \"$COLS\"",
        "CLASH_TUI_TUI_INPUT_TRACE",
        "TRACE_BEGIN",
        "TRACE_END",
        "log_user 0",
        "运行概览",
        "代理选择",
        "快速开关",
        "模式切换",
        "key code=char",
        "key code=esc",
        "target/clash-tui-acceptance/remote-smoke/$RUN_ID/report.json",
    ] {
        assert!(
            source.contains(&marker_regex_literal(marker)),
            "missing marker {marker}"
        );
    }
}

#[test]
fn online_installer_verifies_archive_and_delegates_args_to_package_install_sh() {
    let root = root_dir().expect("root");
    let fixture = create_online_install_artifacts(false);
    let result = run_online_install(
        &root,
        &fixture.0,
        &fixture.1,
        &["--", "--prefix", "/opt/example", "--no-start"],
    );
    assert_eq!(result.status, 0, "{}\n{}", result.stdout, result.stderr);
    assert!(result.stdout.contains("archive verified"));
    assert_eq!(
        fs::read_to_string(&fixture.1).expect("read marker"),
        "--prefix\n/opt/example\n--no-start\n"
    );
    let _ = fs::remove_dir_all(fixture.2);
}

#[test]
fn online_installer_rejects_archive_when_sha256_mismatches() {
    let root = root_dir().expect("root");
    let fixture = create_online_install_artifacts(true);
    let result = run_online_install(&root, &fixture.0, &fixture.1, &[]);
    assert_ne!(result.status, 0);
    assert!(result.stderr.contains("archive sha256 mismatch"));
    assert!(!fixture.1.exists());
    let _ = fs::remove_dir_all(fixture.2);
}

fn create_online_install_artifacts(corrupt_sha: bool) -> (PathBuf, PathBuf, PathBuf) {
    let tmp_dir = temp_dir("clash-tui-online-install-test");
    let package_dir = tmp_dir.join(REAL_PACKAGE_NAME);
    let artifacts_dir = tmp_dir.join("artifacts");
    let marker_path = tmp_dir.join("installer-args.txt");
    fs::create_dir_all(&artifacts_dir).expect("create artifacts");
    write_executable(
        &package_dir.join("install.sh"),
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf \"%s\\n\" \"$@\" > \"$CLASH_TUI_ONLINE_INSTALL_MARKER\"\n",
    );
    write_file(&package_dir.join("manifest.json"), "{\"packageName\":\"fixture\"}\n");
    let archive = artifacts_dir.join(format!("{REAL_PACKAGE_NAME}.tar.gz"));
    let tar = run_local(
        "tar",
        &[
            OsString::from("-czf"),
            archive.as_os_str().to_owned(),
            OsString::from("-C"),
            tmp_dir.as_os_str().to_owned(),
            OsString::from(REAL_PACKAGE_NAME),
        ],
        &tmp_dir,
        &[],
    );
    assert_eq!(tar.status, 0, "{}", tar.stderr);
    let archive_sha = sha256(&archive).expect("archive sha256");
    let sha_text = if corrupt_sha {
        format!(
            "{}  {}\n",
            "0".repeat(64),
            archive.file_name().unwrap().to_string_lossy()
        )
    } else {
        format!("{archive_sha}  {}\n", archive.file_name().unwrap().to_string_lossy())
    };
    write_file(&PathBuf::from(format!("{}.sha256", archive.display())), &sha_text);
    write_file(
        &artifacts_dir.join(format!("{REAL_PACKAGE_NAME}.manifest.json")),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "packageName": REAL_PACKAGE_NAME,
                "archive": { "file": archive.file_name().unwrap().to_string_lossy(), "sha256": archive_sha },
            }))
            .expect("manifest")
        ),
    );
    (artifacts_dir, marker_path, tmp_dir)
}

fn run_online_install(root: &Path, artifacts_dir: &Path, marker_path: &Path, extra_args: &[&str]) -> CommandOutput {
    let mut args = vec![
        root.join("packaging/install.sh").into_os_string(),
        OsString::from("--base-url"),
        OsString::from(format!("file://{}", artifacts_dir.display())),
        OsString::from("--target"),
        OsString::from("x86_64"),
        OsString::from("--no-sudo"),
    ];
    args.extend(extra_args.iter().map(OsString::from));
    run_local(
        "bash",
        &args,
        root,
        &[("CLASH_TUI_ONLINE_INSTALL_MARKER", marker_path.display().to_string())],
    )
}

#[test]
fn clash_tui_package_emits_online_bootstrap_and_omits_sidecar_install_script() {
    let tmp_dir = temp_dir("clash-tui-package-artifacts-test");
    let binary = tmp_dir.join("clash-tui");
    let mihomo = tmp_dir.join("mihomo");
    let geo_dir = tmp_dir.join("geo");
    let out_dir = tmp_dir.join("out");
    write_executable(&binary, "#!/usr/bin/env sh\nexit 0\n");
    write_executable(&mihomo, "#!/usr/bin/env sh\nexit 0\n");
    write_file(&geo_dir.join("Country.mmdb"), "country\n");
    write_file(&geo_dir.join("geosite.dat"), "geosite\n");
    write_file(&geo_dir.join("geoip.dat"), "geoip\n");

    package::run(&[
        "--no-docker".to_owned(),
        "--skip-build".to_owned(),
        "--skip-archive".to_owned(),
        "--binary".to_owned(),
        binary.display().to_string(),
        "--mihomo-bin".to_owned(),
        mihomo.display().to_string(),
        "--geo-dir".to_owned(),
        geo_dir.display().to_string(),
        "--out-dir".to_owned(),
        out_dir.display().to_string(),
    ])
    .expect("package run");

    assert!(is_executable(&out_dir.join("install.sh")));
    assert!(is_executable(&out_dir.join(REAL_PACKAGE_NAME).join("install.sh")));
    assert!(!out_dir.join(format!("{REAL_PACKAGE_NAME}.install.sh")).exists());
    let online_install = fs::read_to_string(out_dir.join("install.sh")).expect("read online install");
    assert!(online_install.contains("CLASH_TUI_INSTALL_BASE_URL"));
    assert!(online_install.contains("https://github.com/haokejie/clash-tui/releases/latest/download"));
    assert!(online_install.contains("delegating to package installer"));
    let package_install =
        fs::read_to_string(out_dir.join(REAL_PACKAGE_NAME).join("install.sh")).expect("read package install");
    assert!(package_install.contains("Existing installation detected"));
    assert!(package_install.contains("Stopping active service before update"));
    assert!(package_install.contains("SHOULD_START"));
    assert!(package_install.contains("open TUI:"));
    assert!(package_install.contains("service status: systemctl status"));
    assert!(package_install.contains("systemd not available; skipping service installation"));
    let package_service =
        fs::read_to_string(out_dir.join(REAL_PACKAGE_NAME).join("systemd/clash-tui.service")).expect("read service");
    assert!(package_service.contains("Type=simple"));
    assert!(package_service.contains("CLASH_TUI_CORE_OWNER=systemd"));
    assert!(package_service.contains("ExecStart=/opt/clash-tui/clash-tui core run"));
    assert!(package_service.contains("Restart=on-failure"));
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(out_dir.join(REAL_PACKAGE_NAME).join("manifest.json")).expect("read manifest"),
    )
    .expect("parse manifest");
    assert_eq!(manifest["versions"], json!({ "app": "0.2.1" }));
    assert!(manifest["mihomo"]["version"].is_string());
    let _ = fs::remove_dir_all(tmp_dir);
}

#[test]
fn clash_tui_package_verifier_accepts_required_markers() {
    let fixture = create_package_fixture(false, false);
    package_verify::run(&[
        "--archive".to_owned(),
        fixture.archive.display().to_string(),
        "--manifest".to_owned(),
        fixture.manifest.display().to_string(),
        "--bootstrap".to_owned(),
        fixture.bootstrap.display().to_string(),
    ])
    .expect("verify package");
    let _ = fs::remove_dir_all(fixture.tmp_dir);
}

#[test]
fn clash_tui_package_verifier_rejects_stale_online_bootstrap() {
    let fixture = create_package_fixture(true, false);
    let result = package_verify::run(&[
        "--archive".to_owned(),
        fixture.archive.display().to_string(),
        "--manifest".to_owned(),
        fixture.manifest.display().to_string(),
        "--bootstrap".to_owned(),
        fixture.bootstrap.display().to_string(),
    ]);
    assert!(result.is_err());
    let _ = fs::remove_dir_all(fixture.tmp_dir);
}

#[test]
fn clash_tui_package_verifier_rejects_dirty_source_when_clean_required() {
    let fixture = create_package_fixture(false, true);
    let result = package_verify::run(&[
        "--archive".to_owned(),
        fixture.archive.display().to_string(),
        "--manifest".to_owned(),
        fixture.manifest.display().to_string(),
        "--bootstrap".to_owned(),
        fixture.bootstrap.display().to_string(),
        "--require-clean-source".to_owned(),
    ]);
    assert!(result.is_err());
    let _ = fs::remove_dir_all(fixture.tmp_dir);
}

struct PackageFixture {
    tmp_dir: PathBuf,
    archive: PathBuf,
    manifest: PathBuf,
    bootstrap: PathBuf,
}

fn create_package_fixture(stale_bootstrap: bool, dirty: bool) -> PackageFixture {
    let tmp_dir = temp_dir("clash-tui-package-verify-test");
    let package_dir = tmp_dir.join(REAL_PACKAGE_NAME);
    let resources_dir = package_dir.join("resources");
    let tools_dir = package_dir.join("tools");
    fs::create_dir_all(&tools_dir).expect("create tools");
    fs::create_dir_all(&resources_dir).expect("create resources");

    write_file(&package_dir.join("README.md"), "readme\n");
    write_file(&package_dir.join("env.example"), "env\n");
    write_executable(&package_dir.join("clash-tui"), "#!/usr/bin/env sh\nexit 0\n");
    write_executable(&resources_dir.join("mihomo"), "#!/usr/bin/env sh\nexit 0\n");
    write_file(&resources_dir.join("Country.mmdb"), "country\n");
    write_file(&resources_dir.join("geosite.dat"), "geosite\n");
    write_file(&resources_dir.join("geoip.dat"), "geoip\n");
    write_executable(
        &package_dir.join("install.sh"),
        "#!/usr/bin/env bash\nset -euo pipefail\n",
    );
    write_file(
        &package_dir.join("systemd/clash-tui.service"),
        "[Service]\nExecStart=/opt/clash-tui/clash-tui\n",
    );
    write_executable(
        &tools_dir.join("clash-tui-system-proxy-gnome-acceptance.sh"),
        "#!/usr/bin/env bash\n--verify-acceptance-dir\ngnome-acceptance-verified.json\n--archive\ngnome-acceptance-SHA256SUMS.txt\nCLASH_TUI_SYSTEM_PROXY_SMOKE=1\n",
    );
    write_executable(
        &tools_dir.join("clash-tui-system-proxy-gnome-smoke.py"),
        "#!/usr/bin/env python3\n--preflight\n--output\n--verify-report\n--verify-acceptance-dir\nschemaVersion\ndesktopSession\nreportHashes\nresolvedPath\n",
    );
    write_executable(
        &tools_dir.join("clash-tui-tun-linux-smoke.py"),
        "#!/usr/bin/env python3\n--preflight\n--output\nCLASH_TUI_TUN_SMOKE\n",
    );

    let geo_resources = ["Country.mmdb", "geosite.dat", "geoip.dat"]
        .into_iter()
        .map(|file| {
            json!({
                "file": file,
                "source": "local",
                "downloadUrl": null,
                "sha256": sha256(&resources_dir.join(file)).expect("geo resource sha256"),
            })
        })
        .collect::<Vec<_>>();
    let manifest_value = json!({
        "schemaVersion": 1,
        "packageName": REAL_PACKAGE_NAME,
        "createdAt": "2026-06-23T00:00:00.000Z",
        "gitCommit": "fixture",
        "gitDirty": dirty,
        "target": "x86_64-unknown-linux-gnu",
        "dockerPlatform": "linux/amd64",
        "versions": { "app": "0.2.1" },
        "clashTui": { "binary": "clash-tui", "sha256": sha256(&package_dir.join("clash-tui")).expect("clash-tui sha256") },
        "mihomo": {
            "binary": "resources/mihomo",
            "source": "local",
            "version": "local",
            "downloadUrl": null,
            "archiveSha256": null,
            "sha256": sha256(&resources_dir.join("mihomo")).expect("mihomo sha256"),
        },
        "geoResources": geo_resources,
        "installEntry": {
            "pathDefault": "/usr/local/bin/clash-tui",
            "type": "symlink",
            "layoutFile": "install-layout.env",
        },
        "packageFiles": [
            "README.md",
            "env.example",
            "install.sh",
            "tools/clash-tui-system-proxy-gnome-acceptance.sh",
            "tools/clash-tui-system-proxy-gnome-smoke.py",
            "tools/clash-tui-tun-linux-smoke.py",
            "systemd/clash-tui.service",
        ],
    });
    write_file(
        &package_dir.join("manifest.json"),
        &format!("{}\n", serde_json::to_string_pretty(&manifest_value).expect("manifest")),
    );

    let archive = tmp_dir.join(format!("{REAL_PACKAGE_NAME}.tar.gz"));
    let tar = run_local(
        "tar",
        &[
            OsString::from("-czf"),
            archive.as_os_str().to_owned(),
            OsString::from("-C"),
            tmp_dir.as_os_str().to_owned(),
            OsString::from(REAL_PACKAGE_NAME),
        ],
        &tmp_dir,
        &[],
    );
    assert_eq!(tar.status, 0, "{}", tar.stderr);
    let archive_sha = sha256(&archive).expect("archive sha256");
    let mut sidecar = manifest_value;
    sidecar["archive"] = json!({
        "file": archive.file_name().unwrap().to_string_lossy(),
        "sha256": archive_sha,
    });
    let manifest = tmp_dir.join(format!("{REAL_PACKAGE_NAME}.manifest.json"));
    write_file(
        &manifest,
        &format!("{}\n", serde_json::to_string_pretty(&sidecar).expect("manifest")),
    );
    let bootstrap = tmp_dir.join("install.sh");
    let bootstrap_content = if stale_bootstrap {
        "#!/usr/bin/env bash\n# missing required marker\n"
    } else {
        "#!/usr/bin/env bash\nCLASH_TUI_INSTALL_BASE_URL=1\n--base-url\nverify_archive\ndelegating to package installer\n"
    };
    write_executable(&bootstrap, bootstrap_content);
    PackageFixture {
        tmp_dir,
        archive,
        manifest,
        bootstrap,
    }
}

#[test]
fn gnome_acceptance_wrapper_writes_optional_evidence_archive_after_verification() {
    let root = root_dir().expect("root");
    let tmp_dir = temp_dir("clash-tui-gnome-acceptance-test");
    let fake_smoke = tmp_dir.join("fake-smoke.py");
    let output_dir = tmp_dir.join("reports");
    let archive_path = tmp_dir.join("evidence/gnome-acceptance.tar.gz");
    let extract_dir = tmp_dir.join("extract");
    write_executable(&fake_smoke, fake_smoke_script());
    let acceptance_script = root.join("scripts/clash-tui-system-proxy-gnome-acceptance.sh");
    let syntax = run_local(
        "bash",
        &[OsString::from("-n"), acceptance_script.as_os_str().to_owned()],
        &root,
        &[],
    );
    assert_eq!(syntax.status, 0, "{}", syntax.stderr);
    let result = run_local(
        "bash",
        &[
            acceptance_script.into_os_string(),
            OsString::from("--smoke-script"),
            fake_smoke.into_os_string(),
            OsString::from("--bin"),
            OsString::from("fake-clash-tui"),
            OsString::from("--output-dir"),
            output_dir.into_os_string(),
            OsString::from("--archive"),
            archive_path.as_os_str().to_owned(),
            OsString::from("--yes"),
        ],
        &root,
        &[],
    );
    assert_eq!(result.status, 0, "{}\n{}", result.stdout, result.stderr);
    assert!(result.stdout.contains("Evidence archive:"));
    assert!(result.stdout.contains("Evidence archive SHA256:"));
    assert!(archive_path.is_file());
    fs::create_dir_all(&extract_dir).expect("create extract");
    let extract = run_local(
        "tar",
        &[
            OsString::from("-xzf"),
            archive_path.into_os_string(),
            OsString::from("-C"),
            extract_dir.as_os_str().to_owned(),
        ],
        &root,
        &[],
    );
    assert_eq!(extract.status, 0, "{}", extract.stderr);
    let files = sorted_file_names(&extract_dir);
    assert_eq!(
        files,
        vec![
            "gnome-acceptance-SHA256SUMS.txt",
            "gnome-acceptance-verified.json",
            "gnome-preflight.json",
            "gnome-smoke-verified.json",
            "gnome-smoke.json",
        ]
    );
    let sums = fs::read_to_string(extract_dir.join("gnome-acceptance-SHA256SUMS.txt")).expect("read sums");
    for file in files.into_iter().filter(|name| name.ends_with(".json")) {
        assert!(sums.contains(&format!("  {file}")), "missing sum for {file}");
    }
    let _ = fs::remove_dir_all(tmp_dir);
}

fn fake_smoke_script() -> &'static str {
    r#"#!/usr/bin/env python3
import argparse
import json
import os

parser = argparse.ArgumentParser()
parser.add_argument("--preflight", action="store_true")
parser.add_argument("--yes", action="store_true")
parser.add_argument("--allow-root", action="store_true")
parser.add_argument("--bin")
parser.add_argument("--output")
parser.add_argument("--verify-report")
parser.add_argument("--verify-acceptance-dir")
args = parser.parse_args()

if args.preflight:
    mode = "preflight"
    mutated = False
elif args.verify_report:
    mode = "verify-report"
    mutated = False
elif args.verify_acceptance_dir:
    mode = "verify-acceptance"
    mutated = False
else:
    mode = "smoke"
    mutated = True

report = {
    "ok": True,
    "mode": mode,
    "mutated": mutated,
    "urlLeak": False,
    "fake": True,
}

if args.output:
    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "w", encoding="utf-8") as handle:
        json.dump(report, handle, sort_keys=True)
        handle.write("\n")

print(json.dumps(report, sort_keys=True))
"#
}

fn sorted_file_names(dir: &Path) -> Vec<String> {
    let mut files = fs::read_dir(dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn is_executable(file: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::metadata(file)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        file.is_file()
    }
}
