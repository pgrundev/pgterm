//! pgterm — library crate behind the `pgterm` binary, split out so
//! integration tests can drive the runner and app state directly.

pub mod action;
pub mod app;
pub mod cli;
pub mod config;
pub mod event;
pub mod format;
pub mod health;
pub mod model;
pub mod runner;
pub mod sanitize;
pub mod screens;
pub mod ui;
