// crates/conary-core/src/generation/verity_policy.rs

//! Boot verification policy for every Conary initramfs entry point.
//!
//! Images without the Conary binary use the conformance-tested shell adapter
//! in packaging/dracut/90conary/conary-verity.sh until initramfs consolidation.

use super::metadata::GenerationMetadata;

/// The last exact `conary.verity=` argument controls boot verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerityPolicy {
    Verified,
    ExplicitlyOff,
    Invalid { value: String },
}

/// A boot policy failure must not enter artifact repair or downgrade paths.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerityPolicyError {
    #[error("invalid conary.verity value '{value}'; expected on or off")]
    InvalidArgument { value: String },
    #[error("generation {generation} lacks the fs-verity metadata required for verified recovery")]
    MissingGenerationVerity { generation: i64 },
}

impl VerityPolicy {
    /// Preserve presence separately from the value; only absence defaults on.
    /// Arguments use the whitespace separators consumed by the initramfs shell.
    #[must_use]
    pub fn from_kernel_cmdline(cmdline: &str) -> Self {
        let mut value = None;
        for argument in cmdline.split([' ', '\t', '\n']) {
            if let Some(argument_value) = argument.strip_prefix("conary.verity=") {
                value = Some(argument_value);
            }
        }
        match value {
            None | Some("on") => Self::Verified,
            Some("off") => Self::ExplicitlyOff,
            Some(value) => Self::Invalid {
                value: value.to_owned(),
            },
        }
    }

    pub fn requires_verification(&self) -> Result<bool, VerityPolicyError> {
        match self {
            Self::Verified => Ok(true),
            Self::ExplicitlyOff => Ok(false),
            Self::Invalid { value } => Err(VerityPolicyError::InvalidArgument {
                value: value.clone(),
            }),
        }
    }

    /// Console warning for the only authorized verification downgrade.
    #[must_use]
    pub fn warning(&self) -> Option<&'static str> {
        match self {
            Self::ExplicitlyOff => {
                Some("conary: WARNING: conary.verity=off disables composefs fs-verity verification")
            }
            _ => None,
        }
    }

    /// Metadata supplies verification evidence; it never authorizes a downgrade.
    pub fn mount_requirements(
        &self,
        metadata: &GenerationMetadata,
    ) -> Result<(bool, Option<String>), VerityPolicyError> {
        if !self.requires_verification()? {
            return Ok((false, None));
        }
        match &metadata.erofs_verity_digest {
            Some(digest) if metadata.fsverity_enabled && !digest.is_empty() => {
                Ok((true, Some(digest.clone())))
            }
            _ => Err(VerityPolicyError::MissingGenerationVerity {
                generation: metadata.generation,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
