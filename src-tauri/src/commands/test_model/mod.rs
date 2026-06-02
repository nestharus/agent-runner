//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/test_model/mod.rs
//!     role: adapter
//!     Translates:
//!       - test_model
//!       - Tauri IPC
//!       - executor service output
//!       - provider quota mutation
//!       - TestModelResult
//!   - component: src-tauri/src/commands/test_model/lookup.rs
//!     role: adapter
//!     Translates:
//!       - model configuration lookup contract
//!       - state database opener contract
//!       - providers configuration repository contract
//!       - effective-provider source lookup contract
//!   - component: src-tauri/src/commands/test_model/dispatch.rs
//!     role: adapter
//!     Translates:
//!       - routing service contract
//!       - executor service contract
//!       - provider quota repository contract
//!       - diagnostics service contract
//!   - component: src-tauri/src/commands/test_model/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC test_model request contract
//!       - test_model lookup helper surface
//!       - test_model dispatch helper surface
//!       - test_model mapping helper surface
//!       - test_model validation and formatting helper surface
//!   - component: src-tauri/src/commands/test_model/mapper.rs
//!     role: adapter
//!     Translates:
//!       - test_model service-bundle mapping contract
//!       - diagnostics service contract
//!       - executor effective request contract
//!       - executor result to TestModelResult IPC contract
//!       - effective-provider fallback tuple contract
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/test_model/validator.rs
//!     role: intrinsic-surface
//!     Domain: test_model_exhaustion_and_provider_validation
//!     Owns:
//!       - provider-index validation
//!       - diagnostics output variant validation
//!       - terminal signal quota-exhausted predicate
//!       - diagnostics quota-exhausted predicate
//!       - quota-marking predicate
//!       - model command provider fallback predicate
//! ```

pub mod diagnostics_fallback;
pub(crate) mod dispatch;
mod formatter;
pub(crate) mod lookup;
mod mapper;
pub(crate) mod orchestration;
pub mod validator;

pub use mapper::TestModelResult;
pub use mapper::TestModelServices;
pub use orchestration::effective_provider_for_model_provider;
pub use orchestration::{test_model_for_test, test_model_with_db_path};
