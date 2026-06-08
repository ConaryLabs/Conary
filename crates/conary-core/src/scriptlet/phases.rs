// conary-core/src/scriptlet/phases.rs

use crate::packages::traits::ScriptletPhase;

/// Convert ScriptletPhase to string for database storage
pub fn phase_to_string(phase: ScriptletPhase) -> String {
    phase.to_string()
}

/// Parse phase string back to ScriptletPhase
pub fn phase_from_string(s: &str) -> Option<ScriptletPhase> {
    match s {
        "pre-install" => Some(ScriptletPhase::PreInstall),
        "post-install" => Some(ScriptletPhase::PostInstall),
        "pre-remove" => Some(ScriptletPhase::PreRemove),
        "post-remove" => Some(ScriptletPhase::PostRemove),
        "pre-upgrade" => Some(ScriptletPhase::PreUpgrade),
        "post-upgrade" => Some(ScriptletPhase::PostUpgrade),
        "pre-transaction" => Some(ScriptletPhase::PreTransaction),
        "post-transaction" => Some(ScriptletPhase::PostTransaction),
        "trigger" => Some(ScriptletPhase::Trigger),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{phase_from_string, phase_to_string};
    use crate::packages::traits::ScriptletPhase;

    #[test]
    fn test_phase_conversion() {
        assert_eq!(phase_to_string(ScriptletPhase::PreInstall), "pre-install");
        assert_eq!(
            phase_from_string("pre-install"),
            Some(ScriptletPhase::PreInstall)
        );
        assert_eq!(phase_from_string("invalid"), None);
    }
}
