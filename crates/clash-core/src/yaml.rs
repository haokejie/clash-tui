use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use serde_yaml_ng::{Mapping, Value};

pub async fn read_yaml<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T> {
    let path = path.as_ref();
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        bail!("file not found \"{}\"", path.display());
    }

    let yaml = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_yaml_ng::from_str(&yaml).with_context(|| format!("failed to parse yaml {}", path.display()))
}

pub async fn read_mapping(path: impl AsRef<Path>) -> Result<Mapping> {
    let path = path.as_ref();
    let mut value: Value = read_yaml(path).await?;
    value
        .apply_merge()
        .with_context(|| format!("failed to apply yaml merge {}", path.display()))?;
    value
        .as_mapping()
        .cloned()
        .with_context(|| format!("yaml root must be a mapping {}", path.display()))
}

pub async fn save_yaml<T: Serialize + Sync>(path: impl AsRef<Path>, data: &T, prefix: Option<&str>) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let data = serde_yaml_ng::to_string(data).context("failed to serialize yaml")?;
    let yaml = match prefix {
        Some(prefix) => format!("{prefix}\n\n{data}"),
        None => data,
    };
    tokio::fs::write(path, yaml)
        .await
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::fs;

    use serde::{Deserialize, Serialize};

    use super::{read_yaml, save_yaml};

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        name: String,
        enabled: bool,
    }

    #[tokio::test]
    async fn yaml_roundtrip_preserves_struct_data() {
        let root = std::env::temp_dir().join(format!("clash-core-yaml-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = root.join("sample.yaml");
        let sample = Sample {
            name: "demo".into(),
            enabled: true,
        };

        save_yaml(&path, &sample, Some("# test")).await.expect("save yaml");
        let loaded: Sample = read_yaml(&path).await.expect("read yaml");

        assert_eq!(loaded, sample);

        let _ = fs::remove_dir_all(&root);
    }
}
