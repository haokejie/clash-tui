#![allow(
    clippy::branches_sharing_code,
    clippy::cognitive_complexity,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
#[cfg(test)]
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::{XtaskResult, display_command, root_dir, run_command, run_command_str, strip_separator};

const DEFAULT_OUT_DIR: &str = "target/clash-tui-dist";
const DEFAULT_CARGO_CACHE_DIR: &str = "target/clash-tui-cargo-cache";
const DEFAULT_DOWNLOAD_CACHE_DIR: &str = "target/clash-tui-download-cache";
const DEFAULT_MIHOMO_RELEASE_BASE_URL: &str = "https://github.com/MetaCubeX/mihomo/releases/download";
const DEFAULT_MIHOMO_VERSION_URL: &str = "https://github.com/MetaCubeX/mihomo/releases/latest/download/version.txt";
const DEFAULT_GEO_RELEASE_BASE_URL: &str = "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest";

const PASS_THROUGH_ENV: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "CARGO_HTTP_TIMEOUT",
    "CARGO_HTTP_MULTIPLEXING",
    "CARGO_REGISTRIES_CRATES_IO_PROTOCOL",
    "CLASH_TUI_PACKAGE_MIHOMO_VERSION",
    "CLASH_TUI_PACKAGE_MIHOMO_VERSION_URL",
    "CLASH_TUI_PACKAGE_MIHOMO_BASE_URL",
    "CLASH_TUI_PACKAGE_GEO_BASE_URL",
    "CLASH_TUI_PACKAGE_GEO_COUNTRY_URL",
    "CLASH_TUI_PACKAGE_GEO_GEOSITE_URL",
    "CLASH_TUI_PACKAGE_GEO_GEOIP_URL",
];

#[derive(Clone, Copy)]
struct TargetInfo {
    package_arch: &'static str,
    docker_platform: &'static str,
    mihomo_name: &'static str,
}

struct GeoResource {
    file: &'static str,
    remote_file: &'static str,
    env_url: &'static str,
}

const GEO_RESOURCES: &[GeoResource] = &[
    GeoResource {
        file: "Country.mmdb",
        remote_file: "country.mmdb",
        env_url: "CLASH_TUI_PACKAGE_GEO_COUNTRY_URL",
    },
    GeoResource {
        file: "geosite.dat",
        remote_file: "geosite.dat",
        env_url: "CLASH_TUI_PACKAGE_GEO_GEOSITE_URL",
    },
    GeoResource {
        file: "geoip.dat",
        remote_file: "geoip.dat",
        env_url: "CLASH_TUI_PACKAGE_GEO_GEOIP_URL",
    },
];

#[derive(Debug)]
struct Options {
    targets: Vec<String>,
    docker: bool,
    skip_build: bool,
    skip_archive: bool,
    out_dir: PathBuf,
    binary: Option<PathBuf>,
    mihomo_bin: Option<PathBuf>,
    geo_dir: Option<PathBuf>,
}

struct Mihomo {
    file: PathBuf,
    source: &'static str,
    version: String,
    download_url: Option<String>,
    archive_sha256: Option<String>,
}

pub(crate) fn run(args: &[String]) -> XtaskResult {
    let options = parse_args(strip_separator(args))?;
    if options.docker && env::var_os("CLASH_TUI_PACKAGE_INSIDE_DOCKER").is_none() {
        return docker_run(&options);
    }

    for target in &options.targets {
        assemble_target(target, &options)?;
    }
    Ok(())
}

fn usage() -> &'static str {
    "usage: cargo xtask package [options]

Options:
  --target <triple>       Rust target triple. Repeatable.
  --out-dir <path>        Output directory. Default: target/clash-tui-dist
  --docker                Run the package job inside Docker. Default when launched on the host.
  --no-docker             Run directly on the current host.
  --skip-build            Reuse an existing binary instead of building.
  --binary <path>         clash-tui binary to package.
  --mihomo-bin <path>     Local mihomo binary to package.
  --geo-dir <path>        Directory containing Country.mmdb, geosite.dat, geoip.dat.
  --skip-archive          Assemble package directory without tar.gz.
  -h, --help              Show this help.
"
}

