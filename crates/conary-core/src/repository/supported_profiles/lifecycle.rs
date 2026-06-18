// conary-core/src/repository/supported_profiles/lifecycle.rs

use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

use super::types::{LifecyclePolicyDocument, LifecyclePolicyMode, SupportedProfile};

fn match_policy(
    policy: &LifecyclePolicyDocument,
    value: &str,
    use_keys: bool,
) -> ProfileConstraintStatus {
    match policy.mode {
        LifecyclePolicyMode::Unsupported => ProfileConstraintStatus::Unsupported,
        LifecyclePolicyMode::AllowList => {
            let values = if use_keys {
                &policy.keys
            } else {
                &policy.entries
            };
            if values.iter().any(|entry| entry == value) {
                ProfileConstraintStatus::Accepted
            } else {
                ProfileConstraintStatus::Unsupported
            }
        }
    }
}

impl TargetProfileQuery for SupportedProfile {
    fn service_status(&self, service: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().services, service, false)
    }

    fn tmpfiles_status(&self, entry: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().tmpfiles, entry, false)
    }

    fn sysctl_status(&self, key: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().sysctl, key, true)
    }

    fn user_status(&self, user: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().users, user, false)
    }

    fn group_status(&self, group: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().groups, group, false)
    }

    fn directory_status(&self, directory: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().directories, directory, false)
    }

    fn alternative_status(&self, alternative: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().alternatives, alternative, false)
    }
}
