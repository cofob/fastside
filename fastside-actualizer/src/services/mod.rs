mod searx;
mod default_checker;

use crate::types::FullServiceUpdater;

pub use default_checker::DefaultInstanceChecker;

pub fn get_service_updater(name: &str) -> Option<Box<dyn FullServiceUpdater>> {
    match name {
        "searx" => Some(Box::new(searx::SearxUpdater::default())),
        _ => None,
    }
}
