#![recursion_limit = "512"]
#![cfg_attr(test, allow(clippy::expect_used))]

mod actions;
mod cli;
mod jobs;
mod kernel;
mod metrics;
mod mihomo_controller;
mod options;
mod platform;
mod state;
mod subscriptions;
mod system_info;
mod terminal_display;
mod tui;
mod validation;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse_args();
    let options = cli.options()?;
    let state = state::AppState::initialize(options).await?;
    if cli.runs_tui() {
        subscriptions::spawn_subscription_startup_sweep(std::sync::Arc::clone(&state));
    }
    let code = cli::execute(cli, state).await;
    std::process::exit(code);
}
