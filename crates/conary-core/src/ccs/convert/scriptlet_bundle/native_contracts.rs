// conary-core/src/ccs/convert/scriptlet_bundle/native_contracts.rs

use crate::ccs::legacy_scriptlets::{LifecyclePath, NativeInvocation, TransactionOrder};
use crate::packages::native_abi::{
    NativeArgumentContract, NativeArgumentValue, NativeInvocationContract, NativeLifecyclePath,
    NativeRootExpectation, NativeScriptletBody, NativeScriptletBodyEncoding, NativeScriptletEntry,
    NativeScriptletKind, NativeStdinContract, NativeTransactionOrder, NativeTransactionPosition,
};
use crate::packages::traits::ScriptletPhase;
use std::collections::BTreeMap;

pub(super) fn encoded_native_body(body: &NativeScriptletBody) -> (String, Option<String>) {
    match body.encoding {
        NativeScriptletBodyEncoding::Utf8 => (
            body.text
                .clone()
                .unwrap_or_else(|| String::from_utf8_lossy(&body.bytes).into_owned()),
            None,
        ),
        NativeScriptletBodyEncoding::Binary => {
            use base64::Engine as _;
            (
                base64::engine::general_purpose::STANDARD.encode(&body.bytes),
                Some("base64".to_string()),
            )
        }
    }
}

pub(super) fn native_invocation(invocation: &NativeInvocationContract) -> NativeInvocation {
    NativeInvocation {
        args: invocation
            .args
            .iter()
            .map(native_argument_contract)
            .collect(),
        environment: invocation
            .environment
            .iter()
            .map(|fact| match &fact.value {
                Some(value) => format!("{}={value}", fact.name),
                None => fact.name.clone(),
            })
            .collect(),
        stdin: native_stdin(invocation.stdin).map(str::to_string),
        chroot: Some(native_root(invocation.root).to_string()),
        extra: BTreeMap::new(),
    }
}

pub(super) fn native_transaction_order(order: &NativeTransactionOrder) -> TransactionOrder {
    TransactionOrder {
        position: native_transaction_position(order.position).to_string(),
        before: Vec::new(),
        after: order.relative_to.iter().cloned().collect(),
        extra: BTreeMap::new(),
    }
}

pub(super) fn flat_transaction_order(phase: ScriptletPhase) -> TransactionOrder {
    let position = match phase {
        ScriptletPhase::PreInstall
        | ScriptletPhase::PreUpgrade
        | ScriptletPhase::PreRemove
        | ScriptletPhase::PreTransaction => "before-payload",
        ScriptletPhase::PostInstall
        | ScriptletPhase::PostUpgrade
        | ScriptletPhase::PostRemove
        | ScriptletPhase::PostTransaction => "after-payload",
        ScriptletPhase::Trigger => "trigger",
    };
    TransactionOrder {
        position: position.to_string(),
        before: Vec::new(),
        after: Vec::new(),
        extra: BTreeMap::new(),
    }
}

pub(super) fn phase_from_scriptlet_phase(phase: ScriptletPhase) -> LifecyclePath {
    match phase {
        ScriptletPhase::PreInstall => LifecyclePath::PreInstall,
        ScriptletPhase::PostInstall => LifecyclePath::PostInstall,
        ScriptletPhase::PreRemove => LifecyclePath::PreRemove,
        ScriptletPhase::PostRemove => LifecyclePath::PostRemove,
        ScriptletPhase::PreUpgrade => LifecyclePath::PreUpgrade,
        ScriptletPhase::PostUpgrade => LifecyclePath::PostUpgrade,
        ScriptletPhase::PreTransaction => LifecyclePath::PreTransaction,
        ScriptletPhase::PostTransaction => LifecyclePath::PostTransaction,
        ScriptletPhase::Trigger => LifecyclePath::Trigger,
    }
}

