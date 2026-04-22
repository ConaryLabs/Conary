// conary-core/tests/bootstrap_phase3_recipe_policy.rs

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use conary_core::bootstrap::SYSTEM_BUILD_ORDER;
use conary_core::recipe::{Recipe, parse_recipe_file};
use toml::Value;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("recipes/system/systemd.toml").is_file())
        .expect("workspace root not found from crate manifest ancestors")
}

fn system_dir() -> PathBuf {
    workspace_root().join("recipes/system")
}

fn versions_toml() -> PathBuf {
    workspace_root().join("recipes/versions.toml")
}

fn expected_phase3_versions() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("man-pages", "6.17"),
        ("iana-etc", "20260202"),
        ("glibc", "2.43"),
        ("zlib", "1.3.2"),
        ("bzip2", "1.0.8"),
        ("xz", "5.8.2"),
        ("lz4", "1.10.0"),
        ("zstd", "1.5.7"),
        ("file", "5.46"),
        ("readline", "8.3"),
        ("pcre2", "10.47"),
        ("m4", "1.4.21"),
        ("bc", "7.0.3"),
        ("flex", "2.6.4"),
        ("tcl", "8.6.17"),
        ("expect", "5.45.4"),
        ("dejagnu", "1.6.3"),
        ("pkgconf", "2.5.1"),
        ("binutils", "2.46.0"),
        ("gmp", "6.3.0"),
        ("mpfr", "4.2.2"),
        ("mpc", "1.3.1"),
        ("attr", "2.5.2"),
        ("acl", "2.3.2"),
        ("libcap", "2.77"),
        ("libxcrypt", "4.5.2"),
        ("shadow", "4.19.3"),
        ("gcc", "15.2.0"),
        ("ncurses", "6.6"),
        ("sed", "4.9"),
        ("psmisc", "23.7"),
        ("gettext", "1.0"),
        ("bison", "3.8.2"),
        ("grep", "3.12"),
        ("bash", "5.3"),
        ("libtool", "2.5.4"),
        ("gdbm", "1.26"),
        ("gperf", "3.3"),
        ("expat", "2.7.4"),
        ("inetutils", "2.7"),
        ("less", "692"),
        ("perl", "5.42.0"),
        ("xml-parser", "2.47"),
        ("intltool", "0.51.0"),
        ("autoconf", "2.72"),
        ("automake", "1.18.1"),
        ("openssl", "3.6.1"),
        ("elfutils", "0.194"),
        ("libffi", "3.5.2"),
        ("sqlite", "3510200"),
        ("python", "3.14.3"),
        ("flit-core", "3.12.0"),
        ("packaging", "26.0"),
        ("wheel", "0.46.3"),
        ("setuptools", "82.0.0"),
        ("ninja", "1.13.2"),
        ("meson", "1.10.1"),
        ("composefs", "1.0.8"),
        ("kmod", "34.2"),
        ("linux", "6.19.8"),
        ("coreutils", "9.10"),
        ("diffutils", "3.12"),
        ("gawk", "5.3.2"),
        ("findutils", "4.10.0"),
        ("groff", "1.23.0"),
        ("gzip", "1.14"),
        ("iproute2", "6.18.0"),
        ("kbd", "2.9.0"),
        ("libpipeline", "1.5.8"),
        ("make", "4.4.1"),
        ("patch", "2.8"),
        ("tar", "1.35"),
        ("texinfo", "7.2"),
        ("vim", "9.2.0078"),
        ("markupsafe", "3.0.3"),
        ("jinja2", "3.1.6"),
        ("pyelftools", "0.32"),
        ("systemd", "259.1"),
        ("dbus", "1.16.2"),
        ("man-db", "2.13.1"),
        ("procps-ng", "4.0.6"),
        ("util-linux", "2.41.3"),
        ("e2fsprogs", "1.47.3"),
    ])
}

fn load_system_recipes() -> BTreeMap<String, Recipe> {
    let mut recipes = BTreeMap::new();

    for entry in fs::read_dir(system_dir()).expect("failed to read recipes/system") {
        let entry = entry.expect("failed to read recipes/system entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }

        let recipe = parse_recipe_file(&path)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));
        recipes.insert(recipe.package.name.clone(), recipe);
    }

    recipes
}

fn load_phase3_versions() -> BTreeMap<String, String> {
    let content =
        fs::read_to_string(versions_toml()).expect("failed to read recipes/versions.toml");
    let parsed: Value = toml::from_str(&content).expect("recipes/versions.toml must parse as TOML");
    let mut versions = BTreeMap::new();

    let toolchain = parsed
        .get("toolchain")
        .and_then(Value::as_table)
        .expect("recipes/versions.toml must contain a [toolchain] table");
    for name in ["gcc", "glibc", "binutils", "linux", "gmp", "mpfr", "mpc"] {
        let version = toolchain
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("recipes/versions.toml missing [toolchain].{name}"));
        versions.insert(name.to_string(), version.to_string());
    }

    let system = parsed
        .get("system")
        .and_then(Value::as_table)
        .expect("recipes/versions.toml must contain a [system] table");

    for (name, value) in system {
        let version = value
            .as_str()
            .unwrap_or_else(|| panic!("system version for {name} must be a string"));
        versions.insert(name.clone(), version.to_string());
    }

    versions
}

#[test]
fn phase3_build_order_matches_the_approved_lfs13_selection() {
    let actual: BTreeSet<&str> = SYSTEM_BUILD_ORDER.iter().copied().collect();
    let expected: BTreeSet<&str> = expected_phase3_versions().keys().copied().collect();

    assert_eq!(
        actual, expected,
        "Phase 3 package set must match the approved LFS 13.0-systemd Chapter 8 selection"
    );
    assert!(
        !actual.contains("grub"),
        "Conary intentionally omits standalone grub in Phase 3 because the qcow2 path uses systemd-boot"
    );
}