fn parse_args(args: &[String]) -> XtaskResult<Options> {
    let root = root_dir()?;
    let mut options = Options {
        targets: Vec::new(),
        docker: env::var_os("CLASH_TUI_PACKAGE_INSIDE_DOCKER").is_none(),
        skip_build: false,
        skip_archive: false,
        out_dir: root.join(DEFAULT_OUT_DIR),
        binary: None,
        mihomo_bin: None,
        geo_dir: None,
    };

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
            "--target" => options.targets.push(read_value(&mut index)?),
            "--out-dir" => options.out_dir = resolve_path(&root, &read_value(&mut index)?),
            "--docker" => options.docker = true,
            "--no-docker" => options.docker = false,
            "--skip-build" => options.skip_build = true,
            "--skip-archive" => options.skip_archive = true,
            "--binary" => options.binary = Some(resolve_path(&root, &read_value(&mut index)?)),
            "--mihomo-bin" => options.mihomo_bin = Some(resolve_path(&root, &read_value(&mut index)?)),
            "--geo-dir" => options.geo_dir = Some(resolve_path(&root, &read_value(&mut index)?)),
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument: {unknown}")),
        }
        index += 1;
    }

    if options.targets.is_empty() {
        options.targets.push("x86_64-unknown-linux-gnu".to_owned());
    }
    for target in &options.targets {
        target_info(target)?;
    }
    Ok(options)
}

fn resolve_path(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() { path } else { root.join(path) }
}

fn target_info(target: &str) -> XtaskResult<TargetInfo> {
    match target {
        "x86_64-unknown-linux-gnu" => Ok(TargetInfo {
            package_arch: "x86_64",
            docker_platform: "linux/amd64",
            mihomo_name: "mihomo-linux-amd64-v2",
        }),
        "aarch64-unknown-linux-gnu" => Ok(TargetInfo {
            package_arch: "aarch64",
            docker_platform: "linux/arm64",
            mihomo_name: "mihomo-linux-arm64",
        }),
        _ => Err(format!("unsupported target: {target}")),
    }
}

