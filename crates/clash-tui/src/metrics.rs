use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use clash_mihomo::{MihomoClientConfig, SimpleMihomoClient};
use tokio::sync::{Mutex, watch};

use crate::mihomo_controller::{MemoryResponse, MihomoController, TrafficResponse};

const METRICS_RESOURCE_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const METRICS_CONTROLLER_TIMEOUT: Duration = Duration::from_secs(3);
const METRICS_STREAM_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const TRAFFIC_STREAM_STALE_TIMEOUT: Duration = Duration::from_secs(5);
const TRAFFIC_DISPLAY_HOLD: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControllerMetricsSnapshot {
    pub upload_speed: Option<u64>,
    pub download_speed: Option<u64>,
    pub memory: Option<u64>,
    pub sampled_at: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct ControllerMetricsManager {
    client: SimpleMihomoClient,
    sender: watch::Sender<ControllerMetricsSnapshot>,
    started: Arc<AtomicBool>,
}

impl ControllerMetricsManager {
    #[must_use]
    pub fn new(config: MihomoClientConfig) -> Self {
        let client = SimpleMihomoClient::new(config.with_timeout(METRICS_CONTROLLER_TIMEOUT));
        let (sender, _) = watch::channel(ControllerMetricsSnapshot::default());
        Self {
            client,
            sender,
            started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }

        let runtime = Arc::new(Mutex::new(ControllerMetricsRuntime::default()));

        let traffic_client = self.client.clone();
        let traffic_sender = self.sender.clone();
        let traffic_runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            collect_traffic_metrics(traffic_client, traffic_sender, traffic_runtime).await;
        });

        let resource_client = self.client.clone();
        let resource_sender = self.sender.clone();
        tokio::spawn(async move {
            collect_resource_metrics(resource_client, resource_sender, runtime).await;
        });
    }

    #[must_use]
    pub fn snapshot(&self) -> ControllerMetricsSnapshot {
        self.sender.borrow().clone()
    }
}

#[derive(Debug, Clone, Default)]
struct ControllerMetricsRuntime {
    snapshot: ControllerMetricsSnapshot,
    last_nonzero_traffic: Option<TrafficHold>,
}

#[derive(Debug, Clone, Copy)]
struct TrafficHold {
    upload: u64,
    download: u64,
    sampled_at: Instant,
}

#[derive(Debug, Clone)]
struct ControllerResourceSample {
    memory: Option<MemoryResponse>,
    sampled_at: Instant,
}

async fn collect_traffic_metrics(
    client: SimpleMihomoClient,
    sender: watch::Sender<ControllerMetricsSnapshot>,
    runtime: Arc<Mutex<ControllerMetricsRuntime>>,
) {
    loop {
        let controller = MihomoController::new(client.clone());
        match controller.traffic_stream().await {
            Ok(mut stream) => loop {
                match tokio::time::timeout(TRAFFIC_STREAM_STALE_TIMEOUT, stream.next()).await {
                    Ok(Some(Ok(traffic))) => {
                        update_runtime_snapshot(&runtime, &sender, |runtime| {
                            apply_traffic_response(runtime, traffic, Instant::now());
                        })
                        .await;
                    }
                    Ok(Some(Err(_))) | Ok(None) | Err(_) => {
                        update_runtime_snapshot(&runtime, &sender, |runtime| {
                            clear_traffic_metrics(runtime, Instant::now());
                        })
                        .await;
                        break;
                    }
                }
            },
            Err(_) => {
                update_runtime_snapshot(&runtime, &sender, |runtime| {
                    clear_traffic_metrics(runtime, Instant::now());
                })
                .await;
            }
        }
        tokio::time::sleep(METRICS_STREAM_RETRY_INTERVAL).await;
    }
}

async fn collect_resource_metrics(
    client: SimpleMihomoClient,
    sender: watch::Sender<ControllerMetricsSnapshot>,
    runtime: Arc<Mutex<ControllerMetricsRuntime>>,
) {
    let mut interval = tokio::time::interval(METRICS_RESOURCE_REFRESH_INTERVAL);
    loop {
        interval.tick().await;
        let sample = fetch_controller_resource_sample(&client).await;
        update_runtime_snapshot(&runtime, &sender, |runtime| {
            apply_resource_sample(runtime, sample);
        })
        .await;
    }
}

async fn update_runtime_snapshot<F>(
    runtime: &Arc<Mutex<ControllerMetricsRuntime>>,
    sender: &watch::Sender<ControllerMetricsSnapshot>,
    update: F,
) where
    F: FnOnce(&mut ControllerMetricsRuntime),
{
    let snapshot = {
        let mut runtime = runtime.lock().await;
        update(&mut runtime);
        runtime.snapshot.clone()
    };
    sender.send_replace(snapshot);
}

