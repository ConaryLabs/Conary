// conary-core/src/generation/builder/file_capabilities.rs

use std::collections::BTreeSet;

use crate::ccs::manifest::FileCapability;

pub(crate) use crate::ccs::manifest::LINUX_FILE_CAPABILITY_NAMES;

pub(crate) const SECURITY_CAPABILITY_XATTR: &str = "security.capability";

const VFS_CAP_REVISION_2: u32 = 0x02000000;
const VFS_CAP_FLAGS_EFFECTIVE: u32 = 0x00000001;
const VFS_CAP_U32_COUNT: usize = 2;

pub(crate) fn encode_security_capability_xattr(
    capability: &FileCapability,
) -> crate::Result<Vec<u8>> {
    let capability = normalized_capability(capability)?;
    let mut permitted = [0_u32; VFS_CAP_U32_COUNT];
    let inheritable = [0_u32; VFS_CAP_U32_COUNT];

    if capability.permitted {
        for name in &capability.capabilities {
            let index = LINUX_FILE_CAPABILITY_NAMES
                .iter()
                .position(|candidate| candidate == name)
                .ok_or_else(|| {
                    crate::Error::Capability(format!(
                        "unknown Linux file capability '{}' for {}",
                        name, capability.path
                    ))
                })?;
            let word_index = index / 32;
            let bit_index = index % 32;
            permitted[word_index] |= 1_u32 << bit_index;
        }
    }

    let mut magic_etc = VFS_CAP_REVISION_2;
    if capability.effective {
        magic_etc |= VFS_CAP_FLAGS_EFFECTIVE;
    }

    let mut encoded = Vec::with_capacity(20);
    encoded.extend_from_slice(&magic_etc.to_le_bytes());
    for index in 0..VFS_CAP_U32_COUNT {
        encoded.extend_from_slice(&permitted[index].to_le_bytes());
        encoded.extend_from_slice(&inheritable[index].to_le_bytes());
    }
    Ok(encoded)
}

fn normalized_capability(capability: &FileCapability) -> crate::Result<FileCapability> {
    let normalized = FileCapability {
        path: capability.path.clone(),
        capabilities: capability
            .capabilities
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        permitted: capability.permitted,
        effective: capability.effective,
        inheritable: capability.inheritable,
    };
    normalized
        .validate()
        .map_err(|error| crate::Error::Capability(error.to_string()))?;
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use crate::ccs::manifest::FileCapability;

    use super::{
        LINUX_FILE_CAPABILITY_NAMES, SECURITY_CAPABILITY_XATTR, encode_security_capability_xattr,
    };

    fn file_capability(path: &str, capabilities: &[&str]) -> FileCapability {
        FileCapability {
            path: path.to_string(),
            capabilities: capabilities
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            permitted: true,
            effective: true,
            inheritable: false,
        }
    }

    #[test]
    fn linux_capability_name_order_matches_kernel_indices() {
        assert_eq!(SECURITY_CAPABILITY_XATTR, "security.capability");
        assert_eq!(LINUX_FILE_CAPABILITY_NAMES[10], "cap_net_bind_service");
        assert_eq!(LINUX_FILE_CAPABILITY_NAMES[39], "cap_bpf");
    }

    #[test]
    fn net_bind_service_sets_permitted_bit_and_effective_flag() {
        let encoded = encode_security_capability_xattr(&file_capability(
            "/usr/bin/server",
            &["cap_net_bind_service"],
        ))
        .expect("encode capability xattr");

        assert_eq!(encoded.len(), 20);
        assert_eq!(
            u32::from_le_bytes(encoded[0..4].try_into().unwrap()),
            0x02000001
        );
        assert_eq!(
            u32::from_le_bytes(encoded[4..8].try_into().unwrap()),
            1_u32 << 10
        );
        assert_eq!(u32::from_le_bytes(encoded[8..12].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(encoded[12..16].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(encoded[16..20].try_into().unwrap()), 0);
    }

    #[test]
    fn high_capability_lands_in_second_permitted_word() {
        let encoded =
            encode_security_capability_xattr(&file_capability("/usr/bin/tracer", &["cap_bpf"]))
                .expect("encode capability xattr");

        assert_eq!(u32::from_le_bytes(encoded[4..8].try_into().unwrap()), 0);
        assert_eq!(
            u32::from_le_bytes(encoded[12..16].try_into().unwrap()),
            1_u32 << 7
        );
    }

    #[test]
    fn invalid_effective_without_permitted_is_rejected() {
        let mut capability = file_capability("/usr/bin/server", &["cap_net_bind_service"]);
        capability.permitted = false;
        capability.effective = true;

        let error = encode_security_capability_xattr(&capability)
            .expect_err("effective without permitted must be rejected");

        assert!(error.to_string().contains("requires permitted"));
    }

    #[test]
    fn inheritable_capability_is_rejected() {
        let mut capability = file_capability("/usr/bin/server", &["cap_net_bind_service"]);
        capability.inheritable = true;

        let error = encode_security_capability_xattr(&capability)
            .expect_err("inheritable capabilities must be rejected");

        assert!(error.to_string().contains("inheritable"));
    }

    #[test]
    fn duplicate_capability_names_are_normalized() {
        let encoded = encode_security_capability_xattr(&file_capability(
            "/usr/bin/server",
            &[
                "cap_net_raw",
                "cap_net_bind_service",
                "cap_net_raw",
                "cap_net_bind_service",
            ],
        ))
        .expect("encode capability xattr");

        assert_eq!(
            u32::from_le_bytes(encoded[4..8].try_into().unwrap()),
            (1_u32 << 10) | (1_u32 << 13)
        );
    }
}
