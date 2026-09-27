#![allow(dead_code)]

pub mod cli;

pub(crate) mod accessor;
pub(crate) mod dispatch;
pub(crate) mod fetcher;
pub(crate) mod filter;
pub(crate) mod mapper;
#[cfg(feature = "age319-private-broker-fixture")]
pub(crate) mod private_manual;
pub(crate) mod renderer;
pub(crate) mod row;
pub(crate) mod vendor;