#[test]
fn phase3_build_order_keeps_kmod_after_ninja_and_meson() {
    let positions = SYSTEM_BUILD_ORDER
        .iter()
        .enumerate()
        .map(|(idx, package)| (*package, idx))
        .collect::<BTreeMap<_, _>>();

    let kmod = positions["kmod"];
    let ninja = positions["ninja"];
    let meson = positions["meson"];

    assert!(
        ninja < meson,
        "Phase 3 must build ninja before meson so meson is usable when kmod runs"
    );
    assert!(
        meson < kmod,
        "Phase 3 must build meson before kmod because recipes/system/kmod.toml invokes meson setup"
    );
}

#[test]
fn phase3_build_order_keeps_composefs_after_meson_before_kmod() {
    let positions = SYSTEM_BUILD_ORDER
        .iter()
        .enumerate()
        .map(|(idx, package)| (*package, idx))
        .collect::<BTreeMap<_, _>>();

    let composefs = positions["composefs"];
    let meson = positions["meson"];
    let kmod = positions["kmod"];

    assert!(
        meson < composefs,
        "Phase 3 must build composefs after meson so mount.composefs comes from the checked-in system recipe rather than the host"
    );
    assert!(
        composefs < kmod,
        "Phase 3 should place composefs before the later system package stretch so the self-hosting VM contract stays explicit"
    );
}

#[test]
fn system_versions_toml_matches_the_approved_phase3_audit() {
    let versions = load_phase3_versions();

    for (package_name, expected_version) in expected_phase3_versions() {
        let actual_version = versions.get(package_name).unwrap_or_else(|| {
            panic!("recipes/versions.toml missing effective Phase 3 version for {package_name}")
        });
        assert_eq!(
            actual_version, expected_version,
            "recipes/versions.toml drifted for Phase 3 package {package_name}"
        );
    }
}

#[test]
fn system_recipe_versions_match_versions_toml_for_phase3_packages() {
    let recipes = load_system_recipes();
    let versions = load_phase3_versions();

    for package_name in SYSTEM_BUILD_ORDER {
        let recipe = recipes
            .get(package_name)
            .unwrap_or_else(|| panic!("missing Phase 3 recipe {package_name}"));
        let expected_version = versions.get(package_name).unwrap_or_else(|| {
            panic!("recipes/versions.toml missing effective Phase 3 version for {package_name}")
        });

        assert_eq!(
            recipe.package.version, *expected_version,
            "recipe {} version drifted from recipes/versions.toml",
            package_name
        );
    }
}

#[test]
fn systemd_bootloader_deviation_sets_explicit_sbat_metadata() {
    let systemd = load_system_recipes()
        .remove("systemd")
        .expect("missing Phase 3 recipe systemd");
    let configure = systemd
        .build
        .configure
        .as_deref()
        .expect("systemd recipe must define a configure phase");

    assert!(
        configure.contains("-D sbat-distro=conaryos"),
        "systemd bootloader builds must not rely on os-release autodetection for SBAT distro ID"
    );
    assert!(
        configure.contains("-D sbat-distro-summary=conaryOS"),
        "systemd bootloader builds must set a stable SBAT summary explicitly"
    );
    assert!(
        configure.contains("-D sbat-distro-url=https://conary.io"),
        "systemd bootloader builds must set a stable SBAT distro URL explicitly"
    );
    assert!(
        configure.contains("-D sbat-distro-version=%(version)s-conary"),
        "systemd bootloader builds must set a deterministic SBAT version explicitly"
    );
}

#[test]
fn linux_recipe_enables_built_in_erofs_for_generation_mounts() {
    let linux = load_system_recipes()
        .remove("linux")
        .expect("missing Phase 3 recipe linux");
    let configure = linux
        .build
        .configure
        .as_deref()
        .expect("linux recipe must define a configure phase");

    assert!(
        configure.contains("scripts/config --enable EROFS_FS"),
        "linux Phase 3 recipe must enable EROFS so guest generation mounts work"
    );
    assert!(
        configure.contains("scripts/config --set-val EROFS_FS y"),
        "linux Phase 3 recipe must build EROFS support into the kernel for self-host validation"
    );
    assert!(
        configure.contains("scripts/config --enable OVERLAY_FS"),
        "linux Phase 3 recipe must enable overlayfs because composefs runtime mounts depend on it"
    );
    assert!(
        configure.contains("scripts/config --set-val OVERLAY_FS y"),
        "linux Phase 3 recipe must build overlayfs into the kernel so composefs generation mounts do not depend on late module loading"
    );
    assert!(
        configure.contains("scripts/config --enable BLK_DEV_LOOP"),
        "linux Phase 3 recipe must enable loop-device support because composefs mounts EROFS metadata images through the loop stack"
    );
    assert!(
        configure.contains("scripts/config --set-val BLK_DEV_LOOP y"),
        "linux Phase 3 recipe must build loop-device support into the kernel for truthful composefs validation"
    );
    assert!(
        configure.contains("scripts/config --enable FS_VERITY"),
        "linux Phase 3 recipe must enable fs-verity so composefs root.erofs images can be protected before remount"
    );
}

#[test]
fn linux_recipe_install_step_is_rerunnable_without_interactive_overwrite_prompts() {
    let linux = load_system_recipes()
        .remove("linux")
        .expect("missing Phase 3 recipe linux");
    let install = linux
        .build
        .install
        .as_deref()
        .expect("linux recipe must define an install phase");

    assert!(
        !install.contains("cp -iv"),
        "linux Phase 3 install must not use interactive cp prompts because resumed bootstrap rebuilds must be non-interactive"
    );
}
