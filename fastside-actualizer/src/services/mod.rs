mod default;
mod searx;
mod searxng;

use crate::types::ServiceUpdater;

pub use default::DefaultInstanceChecker;

/// Get a service updater by name.
pub fn get_service_updater(name: &str) -> Option<Box<dyn ServiceUpdater>> {
    match name {
        "searx" => Some(Box::new(searx::SearxUpdater::new())),
        "searxng" => Some(Box::new(searxng::SearxngUpdater::new())),
        _ => None,
    }
}

/// Get an instance checker by name.
#[allow(clippy::match_single_binding)]
pub fn get_instance_checker(
    name: &str,
) -> Box<(dyn crate::types::InstanceChecker + Send + Sync + 'static)> {
    match name {
        _ => Box::new(DefaultInstanceChecker::new()),
    }
}
