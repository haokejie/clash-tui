use std::{sync::Arc, time::Instant};

use anyhow::Result;
use clash_core::{ConfigLoadResult, ConfigStore, RuntimeConfigGenerator};
use clash_mihomo::{MihomoClientConfig, SimpleMihomoClient};
use tokio::sync::{Mutex, RwLock};

use crate::{
    jobs::JobManager,
    kernel::{KernelManager, KernelProcessConfig, controller_client_config},
    metrics::ControllerMetricsManager,
    options::ClashTuiOptions,
};

pub struct AppState {
    pub options: ClashTuiOptions,
    pub store: ConfigStore,
    pub config: RwLock<ConfigLoadResult>,
    pub runtime: RuntimeConfigGenerator,
    pub kernel: KernelManager,
    pub mihomo: SimpleMihomoClient,
    pub mihomo_config: MihomoClientConfig,
    pub metrics: ControllerMetricsManager,
    pub jobs: JobManager,
    pub profile_switch_lock: Mutex<()>,
    pub started_at: Instant,
}

impl AppState {
    pub async fn initialize(options: ClashTuiOptions) -> Result<Arc<Self>> {
        let app_paths = options.app_paths();
        let job_history_path = app_paths.home_dir.join("jobs.json");
        let store = ConfigStore::new(app_paths);
        let config = store.initialize().await?;
        let runtime = RuntimeConfigGenerator::from_loaded(&config);
        let kernel_config = KernelProcessConfig::from_options(&options, &config.paths, config.clash_info().secret);
        let mihomo_config = controller_client_config(&kernel_config);
        let mihomo = SimpleMihomoClient::new(mihomo_config.clone());
        let metrics = ControllerMetricsManager::new(mihomo_config.clone());
        let jobs = JobManager::with_history_file(job_history_path).await;
        let kernel = KernelManager::with_events(kernel_config, runtime.clone(), jobs.clone());
        kernel.spawn_health_monitor();

        Ok(Arc::new(Self {
            options,
            store,
            config: RwLock::new(config),
            runtime,
            kernel,
            mihomo,
            mihomo_config,
            metrics,
            jobs,
            profile_switch_lock: Mutex::new(()),
            started_at: Instant::now(),
        }))
    }
}
