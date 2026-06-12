// conary-core/src/recipe/inference/mod.rs

//! Source recipe inference.

pub mod detectors;
pub mod materialize;
pub mod types;

pub use detectors::infer_recipe_from_path;
pub use materialize::{
    MaterializeOptions, render_recipe_toml, scaffold_named_recipe, write_recipe_toml,
};
pub use types::{BuildSystem, InferenceEvent, InferenceOptions, InferenceResult, InferenceTrace};
