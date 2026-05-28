// conary-core/src/ccs/convert/payload_hints.rs

use crate::packages::traits::ExtractedFile;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PayloadHints {
    pub systemd_units: BTreeSet<String>,
    pub tmpfiles_configs: BTreeSet<String>,
    pub sysusers_configs: BTreeSet<String>,
    pub shared_libraries: BTreeSet<String>,
    pub cache_inputs: BTreeMap<String, BTreeSet<String>>,
}

impl PayloadHints {
    pub fn from_files(files: &[ExtractedFile]) -> Self {
        let mut hints = Self::default();

        for file in files {
            let path = file.path.as_str();
            if let Some(unit) = systemd_unit_name(path) {
                hints.systemd_units.insert(unit.to_string());
            }
            if is_tmpfiles_config(path) {
                hints.tmpfiles_configs.insert(path.to_string());
            }
            if is_sysusers_config(path) {
                hints.sysusers_configs.insert(path.to_string());
            }
            if is_shared_library(path) {
                hints.shared_libraries.insert(path.to_string());
            }
            for kind in cache_input_kinds(path) {
                hints
                    .cache_inputs
                    .entry(kind.to_string())
                    .or_default()
                    .insert(path.to_string());
            }
        }

        hints
    }

    pub fn has_cache_input(&self, kind: &str) -> bool {
        self.cache_inputs
            .get(kind)
            .is_some_and(|paths| !paths.is_empty())
    }
}

fn systemd_unit_name(path: &str) -> Option<&str> {
    let prefix_matches = path.starts_with("/usr/lib/systemd/system/")
        || path.starts_with("/lib/systemd/system/")
        || path.starts_with("/etc/systemd/system/");
    if !prefix_matches {
        return None;
    }
    path.rsplit('/').next().filter(|name| {
        matches!(
            name.rsplit_once('.').map(|(_, suffix)| suffix),
            Some("service" | "socket" | "timer" | "path" | "target")
        )
    })
}

fn is_tmpfiles_config(path: &str) -> bool {
    (path.starts_with("/usr/lib/tmpfiles.d/")
        || path.starts_with("/lib/tmpfiles.d/")
        || path.starts_with("/etc/tmpfiles.d/"))
        && path.ends_with(".conf")
}

fn is_sysusers_config(path: &str) -> bool {
    (path.starts_with("/usr/lib/sysusers.d/")
        || path.starts_with("/lib/sysusers.d/")
        || path.starts_with("/etc/sysusers.d/"))
        && path.ends_with(".conf")
}

fn is_shared_library(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|name| {
        name.starts_with("lib") && (name.contains(".so.") || name.ends_with(".so"))
    })
}

fn cache_input_kinds(path: &str) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if path.starts_with("/usr/share/mime/packages/") && path.ends_with(".xml") {
        kinds.push("mime-db");
    }
    if path.starts_with("/usr/share/applications/") && path.ends_with(".desktop") {
        kinds.push("desktop-db");
    }
    if path.starts_with("/usr/share/icons/") {
        kinds.push("icon-cache");
    }
    if path.starts_with("/usr/share/glib-2.0/schemas/") && path.ends_with(".gschema.xml") {
        kinds.push("gsettings");
    }
    if (path.starts_with("/usr/share/fonts/")
        || path.starts_with("/usr/local/share/fonts/")
        || path.starts_with("/usr/share/texmf/fonts/"))
        && matches!(
            path.rsplit('.').next(),
            Some("ttf" | "otf" | "pcf" | "pfb" | "pfm")
        )
    {
        kinds.push("font-cache");
    }
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> ExtractedFile {
        ExtractedFile {
            path: path.to_string(),
            content: Vec::new(),
            size: 0,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
        }
    }

    #[test]
    fn payload_hints_find_systemd_tmpfiles_sysusers_and_libraries() {
        let hints = PayloadHints::from_files(&[
            file("/usr/lib/systemd/system/demo.service"),
            file("/usr/lib/tmpfiles.d/demo.conf"),
            file("/usr/lib/sysusers.d/demo.conf"),
            file("/usr/lib64/libdemo.so.1"),
        ]);

        assert!(hints.systemd_units.contains("demo.service"));
        assert!(
            hints
                .tmpfiles_configs
                .contains("/usr/lib/tmpfiles.d/demo.conf")
        );
        assert!(
            hints
                .sysusers_configs
                .contains("/usr/lib/sysusers.d/demo.conf")
        );
        assert!(hints.shared_libraries.contains("/usr/lib64/libdemo.so.1"));
    }

    #[test]
    fn payload_hints_find_cache_inputs_by_kind() {
        let hints = PayloadHints::from_files(&[
            file("/usr/share/mime/packages/demo.xml"),
            file("/usr/share/applications/demo.desktop"),
            file("/usr/share/icons/hicolor/16x16/apps/demo.png"),
            file("/usr/share/glib-2.0/schemas/org.example.demo.gschema.xml"),
            file("/usr/share/fonts/demo/demo.ttf"),
        ]);

        assert!(hints.cache_inputs["mime-db"].contains("/usr/share/mime/packages/demo.xml"));
        assert!(hints.cache_inputs["desktop-db"].contains("/usr/share/applications/demo.desktop"));
        assert!(
            hints.cache_inputs["icon-cache"]
                .contains("/usr/share/icons/hicolor/16x16/apps/demo.png")
        );
        assert!(
            hints.cache_inputs["gsettings"]
                .contains("/usr/share/glib-2.0/schemas/org.example.demo.gschema.xml")
        );
        assert!(hints.cache_inputs["font-cache"].contains("/usr/share/fonts/demo/demo.ttf"));
    }
}
