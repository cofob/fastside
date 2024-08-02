use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SingleInstancePing {
    pub timestamp: u64,
    pub success: bool,
}

impl SingleInstancePing {
    pub fn new(timestamp: u64, success: bool) -> Self {
        Self { timestamp, success }
    }

    pub fn now(success: bool) -> Self {
        Self::new(chrono::Utc::now().timestamp() as u64, success)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PingHistory(Vec<SingleInstancePing>);

impl PingHistory {
    pub fn cleanup(&mut self, max_size: usize) {
        if self.0.len() > max_size {
            self.0 = self.0.split_off(self.0.len() - max_size);
        }
    }

    pub fn uptime(&self) -> f64 {
        let total_pings = self.0.len();
        let successful_pings = self.0.iter().filter(|p| p.success).count();
        successful_pings as f64 / total_pings as f64
    }

    pub fn push_ping(&mut self, ping: SingleInstancePing) {
        self.0.push(ping);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InstanceHistory {
    pub url: Url,
    pub ping_history: PingHistory,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ServiceHistory {
    pub instances: Vec<InstanceHistory>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ServicesHistory {
    pub services: HashMap<String, ServiceHistory>,
}
