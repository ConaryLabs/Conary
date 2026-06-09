// conary-core/src/generation/builder/activation.rs

pub enum GenerationActivation {
    /// Publish the generated DB snapshot as the active state immediately.
    ///
    /// Use only for paths that also publish/mount the generation in the same
    /// operation, such as composefs-native package mutation.
    Active,
    /// Leave the generated DB snapshot inactive until an explicit generation
    /// switch selects it for the next boot.
    Inactive,
}

impl GenerationActivation {
    pub(super) fn activates_state(self) -> bool {
        matches!(self, Self::Active)
    }
}