fn run_capture_env(program: &str, args: &[OsString], cwd: &Path, extra_env: &[(&str, String)]) -> XtaskResult<String> {
    eprintln!("$ {}", display_command(program, args));
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|error| format!("{program} output is not UTF-8: {error}"))
    } else {
        Err(format!(
            "command failed: {}\n{}",
            display_command(program, args),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn run_to_file(program: &str, args: &[OsString], output_file: &Path) -> XtaskResult {
    let root = root_dir()?;
    eprintln!("$ {} > {}", display_command(program, args), output_file.display());
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "command failed: {}\n{}",
            display_command(program, args),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if let Some(parent) = output_file.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(output_file, output.stdout).map_err(|error| format!("failed to write {}: {error}", output_file.display()))
}

fn run_env(program: &str, args: &[OsString], extra_env: &[(&str, String)]) -> XtaskResult {
    let root = root_dir()?;
    eprintln!("+ {}", display_command(program, args));
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed: {}", display_command(program, args)))
    }
}

fn docker_run(options: &Options) -> XtaskResult {
    if options.skip_build || options.binary.is_some() || options.mihomo_bin.is_some() || options.geo_dir.is_some() {
        return Err(
            "Docker packaging does not support local override inputs; use --no-docker for --skip-build, --binary, --mihomo-bin, or --geo-dir"
                .to_owned(),
        );
    }

    let root = root_dir()?;
    fs::create_dir_all(&options.out_dir)
        .map_err(|error| format!("failed to create {}: {error}", options.out_dir.display()))?;
    let cargo_cache_dir = env_path("CLASH_TUI_PACKAGE_CARGO_CACHE_DIR", DEFAULT_CARGO_CACHE_DIR)?;
    let download_cache_dir = env_path("CLASH_TUI_PACKAGE_DOWNLOAD_CACHE_DIR", DEFAULT_DOWNLOAD_CACHE_DIR)?;
    fs::create_dir_all(&cargo_cache_dir)
        .map_err(|error| format!("failed to create {}: {error}", cargo_cache_dir.display()))?;
    fs::create_dir_all(&download_cache_dir)
        .map_err(|error| format!("failed to create {}: {error}", download_cache_dir.display()))?;

    let source_revision = git_commit()?;
    let source_dirty = git_dirty()?;
    assert_clean_source(source_dirty)?;

    let image = "clash-tui-package:builder";
    let dockerfile = root.join("packaging/clash-tui/Dockerfile");
    let build_context = root.join("packaging/clash-tui");
    let mut build_args = vec![
        OsString::from("buildx"),
        OsString::from("build"),
        OsString::from("--load"),
        OsString::from("-f"),
        dockerfile.into_os_string(),
    ];
    for (env_name, arg_name) in [
        ("CLASH_TUI_PACKAGE_DEBIAN_MIRROR", "DEBIAN_MIRROR"),
        ("CLASH_TUI_PACKAGE_DEBIAN_SECURITY_MIRROR", "DEBIAN_SECURITY_MIRROR"),
        ("CLASH_TUI_PACKAGE_RUSTUP_DIST_SERVER", "RUSTUP_DIST_SERVER"),
    ] {
        if let Ok(value) = env::var(env_name) {
            build_args.push(OsString::from("--build-arg"));
            build_args.push(OsString::from(format!("{arg_name}={value}")));
        }
    }
    build_args.extend([
        OsString::from("-t"),
        OsString::from(image),
        build_context.into_os_string(),
    ]);
    run_command("docker", &build_args)?;

    for target in &options.targets {
        let mut env_args = vec![
            OsString::from("-e"),
            OsString::from("CLASH_TUI_PACKAGE_INSIDE_DOCKER=1"),
            OsString::from("-e"),
            OsString::from(format!("CLASH_TUI_PACKAGE_GIT_COMMIT={source_revision}")),
            OsString::from("-e"),
            OsString::from(format!(
                "CLASH_TUI_PACKAGE_GIT_DIRTY={}",
                if source_dirty { "1" } else { "0" }
            )),
            OsString::from("-e"),
            OsString::from(format!("CLASH_TUI_PACKAGE_TARGET={target}")),
            OsString::from("-e"),
            OsString::from("CARGO_HOME=/cargo-cache"),
            OsString::from("-e"),
            OsString::from("CLASH_TUI_PACKAGE_DOWNLOAD_CACHE_DIR=/download-cache"),
        ];
        if options.skip_archive {
            env_args.extend([OsString::from("-e"), OsString::from("CLASH_TUI_PACKAGE_SKIP_ARCHIVE=1")]);
        }
        for name in PASS_THROUGH_ENV {
            if env::var_os(name).is_some() {
                env_args.extend([OsString::from("-e"), OsString::from(name)]);
            }
        }

        let container_script = r#"
set -e
mkdir -p /workspace /out
tar -C /source \
  --exclude='./target' \
  -cf - . | tar -C /workspace -xf -
cd /workspace
if [ -n "${CLASH_TUI_PACKAGE_SKIP_ARCHIVE:-}" ]; then
  cargo xtask package --no-docker --target "$CLASH_TUI_PACKAGE_TARGET" --out-dir /out --skip-archive
else
  cargo xtask package --no-docker --target "$CLASH_TUI_PACKAGE_TARGET" --out-dir /out
fi
"#;
        let mut docker_args = vec![OsString::from("run"), OsString::from("--rm")];
        docker_args.extend(env_args);
        docker_args.extend([
            OsString::from("-v"),
            OsString::from(format!("{}:/source:ro", root.display())),
            OsString::from("-v"),
            OsString::from(format!("{}:/out", options.out_dir.display())),
            OsString::from("-v"),
            OsString::from(format!("{}:/cargo-cache", cargo_cache_dir.display())),
            OsString::from("-v"),
            OsString::from(format!("{}:/download-cache", download_cache_dir.display())),
            OsString::from("-w"),
            OsString::from("/workspace"),
            OsString::from(image),
            OsString::from("sh"),
            OsString::from("-c"),
            OsString::from(container_script),
        ]);
        run_command("docker", &docker_args)?;
    }

    Ok(())
}

fn env_path(name: &str, fallback: &str) -> XtaskResult<PathBuf> {
    let root = root_dir()?;
    Ok(env::var(name)
        .map(|value| resolve_path(&root, &value))
        .unwrap_or_else(|_| root.join(fallback)))
}

fn build_workspace(target: &str, options: &Options) -> XtaskResult {
    if options.skip_build {
        return Ok(());
    }
    run_command_str("rustup", &["target", "add", target])?;
    run_command_str("cargo", &["build", "-p", "clash-tui", "--release", "--target", target])
}

fn resolve_binary(target: &str, options: &Options) -> XtaskResult<PathBuf> {
    if let Some(binary) = &options.binary {
        require_file(binary, "clash-tui binary")?;
        return Ok(binary.clone());
    }

    let root = root_dir()?;
    let target_binary = root.join("target").join(target).join("release").join("clash-tui");
    if target_binary.is_file() {
        return Ok(target_binary);
    }

    let native_binary = root.join("target/release/clash-tui");
    require_file(&native_binary, "clash-tui binary")?;
    Ok(native_binary)
}

fn resolve_mihomo(target: &str, options: &Options, work_dir: &Path) -> XtaskResult<Mihomo> {
    if let Some(binary) = &options.mihomo_bin {
        require_file(binary, "mihomo binary")?;
        return Ok(Mihomo {
            file: binary.clone(),
            source: "local",
            version: "local".to_owned(),
            download_url: None,
            archive_sha256: None,
        });
    }

    let info = mihomo_download_info(target)?;
    let archive_path = work_dir.join(&info.1);
    let binary_path = work_dir.join("mihomo");
    download_file(&info.2, &archive_path)?;
    run_to_file(
        "gzip",
        &[OsString::from("-dc"), archive_path.clone().into_os_string()],
        &binary_path,
    )?;
    set_mode(&binary_path, 0o755)?;
    Ok(Mihomo {
        file: binary_path,
        source: "download",
        version: info.0,
        download_url: Some(info.2),
        archive_sha256: Some(sha256(&archive_path)?),
    })
}

fn mihomo_download_info(target: &str) -> XtaskResult<(String, String, String)> {
    let info = target_info(target)?;
    let version = env::var("CLASH_TUI_PACKAGE_MIHOMO_VERSION").unwrap_or_else(|_| {
        let url =
            env::var("CLASH_TUI_PACKAGE_MIHOMO_VERSION_URL").unwrap_or_else(|_| DEFAULT_MIHOMO_VERSION_URL.to_owned());
        curl_text(&url).unwrap_or_default()
    });
    if version.trim().is_empty() {
        return Err("failed to resolve mihomo version".to_owned());
    }
    let base_url =
        env::var("CLASH_TUI_PACKAGE_MIHOMO_BASE_URL").unwrap_or_else(|_| DEFAULT_MIHOMO_RELEASE_BASE_URL.to_owned());
    let archive = format!("{}-{}.gz", info.mihomo_name, version.trim());
    let url = format!("{}/{}/{}", strip_trailing_slash(&base_url), version.trim(), archive);
    Ok((version.trim().to_owned(), archive, url))
}

fn resolve_geo_resources(options: &Options, package_resources_dir: &Path) -> XtaskResult<Vec<Value>> {
    let mut resources = Vec::new();
    for resource in GEO_RESOURCES {
        let target = package_resources_dir.join(resource.file);
        if let Some(geo_dir) = &options.geo_dir {
            let source = geo_dir.join(resource.file);
            require_file(&source, &format!("geo resource {}", resource.file))?;
            copy_file(&source, &target, None)?;
            resources.push(json!({
                "file": resource.file,
                "source": "local",
                "downloadUrl": null,
                "sha256": sha256(&target)?,
            }));
        } else {
            let base_url =
                env::var("CLASH_TUI_PACKAGE_GEO_BASE_URL").unwrap_or_else(|_| DEFAULT_GEO_RELEASE_BASE_URL.to_owned());
            let download_url = env::var(resource.env_url)
                .unwrap_or_else(|_| format!("{}/{}", strip_trailing_slash(&base_url), resource.remote_file));
            download_file(&download_url, &target)?;
            resources.push(json!({
                "file": resource.file,
                "source": "download",
                "downloadUrl": download_url,
                "sha256": sha256(&target)?,
            }));
        }
    }
    Ok(resources)
}

fn git_commit() -> XtaskResult<String> {
    if let Ok(value) = env::var("CLASH_TUI_PACKAGE_GIT_COMMIT") {
        return Ok(value);
    }
    let args = [OsString::from("rev-parse"), OsString::from("HEAD")];
    run_capture_env("git", &args, &root_dir()?, &[])
        .map(|value| value.trim().to_owned())
        .or_else(|_| Ok("unknown".to_owned()))
}

fn git_dirty() -> XtaskResult<bool> {
    if let Ok(value) = env::var("CLASH_TUI_PACKAGE_GIT_DIRTY") {
        return Ok(value == "1" || value == "true");
    }
    let args = [
        OsString::from("status"),
        OsString::from("--short"),
        OsString::from("--untracked-files=all"),
    ];
    run_capture_env("git", &args, &root_dir()?, &[])
        .map(|value| !value.trim().is_empty())
        .or(Ok(true))
}

fn assert_clean_source(source_dirty: bool) -> XtaskResult {
    if env::var("CLASH_TUI_PACKAGE_REQUIRE_CLEAN").as_deref() == Ok("1") && source_dirty {
        Err("working tree is dirty; commit/stash changes or unset CLASH_TUI_PACKAGE_REQUIRE_CLEAN".to_owned())
    } else {
        Ok(())
    }
}

fn package_versions() -> XtaskResult<Value> {
    let root = root_dir()?;
    let workspace_toml =
        fs::read_to_string(root.join("Cargo.toml")).map_err(|error| format!("failed to read Cargo.toml: {error}"))?;
    let app_version = read_workspace_app_version(&workspace_toml)
        .ok_or_else(|| "Cargo.toml is missing [workspace.metadata.clash-tui] app-version".to_owned())?;
    Ok(json!({ "app": app_version }))
}

fn read_workspace_app_version(content: &str) -> Option<String> {
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.metadata.clash-tui]";
            continue;
        }
        if in_section && trimmed.starts_with("app-version") {
            return read_toml_line_string(trimmed);
        }
    }
    None
}

