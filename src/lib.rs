//! pgterm — library crate behind the `pgterm` binary, split out so
//! integration tests can drive the runner and app state directly.

pub mod cli;
pub mod config;
pub mod format;
pub mod health;
pub mod model;
pub mod runner;
pub mod sanitize;
