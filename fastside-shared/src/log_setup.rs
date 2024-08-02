//! Perform basic configuration of logging.

use anyhow::Result;
use log::LevelFilter;
use pretty_env_logger::formatted_builder;
use std::env;

/// Configure logging.
///
/// # Panics
///
/// Function should not panic. On error, logging is just disabled.
pub fn configure_logging(log_level: &Option<String>) -> Result<()> {
    let mut builder = formatted_builder();

    if let Some(s) = log_level {
        builder.parse_filters(s);
    } else if let Ok(s) = env::var("FS__LOG") {
        builder.parse_filters(&s);
    } else {
        builder.filter_level(LevelFilter::Info);
    }

    builder.try_init()?;

    Ok(())
}