pub(super) fn phase_from_native_lifecycle(path: NativeLifecyclePath) -> LifecyclePath {
    match path {
        NativeLifecyclePath::PreInstall => LifecyclePath::PreInstall,
        NativeLifecyclePath::PostInstall | NativeLifecyclePath::Config => {
            LifecyclePath::PostInstall
        }
        NativeLifecyclePath::PreUpgrade => LifecyclePath::PreUpgrade,
        NativeLifecyclePath::PostUpgrade => LifecyclePath::PostUpgrade,
        NativeLifecyclePath::PreRemove => LifecyclePath::PreRemove,
        NativeLifecyclePath::PostRemove
        | NativeLifecyclePath::Purge
        | NativeLifecyclePath::Abort => LifecyclePath::PostRemove,
        NativeLifecyclePath::PreTransaction | NativeLifecyclePath::PreUntransaction => {
            LifecyclePath::PreTransaction
        }
        NativeLifecyclePath::PostTransaction | NativeLifecyclePath::PostUntransaction => {
            LifecyclePath::PostTransaction
        }
        NativeLifecyclePath::Verify | NativeLifecyclePath::Trigger => LifecyclePath::Trigger,
        NativeLifecyclePath::FileTrigger | NativeLifecyclePath::TransactionFileTrigger => {
            LifecyclePath::FileTrigger
        }
    }
}

pub(super) fn native_lifecycle_paths(native: &NativeScriptletEntry) -> Vec<String> {
    let paths = if native.lifecycle_paths.is_empty() {
        vec![native.primary_lifecycle]
    } else {
        native.lifecycle_paths.clone()
    };
    paths
        .into_iter()
        .map(|path| phase_from_native_lifecycle(path).as_str().to_string())
        .collect()
}

pub(super) fn non_empty_or_default(value: &str, default: &str) -> String {
    if value.trim().is_empty() {
        default.to_string()
    } else {
        value.to_string()
    }
}

fn native_argument_contract(argument: &NativeArgumentContract) -> String {
    format!(
        "{}:{}={}",
        argument.index,
        argument.name,
        native_argument_value(&argument.value)
    )
}

fn native_argument_value(value: &NativeArgumentValue) -> String {
    match value {
        NativeArgumentValue::Action => "action".to_string(),
        NativeArgumentValue::OldVersion => "old-version".to_string(),
        NativeArgumentValue::NewVersion => "new-version".to_string(),
        NativeArgumentValue::PackageInstanceCount => "package-instance-count".to_string(),
        NativeArgumentValue::PackageName => "package-name".to_string(),
        NativeArgumentValue::TriggerName => "trigger-name".to_string(),
        NativeArgumentValue::TriggerNames => "trigger-names".to_string(),
        NativeArgumentValue::TriggerCount => "trigger-count".to_string(),
        NativeArgumentValue::FilePath => "file-path".to_string(),
        NativeArgumentValue::InstalledVersion => "installed-version".to_string(),
        NativeArgumentValue::Raw(value) => format!("raw:{value}"),
    }
}

pub(super) fn native_stdin(stdin: NativeStdinContract) -> Option<&'static str> {
    match stdin {
        NativeStdinContract::None => None,
        NativeStdinContract::Debconf => Some("debconf"),
        NativeStdinContract::Paths => Some("paths"),
        NativeStdinContract::Unknown => Some("unknown"),
    }
}

fn native_root(root: NativeRootExpectation) -> &'static str {
    match root {
        NativeRootExpectation::PackageManagerDefault => "package-manager-default",
        NativeRootExpectation::InstallRoot => "install-root",
        NativeRootExpectation::HostRoot => "host-root",
        NativeRootExpectation::Unknown => "unknown",
    }
}

pub(super) fn native_transaction_position(position: NativeTransactionPosition) -> &'static str {
    match position {
        NativeTransactionPosition::BeforePayload => "before-payload",
        NativeTransactionPosition::AfterPayload => "after-payload",
        NativeTransactionPosition::BeforeTransaction => "before-transaction",
        NativeTransactionPosition::AfterTransaction => "after-transaction",
        NativeTransactionPosition::Untransaction => "untransaction",
        NativeTransactionPosition::Verification => "verification",
        NativeTransactionPosition::Trigger => "trigger",
        NativeTransactionPosition::ControlArtifact => "control-artifact",
    }
}

pub(super) fn native_scriptlet_kind(kind: NativeScriptletKind) -> &'static str {
    match kind {
        NativeScriptletKind::Executable => "executable",
        NativeScriptletKind::ControlArtifact => "control-artifact",
    }
}
