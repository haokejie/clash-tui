#![cfg_attr(test, allow(clippy::expect_used))]

pub mod client;
pub mod models;

pub use client::{
    ControllerEndpoint, MihomoClient, MihomoClientConfig, MihomoHttpMethod, MihomoJsonStream, MihomoResponse,
    SimpleMihomoClient,
};
pub use models::{MihomoHealth, MihomoVersion};
