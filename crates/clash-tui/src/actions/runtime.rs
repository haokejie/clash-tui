use anyhow::Result;
use clash_core::RuntimeConfigResult;

use crate::state::AppState;

pub async fn generate(state: &AppState) -> Result<RuntimeConfigResult> {
    state.runtime.generate().await
}

pub async fn read_yaml(state: &AppState) -> Result<String> {
    state.runtime.read_runtime_yaml().await
}
