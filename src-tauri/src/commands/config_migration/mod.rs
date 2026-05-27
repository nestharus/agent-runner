#![allow(dead_code)]

pub(crate) mod accessor;
pub(crate) mod dispatch;
pub(crate) mod filter;
pub(crate) mod formatter;
pub(crate) mod mapper;
pub(crate) mod orchestration;
pub(crate) mod parser;
pub(crate) mod predicate;
pub(crate) mod validator;

#[cfg(test)]
#[path = "tests.rs"]
mod config_migration_tests;

pub(crate) use dispatch::run_migrate_config;
