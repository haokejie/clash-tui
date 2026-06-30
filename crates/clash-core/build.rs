use std::{env, fs, path::PathBuf};

fn main() {
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        println!("cargo:rustc-env=CLASH_TUI_APP_VERSION=0.0.0");
        return;
    };
    let manifest_dir = PathBuf::from(manifest_dir);
    let workspace_manifest = manifest_dir.join("..").join("..").join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", workspace_manifest.display());

    let Ok(manifest) = fs::read_to_string(&workspace_manifest) else {
        println!("cargo:rustc-env=CLASH_TUI_APP_VERSION=0.0.0");
        return;
    };
    let version = workspace_app_version(&manifest).unwrap_or("0.0.0");
    println!("cargo:rustc-env=CLASH_TUI_APP_VERSION={version}");
}

fn workspace_app_version(manifest: &str) -> Option<&str> {
    let mut in_app_metadata = false;

    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_app_metadata = line == "[workspace.metadata.clash-tui]";
            continue;
        }
        if !in_app_metadata {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "app-version" {
            continue;
        }

        return value
            .split('#')
            .next()
            .map(str::trim)
            .map(|value| value.trim_matches('"'));
    }

    None
}