fn read_toml_line_string(line: &str) -> Option<String> {
    let (_, value) = line.split_once('=')?;
    let value = value.trim();
    let value = value.strip_prefix('"')?.split_once('"')?.0;
    Some(value.to_owned())
}

fn write_manifest(out_dir: &Path, package_dir: &Path, manifest: &Value) -> XtaskResult {
    let manifest_text = format!(
        "{}\n",
        serde_json::to_string_pretty(manifest).map_err(|error| format!("failed to serialize manifest: {error}"))?
    );
    fs::write(package_dir.join("manifest.json"), &manifest_text)
        .map_err(|error| format!("failed to write archive manifest: {error}"))?;
    let package_name = manifest
        .get("packageName")
        .and_then(Value::as_str)
        .ok_or_else(|| "manifest is missing packageName".to_owned())?;
    fs::write(out_dir.join(format!("{package_name}.manifest.json")), &manifest_text)
        .map_err(|error| format!("failed to write sidecar manifest: {error}"))?;
    fs::write(out_dir.join("manifest.json"), manifest_text)
        .map_err(|error| format!("failed to write manifest.json: {error}"))
}

fn create_archive(out_dir: &Path, package_name: &str) -> XtaskResult<(String, String)> {
    let archive_name = format!("{package_name}.tar.gz");
    let archive_path = out_dir.join(&archive_name);
    let _ = fs::remove_file(&archive_path);
    let mut args = Vec::<OsString>::new();
    if cfg!(target_os = "macos") {
        args.extend([OsString::from("--no-xattrs"), OsString::from("--no-mac-metadata")]);
    }
    args.extend([
        OsString::from("-czf"),
        archive_path.clone().into_os_string(),
        OsString::from("-C"),
        out_dir.as_os_str().to_owned(),
        OsString::from(package_name),
    ]);
    run_env("tar", &args, &[("COPYFILE_DISABLE", "1".to_owned())])?;
    let archive_sha256 = sha256(&archive_path)?;
    fs::write(
        format!("{}.sha256", archive_path.display()),
        format!("{archive_sha256}  {archive_name}\n"),
    )
    .map_err(|error| format!("failed to write archive sha256: {error}"))?;
    Ok((archive_name, archive_sha256))
}

