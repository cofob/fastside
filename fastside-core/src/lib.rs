pub mod crawler;
pub mod errors;
pub mod filters;
pub mod routes;
pub mod search;
pub mod types;
pub mod utils;

#[deny(unused_imports)]
#[deny(unused_variables)]
#[deny(unused_mut)]
#[deny(unsafe_code)]
// Dependencies
#[macro_use]
extern crate log;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
