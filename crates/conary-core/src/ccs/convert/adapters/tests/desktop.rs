// conary-core/src/ccs/convert/adapters/tests/desktop.rs

use super::*;

#[test]
fn alternatives_install_and_remove_are_complete_when_parseable() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    let install = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "update-alternatives",
            &[
                "--install",
                "/usr/bin/editor",
                "editor",
                "/usr/bin/demo-editor",
                "50",
                "--slave",
                "/usr/share/man/man1/editor.1.gz",
                "editor.1.gz",
                "/usr/share/man/man1/demo-editor.1.gz",
                "--slave",
                "/usr/share/man/man1/view.1.gz",
                "view.1.gz",
                "/usr/share/man/man1/demo-view.1.gz",
            ],
        ),
        payload: &payload,
    });
    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = install
    else {
        panic!("alternatives install should be known");
    };
    assert_eq!(reason_code, "helper-complete-alternatives-registration");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "alternatives");
    assert_eq!(effects[0].path.as_deref(), Some("/usr/bin/editor"));

    let remove = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "alternatives",
            &["--remove", "editor", "/usr/bin/demo-editor"],
        ),
        payload: &payload,
    });
    assert!(matches!(
        remove,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-alternatives-registration"
    ));
}

#[test]
fn alternatives_interactive_and_broad_actions_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for argv in [
        vec!["--config", "editor"],
        vec!["--remove-all", "editor"],
        vec!["--remove", "editor"],
    ] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("update-alternatives", &argv),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review {
                reason_code,
                class_id,
                ..
            }
                if reason_code == "review-class-alternatives-interactive-or-broad"
                    && class_id.as_deref() == Some("alternatives-interactive-or-broad")
        ));
    }
}

#[test]
fn cache_refresh_known_forms_are_complete_with_payload_inputs() {
    let registry = AdapterRegistry::default();
    let mut payload = PayloadHints::default();
    payload
        .cache_inputs
        .entry("mime-db".to_string())
        .or_default()
        .insert("/usr/share/mime/packages/demo.xml".to_string());
    payload
        .cache_inputs
        .entry("desktop-db".to_string())
        .or_default()
        .insert("/usr/share/applications/demo.desktop".to_string());
    payload
        .cache_inputs
        .entry("icon-cache".to_string())
        .or_default()
        .insert("/usr/share/icons/hicolor/16x16/apps/demo.png".to_string());
    payload
        .cache_inputs
        .entry("gsettings".to_string())
        .or_default()
        .insert("/usr/share/glib-2.0/schemas/org.example.demo.gschema.xml".to_string());
    payload
        .cache_inputs
        .entry("font-cache".to_string())
        .or_default()
        .insert("/usr/share/fonts/demo/demo.ttf".to_string());

    let mime = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("update-mime-database", &["/usr/share/mime"]),
        payload: &payload,
    });
    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = mime
    else {
        panic!("mime cache refresh should be known");
    };
    assert_eq!(reason_code, "helper-complete-cache-refresh");
    assert_eq!(effects[0].replacement, EffectReplacement::Complete);
    assert_eq!(effects[0].kind, "cache-refresh");
    assert_eq!(
        effects[0].extra["cache_kind"],
        toml::Value::String("mime-db".to_string())
    );

    let desktop = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "update-desktop-database",
            &["-q", "/usr/share/applications"],
        ),
        payload: &payload,
    });
    assert!(matches!(
        desktop,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let icons = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "gtk-update-icon-cache",
            &["--force", "--quiet", "/usr/share/icons/hicolor"],
        ),
        payload: &payload,
    });
    assert!(matches!(
        icons,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let icons_combined_flags = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "gtk-update-icon-cache",
            &["-qf", "/usr/share/icons/hicolor"],
        ),
        payload: &payload,
    });
    assert!(matches!(
        icons_combined_flags,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let schemas = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation(
            "glib-compile-schemas",
            &["--allow-any-name", "/usr/share/glib-2.0/schemas"],
        ),
        payload: &payload,
    });
    assert!(matches!(
        schemas,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let schemas_default_path = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("glib-compile-schemas", &[]),
        payload: &payload,
    });
    assert!(matches!(
        schemas_default_path,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let fonts = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("fc-cache", &["-fs"]),
        payload: &payload,
    });
    assert!(matches!(
        fonts,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));

    let fonts_with_dir = registry.classify_invocation_with_context(AdapterInput {
        invocation: &invocation("fc-cache", &["-f", "/usr/share/fonts/demo"]),
        payload: &payload,
    });
    assert!(matches!(
        fonts_with_dir,
        ScriptletClassification::Known { reason_code, .. }
            if reason_code == "helper-complete-cache-refresh"
    ));
}

#[test]
fn cache_refresh_nonstandard_paths_are_review() {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::default();

    for path in ["/opt/vendor/mime", "/usr/local/share/mime"] {
        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("update-mime-database", &[path]),
            payload: &payload,
        });
        assert!(matches!(
            classification,
            ScriptletClassification::Review {
                reason_code,
                class_id,
                ..
            }
                if reason_code == "review-class-cache-refresh-nonstandard"
                    && class_id.as_deref() == Some("cache-refresh-nonstandard")
        ));
    }
}