fn assemble_target(target: &str, options: &Options) -> XtaskResult {
    let source_dirty = git_dirty()?;
    assert_clean_source(source_dirty)?;
    let target_info = target_info(target)?;
    let versions = package_versions()?;
    let package_name = format!("clash-tui-linux-{}", target_info.package_arch);
    let out_dir = &options.out_dir;
    let package_dir = out_dir.join(&package_name);
    let package_resources_dir = package_dir.join("resources");
    let package_tools_dir = package_dir.join("tools");
    let work_dir = out_dir.join(".work").join(target);

    build_workspace(target, options)?;
    clean_dir(&package_dir)?;
    clean_dir(&work_dir)?;
    fs::create_dir_all(&package_resources_dir)
        .map_err(|error| format!("failed to create {}: {error}", package_resources_dir.display()))?;
    fs::create_dir_all(&package_tools_dir)
        .map_err(|error| format!("failed to create {}: {error}", package_tools_dir.display()))?;

    let root = root_dir()?;
    let binary = resolve_binary(target, options)?;
    let mihomo = resolve_mihomo(target, options, &work_dir)?;
    copy_file(&binary, &package_dir.join("clash-tui"), Some(0o755))?;
    copy_file(&mihomo.file, &package_resources_dir.join("mihomo"), Some(0o755))?;
    let geo_resources = resolve_geo_resources(options, &package_resources_dir)?;

    let package_template_dir = root.join("packaging/clash-tui");
    copy_dir(&package_template_dir.join("systemd"), &package_dir.join("systemd"))?;
    copy_file(
        &package_template_dir.join("README.md"),
        &package_dir.join("README.md"),
        None,
    )?;
    copy_file(
        &package_template_dir.join("env.example"),
        &package_dir.join("env.example"),
        Some(0o600),
    )?;
    copy_file(
        &package_template_dir.join("install.sh"),
        &package_dir.join("install.sh"),
        Some(0o755),
    )?;
    copy_file(
        &root.join("packaging/install.sh"),
        &out_dir.join("install.sh"),
        Some(0o755),
    )?;
    let _ = fs::remove_file(out_dir.join(format!("{package_name}.install.sh")));
    copy_file(
        &root.join("scripts/clash-tui-system-proxy-gnome-smoke.py"),
        &package_tools_dir.join("clash-tui-system-proxy-gnome-smoke.py"),
        Some(0o755),
    )?;
    copy_file(
        &root.join("scripts/clash-tui-system-proxy-gnome-acceptance.sh"),
        &package_tools_dir.join("clash-tui-system-proxy-gnome-acceptance.sh"),
        Some(0o755),
    )?;
    copy_file(
        &root.join("scripts/clash-tui-tun-linux-smoke.py"),
        &package_tools_dir.join("clash-tui-tun-linux-smoke.py"),
        Some(0o755),
    )?;

    let binary_sha256 = sha256(&package_dir.join("clash-tui"))?;
    let mihomo_sha256 = sha256(&package_resources_dir.join("mihomo"))?;
    let mut manifest = json!({
        "schemaVersion": 1,
        "packageName": package_name,
        "createdAt": created_at()?,
        "gitCommit": git_commit()?,
        "gitDirty": source_dirty,
        "target": target,
        "dockerPlatform": target_info.docker_platform,
        "versions": versions,
        "clashTui": {
            "binary": "clash-tui",
            "sha256": binary_sha256,
        },
        "mihomo": {
            "binary": "resources/mihomo",
            "source": mihomo.source,
            "version": mihomo.version,
            "downloadUrl": mihomo.download_url,
            "archiveSha256": mihomo.archive_sha256,
            "sha256": mihomo_sha256,
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
    write_manifest(out_dir, &package_dir, &manifest)?;

    if !options.skip_archive {
        let (archive_name, archive_sha256) =
            create_archive(out_dir, manifest["packageName"].as_str().unwrap_or_default())?;
        manifest["archive"] = json!({
            "file": archive_name,
            "sha256": archive_sha256,
        });
        write_manifest(out_dir, &package_dir, &manifest)?;
    }

    let _ = fs::remove_dir_all(&work_dir);
    println!("clash-tui package ready: {}", package_dir.display());
    Ok(())
}

fn created_at() -> XtaskResult<String> {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run date: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!(
            "date failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn clean_dir(dir: &Path) -> XtaskResult {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|error| format!("failed to create {}: {error}", dir.display()))
}

fn copy_dir(from: &Path, to: &Path) -> XtaskResult {
    let _ = fs::remove_dir_all(to);
    fs::create_dir_all(to).map_err(|error| format!("failed to create {}: {error}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|error| format!("failed to read {}: {error}", from.display()))? {
        let entry = entry.map_err(|error| format!("failed to read directory entry: {error}"))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            copy_file(&source, &target, None)?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path, mode: Option<u32>) -> XtaskResult {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::copy(from, to).map_err(|error| format!("failed to copy {} to {}: {error}", from.display(), to.display()))?;
    if let Some(mode) = mode {
        set_mode(to, mode)?;
    }
    Ok(())
}

fn set_mode(file: &Path, mode: u32) -> XtaskResult {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(file, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("failed to chmod {}: {error}", file.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
        Ok(())
    }
}

fn require_file(file: &Path, label: &str) -> XtaskResult {
    if file.is_file() {
        Ok(())
    } else {
        Err(format!("{label} not found: {}", file.display()))
    }
}

fn curl_text(url: &str) -> XtaskResult<String> {
    let root = root_dir()?;
    let args = [OsString::from("-fsSL"), OsString::from(url)];
    run_capture_env("curl", &args, &root, &[]).map(|value| value.trim().to_owned())
}

fn download_file(url: &str, to: &Path) -> XtaskResult {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let Some(cache_dir) = download_cache_dir()? else {
        return run_command(
            "curl",
            &[
                OsString::from("-fL"),
                OsString::from("--retry"),
                OsString::from("3"),
                OsString::from("--connect-timeout"),
                OsString::from("20"),
                OsString::from("-o"),
                to.as_os_str().to_owned(),
                OsString::from(url),
            ],
        );
    };

    fs::create_dir_all(&cache_dir).map_err(|error| format!("failed to create {}: {error}", cache_dir.display()))?;
    let cache_path = cache_dir.join(cache_file_name(
        url,
        to.file_name().and_then(OsStr::to_str).unwrap_or("download"),
    ));
    if cache_path.is_file() {
        eprintln!("$ cp {} {} # cached {url}", cache_path.display(), to.display());
        return copy_file(&cache_path, to, None);
    }

    let temp_path = to.with_extension("download");
    let _ = fs::remove_file(&temp_path);
    run_command(
        "curl",
        &[
            OsString::from("-fL"),
            OsString::from("--retry"),
            OsString::from("3"),
            OsString::from("--connect-timeout"),
            OsString::from("20"),
            OsString::from("-o"),
            temp_path.as_os_str().to_owned(),
            OsString::from(url),
        ],
    )?;
    copy_file(&temp_path, to, None)?;
    copy_file(&temp_path, &cache_path, None)?;
    let _ = fs::remove_file(temp_path);
    Ok(())
}

fn download_cache_dir() -> XtaskResult<Option<PathBuf>> {
    env::var("CLASH_TUI_PACKAGE_DOWNLOAD_CACHE_DIR")
        .map(|value| Ok(Some(resolve_path(&root_dir()?, &value))))
        .unwrap_or(Ok(None))
}

fn cache_file_name(url: &str, fallback_name: &str) -> String {
    let hash = fnv64(url.as_bytes());
    let path_part = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    let basename = path_part
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_name);
    let safe = basename
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{hash:016x}-{safe}")
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
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

fn strip_trailing_slash(value: &str) -> &str {
    value.trim_end_matches('/')
}

#[cfg(test)]
pub(crate) fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) fn write_executable(file: &Path, content: &str) {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    let mut handle = fs::File::create(file).expect("create file");
    handle.write_all(content.as_bytes()).expect("write file");
    set_mode(file, 0o755).expect("chmod");
}
