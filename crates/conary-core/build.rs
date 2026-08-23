// crates/conary-core/build.rs

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NATIVE_RPM_ORACLE");
    if std::env::var_os("CARGO_FEATURE_NATIVE_RPM_ORACLE").is_none() {
        return;
    }

    println!("cargo:rerun-if-changed=src/repository/catalog/parity/rpm/libsolv_shim.c");
    pkg_config::Config::new()
        .atleast_version("0.7.36")
        .probe("libsolvext")
        .expect("native-rpm-oracle requires libsolvext >= 0.7.36");
    cc::Build::new()
        .file("src/repository/catalog/parity/rpm/libsolv_shim.c")
        .warnings(true)
        .extra_warnings(true)
        .flag_if_supported("-Werror")
        .compile("conary_libsolv_shim");
}
