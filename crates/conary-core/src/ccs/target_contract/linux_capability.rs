// conary-core/src/ccs/target_contract/linux_capability.rs
//! Closed Linux process-capability vocabulary for static target contracts.

use serde::{Deserialize, Serialize};

macro_rules! linux_capabilities {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum LinuxProcessCapabilityV1 {
            $(#[serde(rename = $name)] $variant),+
        }

        impl LinuxProcessCapabilityV1 {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            #[must_use]
            pub const fn all() -> &'static [Self] {
                Self::ALL
            }

            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

linux_capabilities! {
    Chown => "cap-chown",
    DacOverride => "cap-dac-override",
    DacReadSearch => "cap-dac-read-search",
    Fowner => "cap-fowner",
    Fsetid => "cap-fsetid",
    Kill => "cap-kill",
    Setgid => "cap-setgid",
    Setuid => "cap-setuid",
    Setpcap => "cap-setpcap",
    LinuxImmutable => "cap-linux-immutable",
    NetBindService => "cap-net-bind-service",
    NetBroadcast => "cap-net-broadcast",
    NetAdmin => "cap-net-admin",
    NetRaw => "cap-net-raw",
    IpcLock => "cap-ipc-lock",
    IpcOwner => "cap-ipc-owner",
    SysModule => "cap-sys-module",
    SysRawio => "cap-sys-rawio",
    SysChroot => "cap-sys-chroot",
    SysPtrace => "cap-sys-ptrace",
    SysPacct => "cap-sys-pacct",
    SysAdmin => "cap-sys-admin",
    SysBoot => "cap-sys-boot",
    SysNice => "cap-sys-nice",
    SysResource => "cap-sys-resource",
    SysTime => "cap-sys-time",
    SysTtyConfig => "cap-sys-tty-config",
    Mknod => "cap-mknod",
    Lease => "cap-lease",
    AuditWrite => "cap-audit-write",
    AuditControl => "cap-audit-control",
    Setfcap => "cap-setfcap",
    MacOverride => "cap-mac-override",
    MacAdmin => "cap-mac-admin",
    Syslog => "cap-syslog",
    WakeAlarm => "cap-wake-alarm",
    BlockSuspend => "cap-block-suspend",
    AuditRead => "cap-audit-read",
    Perfmon => "cap-perfmon",
    Bpf => "cap-bpf",
    CheckpointRestore => "cap-checkpoint-restore",
}

pub(super) fn validate_canonical(
    capabilities: &[LinuxProcessCapabilityV1],
    owner: &str,
) -> Result<(), String> {
    if capabilities.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!(
            "{owner} Linux process capabilities are repeated or not canonically ordered"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_vocabulary_matches_the_caps_crate() {
        use std::str::FromStr;

        for capability in LinuxProcessCapabilityV1::all() {
            let json = serde_json::to_string(capability).expect("serialize Linux capability");
            let name = json.trim_matches('"');
            let kernel_name = name.replace('-', "_").to_ascii_uppercase();
            let parsed = caps::Capability::from_str(&kernel_name)
                .expect("typed target capability must exist in caps crate");
            assert_eq!(
                parsed.to_string().to_ascii_lowercase().replace('_', "-"),
                name
            );
            assert_eq!(LinuxProcessCapabilityV1::from_name(name), Some(*capability));
        }
    }
}
