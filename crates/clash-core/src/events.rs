use serde::{Deserialize, Serialize};

use crate::kernel::KernelSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "kebab-case")]
pub enum CoreEvent {
    KernelStateChanged { kernel: KernelSnapshot },
    JobCreated { job_id: String, name: String },
    JobProgress { job_id: String, message: String },
    JobFinished { job_id: String },
    JobFailed { job_id: String, message: String },
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: CoreEvent);
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: CoreEvent) {}
}
