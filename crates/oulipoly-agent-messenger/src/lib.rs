//! ## Declared roles
//!
//! - accessor
//! - orchestration
//!
//! Public facade for the agent-messenger crate.

mod address;
mod channel;
pub mod cli;
mod error;
mod formatter;
mod mapper;
mod model;
mod name;
mod repository;
mod service;
mod source;
mod validator;

pub use channel::append_return_channel;
pub use error::MessengerError;
pub use model::{
    ListReturnedRequest, ReturnRequest, ReturnSource, ReturnedArtifact, ReturnedArtifactMeta,
    ReturnedArtifactRecord, ReturnedArtifactRef, ReturnedArtifactSource, ShowReturnedRequest,
    StoreAddress,
};
pub use name::ReturnName;
pub use service::{list_returned, return_artifact, show_returned};

#[cfg(test)]
mod tests {
    use super::MessengerError;
    use oulipoly_agent_store::StoreError;

    #[test]
    fn store_incompatible_schema_version_is_preserved() {
        let err = MessengerError::from(StoreError::IncompatibleSchema("2".to_string()));

        assert!(matches!(
            err,
            MessengerError::IncompatibleSchema(ref message) if message == "version 2"
        ));
        assert_eq!(err.to_string(), "incompatible database schema: version 2");
    }
}
