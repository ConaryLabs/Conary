// conary-core/src/recipe/inference/mod.rs

//! Source recipe inference.

pub mod detectors;
pub mod types;

pub use detectors::infer_recipe_from_path;
pub use types::{BuildSystem, InferenceEvent, InferenceOptions, InferenceResult, InferenceTrace};
