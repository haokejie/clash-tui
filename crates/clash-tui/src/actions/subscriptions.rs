use std::sync::Arc;

use anyhow::Result;

use crate::{state::AppState, subscriptions};

pub async fn update_due(state: Arc<AppState>) -> Result<subscriptions::SubscriptionSweep> {
    subscriptions::enqueue_due_profile_updates(state).await
}

pub async fn update_one(state: Arc<AppState>, uid: String) -> subscriptions::StartedProfileUpdateJob {
    subscriptions::start_profile_update_job(state, uid, None).await
}

pub async fn update_all(state: Arc<AppState>) -> Result<subscriptions::SubscriptionSweep> {
    subscriptions::enqueue_all_profile_updates(state).await
}

pub async fn status(state: &AppState) -> Result<subscriptions::SubscriptionStatus> {
    subscriptions::status(state).await
}
