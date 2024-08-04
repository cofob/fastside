use anyhow::Result;
use async_trait::async_trait;
use fastside_shared::serde_types::{Instance, Service};
use reqwest::Client;

#[async_trait]
pub trait ServiceUpdater {
    /// Update the list of instances.
    ///
    /// Checks public list of instances and adds new entries to the list.
    ///
    /// # Arguments
    ///
    /// * `current_instances` - The current list of instances.
    ///
    /// # Returns
    ///
    /// The updated list of instances.
    async fn update(&self, client: Client, current_instances: &[Instance])
        -> Result<Vec<Instance>>;
}

#[async_trait]
pub trait InstanceChecker {
    /// Check single instance.
    async fn check(&self, client: Client, service: &Service, instance: &Instance) -> Result<bool>;
}