async fn fetch_controller_resource_sample(client: &SimpleMihomoClient) -> ControllerResourceSample {
    let controller = MihomoController::new(client.clone());
    let memory = controller.memory().await;
    ControllerResourceSample {
        memory: memory.ok(),
        sampled_at: Instant::now(),
    }
}

fn apply_traffic_response(runtime: &mut ControllerMetricsRuntime, traffic: TrafficResponse, sampled_at: Instant) {
    let upload_speed = traffic.up;
    let download_speed = traffic.down;
    let has_speed_fields = upload_speed.is_some() || download_speed.is_some();
    let upload_value = upload_speed.unwrap_or_default();
    let download_value = download_speed.unwrap_or_default();

    if upload_value > 0 || download_value > 0 {
        runtime.last_nonzero_traffic = Some(TrafficHold {
            upload: upload_value,
            download: download_value,
            sampled_at,
        });
        runtime.snapshot.upload_speed = upload_speed;
        runtime.snapshot.download_speed = download_speed;
    } else if has_speed_fields {
        if let Some(traffic_hold) = runtime.last_nonzero_traffic {
            if sampled_at.duration_since(traffic_hold.sampled_at) <= TRAFFIC_DISPLAY_HOLD {
                runtime.snapshot.upload_speed = Some(traffic_hold.upload);
                runtime.snapshot.download_speed = Some(traffic_hold.download);
            } else {
                runtime.last_nonzero_traffic = None;
                runtime.snapshot.upload_speed = upload_speed;
                runtime.snapshot.download_speed = download_speed;
            }
        } else {
            runtime.snapshot.upload_speed = upload_speed;
            runtime.snapshot.download_speed = download_speed;
        }
    } else {
        runtime.last_nonzero_traffic = None;
        runtime.snapshot.upload_speed = None;
        runtime.snapshot.download_speed = None;
    }
    runtime.snapshot.sampled_at = Some(sampled_at);
}

const fn clear_traffic_metrics(runtime: &mut ControllerMetricsRuntime, sampled_at: Instant) {
    runtime.last_nonzero_traffic = None;
    runtime.snapshot.upload_speed = None;
    runtime.snapshot.download_speed = None;
    runtime.snapshot.sampled_at = Some(sampled_at);
}

fn apply_resource_sample(runtime: &mut ControllerMetricsRuntime, sample: ControllerResourceSample) {
    let memory = sample.memory.as_ref().and_then(MemoryResponse::used_bytes);

    runtime.snapshot.memory = memory;
    runtime.snapshot.sampled_at = Some(sample.sampled_at);
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    #[test]
    fn metrics_snapshot_prefers_traffic_stream_speed() {
        let now = Instant::now();
        let mut runtime = ControllerMetricsRuntime::default();

        apply_traffic_response(
            &mut runtime,
            TrafficResponse {
                up: Some(12),
                down: Some(34),
                extra: Map::new(),
            },
            now,
        );
        apply_resource_sample(
            &mut runtime,
            ControllerResourceSample {
                memory: Some(MemoryResponse {
                    in_use: Some(4096),
                    os_limit: None,
                    extra: Map::new(),
                }),
                sampled_at: now,
            },
        );
        let snapshot = runtime.snapshot;

        assert_eq!(snapshot.upload_speed, Some(12));
        assert_eq!(snapshot.download_speed, Some(34));
        assert_eq!(snapshot.memory, Some(4096));
    }

    #[test]
    fn metrics_snapshot_does_not_fake_missing_metric_sources() {
        let mut runtime = ControllerMetricsRuntime::default();
        apply_resource_sample(
            &mut runtime,
            ControllerResourceSample {
                memory: None,
                sampled_at: Instant::now(),
            },
        );
        let snapshot = runtime.snapshot;

        assert_eq!(snapshot.upload_speed, None);
        assert_eq!(snapshot.download_speed, None);
        assert_eq!(snapshot.memory, None);
    }

    #[test]
    fn traffic_snapshot_holds_recent_nonzero_frame_for_tui_render() {
        let base = Instant::now();
        let mut runtime = ControllerMetricsRuntime::default();

        apply_traffic_response(
            &mut runtime,
            TrafficResponse {
                up: Some(2048),
                down: Some(8192),
                extra: Map::new(),
            },
            base,
        );
        apply_traffic_response(
            &mut runtime,
            TrafficResponse {
                up: Some(0),
                down: Some(0),
                extra: Map::new(),
            },
            base + Duration::from_millis(500),
        );

        assert_eq!(runtime.snapshot.upload_speed, Some(2048));
        assert_eq!(runtime.snapshot.download_speed, Some(8192));

        apply_traffic_response(
            &mut runtime,
            TrafficResponse {
                up: Some(0),
                down: Some(0),
                extra: Map::new(),
            },
            base + TRAFFIC_DISPLAY_HOLD + Duration::from_millis(1),
        );

        assert_eq!(runtime.snapshot.upload_speed, Some(0));
        assert_eq!(runtime.snapshot.download_speed, Some(0));
    }
}
