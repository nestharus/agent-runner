//! CLI input resolution and config path derivation.
//!
//! Role-split per the `usage/` directory-module template: `inputs` owns
//! prompt/stdin/answer resolution; `paths` owns models/agents/config dir
//! derivation. Relocated out of `main.rs` (AGE-207, AGE-183 program slice B12).

pub(crate) mod inputs;
pub(crate) mod paths;
