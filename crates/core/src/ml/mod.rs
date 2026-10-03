//! Small traditional-ML toolkit that learns from one user's history. Every model is a prior
//! blended with data and reports an honest `ModelCard`. Deterministic, serde-serialisable.
pub mod core;
pub mod fit_model;
pub mod jd_classifier;
pub mod neighbors;
pub mod outcome;
pub mod pii_candidate;
pub mod ranker;
pub mod recommend;
pub mod registry;
pub mod rewrite_need;

pub use registry::ModelBundle;
