use std::collections::HashMap;

use fastside_shared::serde_types::{Instance, Service, ServicesData};
use serde::{Deserialize, Serialize};
use url::Url;

/// SingleInstancePing is a single ping for a instance in history.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SingleInstancePing {
    pub timestamp: u64,
    pub success: bool,
}

impl SingleInstancePing {
    /// Create a new ping with given timestamp.
    pub fn new(timestamp: u64, success: bool) -> Self {
        Self { timestamp, success }
    }

    /// Create a new ping with current timestamp.
    pub fn now(success: bool) -> Self {
        Self::new(chrono::Utc::now().timestamp() as u64, success)
    }
}

impl From<bool> for SingleInstancePing {
    fn from(success: bool) -> Self {
        Self::now(success)
    }
}

/// PingHistory is a list of pings for a single instance
///
/// It is used to calculate uptime.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PingHistory(Vec<SingleInstancePing>);

impl PingHistory {
    pub fn cleanup(&mut self) {
        // Remove pings older than 7 days
        let min_timestamp = (chrono::Utc::now() - chrono::Duration::days(7)).timestamp() as u64;
        self.0.retain(|p| p.timestamp > min_timestamp);
    }

    pub fn uptime(&self) -> u8 {
        let total_pings = self.0.len();
        let successful_pings = self.0.iter().filter(|p| p.success).count();
        if total_pings == 0 {
            100
        } else {
            ((successful_pings as f64 / total_pings as f64 * 100.0) as u8).clamp(0, 100)
        }
    }

    pub fn push_ping(&mut self, ping: impl Into<SingleInstancePing>) {
        self.0.push(ping.into());
    }

    /// Check if PingHistory have enough pings to be considered ready
    pub fn is_ready(&self) -> bool {
        self.0.len() >= 50
    }
}

/// InstanceHistory is a history of pings for an instance.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InstanceHistory {
    pub url: Url,
    pub ping_history: PingHistory,
}

impl From<&Instance> for InstanceHistory {
    fn from(instance: &Instance) -> Self {
        Self {
            url: instance.url.clone(),
            ping_history: PingHistory(Vec::new()),
        }
    }
}

impl From<Url> for InstanceHistory {
    fn from(url: Url) -> Self {
        Self {
            url,
            ping_history: PingHistory(Vec::new()),
        }
    }
}

/// ServiceHistory is a history of instances for a service.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ServiceHistory {
    #[serde(default)]
    pub instances: Vec<InstanceHistory>,
}

const MIN_UPTIME: u8 = 30;

impl ServiceHistory {
    pub fn get_instance(&self, url: &Url) -> Option<&InstanceHistory> {
        self.instances.iter().find(|i| &i.url == url)
    }

    pub fn get_instance_mut(&mut self, url: &Url) -> Option<&mut InstanceHistory> {
        self.instances.iter_mut().find(|i| &i.url == url)
    }

    pub fn add_instance(&mut self, instance: impl Into<InstanceHistory>) {
        self.instances.push(instance.into());
    }

    /// Remove instances that are not in the list
    pub fn remove_removed_instances(&mut self, instances: &[Instance]) {
        self.instances
            .retain(|i| instances.iter().any(|instance| i.url == instance.url));
    }

    /// Remove instances with uptime lower than 30%
    ///
    /// Returns a list of removed instances.
    pub fn remove_dead_instances(&self, service: &mut Service) -> Vec<Url> {
        let mut dead_instances = Vec::new();
        for instance in &self.instances {
            if instance.ping_history.is_ready() && instance.ping_history.uptime() < MIN_UPTIME {
                debug!("Removing dead instance: {}", instance.url);
                dead_instances.push(instance.url.clone());
            }
        }
        service
            .instances
            .retain(|i| !dead_instances.contains(&i.url));
        dead_instances
    }
}

/// ActualizerData is a history of services availability.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ActualizerData {
    pub services: HashMap<String, ServiceHistory>,
}

impl ActualizerData {
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
        }
    }

    pub fn remove_removed_services(&mut self, services: &ServicesData) {
        let service_names: Vec<&String> = services.keys().collect();
        self.services
            .retain(|name, _| service_names.contains(&name));
    }

    pub fn remove_removed_instances(&mut self, services: &ServicesData) {
        for (name, service) in services {
            if let Some(service_history) = self.services.get_mut(name) {
                service_history.remove_removed_instances(&service.instances);
            }
        }
    }

    pub fn remove_dead_instances(&self, services: &mut ServicesData) -> Vec<Url> {
        let mut dead_instances = Vec::new();
        for (name, service) in services {
            if let Some(service_history) = self.services.get(name) {
                dead_instances.extend(service_history.remove_dead_instances(service));
            }
        }
        dead_instances
    }
}

impl Default for ActualizerData {
    fn default() -> Self {
        Self::new()
    }
}
