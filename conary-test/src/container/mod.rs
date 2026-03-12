// conary-test/src/container/mod.rs

pub mod backend;
pub mod image;
pub mod lifecycle;

pub use backend::{ContainerBackend, ContainerConfig, ContainerId, ExecResult, VolumeMount};
pub use image::build_distro_image;
pub use lifecycle::BollardBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn container_config_defaults() {
        let cfg = ContainerConfig::default();
        assert!(cfg.image.is_empty());
        assert!(cfg.env.is_empty());
        assert!(cfg.volumes.is_empty());
        assert!(!cfg.privileged);
        assert_eq!(cfg.network_mode, "bridge");
        assert!(cfg.tmpfs.is_empty());
        assert_eq!(cfg.memory_limit, None);
    }

    #[test]
    fn container_config_with_values() {
        let mut env = HashMap::new();
        env.insert(
            "REMI_ENDPOINT".to_string(),
            "https://example.com".to_string(),
        );

        let cfg = ContainerConfig {
            image: "alpine:latest".to_string(),
            env,
            volumes: vec![VolumeMount {
                host_path: "/tmp/data".to_string(),
                container_path: "/data".to_string(),
                read_only: true,
            }],
            privileged: true,
            network_mode: "host".to_string(),
            tmpfs: HashMap::from([("/var/lib/conary".to_string(), "size=50m".to_string())]),
            memory_limit: Some(512 * 1024 * 1024),
        };

        assert_eq!(cfg.image, "alpine:latest");
        assert_eq!(cfg.env.len(), 1);
        assert_eq!(cfg.volumes.len(), 1);
        assert!(cfg.volumes[0].read_only);
        assert!(cfg.privileged);
        assert_eq!(cfg.network_mode, "host");
        assert_eq!(
            cfg.tmpfs.get("/var/lib/conary").map(String::as_str),
            Some("size=50m")
        );
        assert_eq!(cfg.memory_limit, Some(512 * 1024 * 1024));
    }

    #[test]
    fn exec_result_fields() {
        let result = ExecResult {
            exit_code: 0,
            stdout: "hello\n".to_string(),
            stderr: String::new(),
        };
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn exec_result_failure() {
        let result = ExecResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "not found\n".to_string(),
        };
        assert_ne!(result.exit_code, 0);
        assert!(result.stderr.contains("not found"));
    }

    #[test]
    fn volume_mount_read_write() {
        let mount = VolumeMount {
            host_path: "/src".to_string(),
            container_path: "/dst".to_string(),
            read_only: false,
        };
        assert!(!mount.read_only);
        assert_eq!(mount.host_path, "/src");
        assert_eq!(mount.container_path, "/dst");
    }
}
