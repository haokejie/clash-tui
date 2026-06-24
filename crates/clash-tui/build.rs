use std::{env, fs, io, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let workspace_toml = Path::new(&manifest_dir).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", workspace_toml.display());

    let content = fs::read_to_string(&workspace_toml)?;
    let app_version = read_workspace_app_version(&content).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Cargo.toml is missing [workspace.metadata.clash-tui] app-version",
        )
    })?;
    println!("cargo:rustc-env=CLASH_TUI_APP_VERSION={app_version}");
    Ok(())
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
