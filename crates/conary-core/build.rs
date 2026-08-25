// crates/conary-core/build.rs

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NATIVE_RPM_ORACLE");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NATIVE_DEBIAN_ORACLE");
    if std::env::var_os("CARGO_FEATURE_NATIVE_RPM_ORACLE").is_some() {
        build_rpm_oracle();
    }
    if std::env::var_os("CARGO_FEATURE_NATIVE_DEBIAN_ORACLE").is_some() {
        build_debian_oracle();
    }
}

fn build_rpm_oracle() {
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

fn build_debian_oracle() {
    println!("cargo:rerun-if-changed=src/repository/catalog/parity/debian/apt_pkg_shim.cpp");
    pkg_config::Config::new()
        .atleast_version("3.2.0")
        .probe("apt-pkg")
        .expect("native-debian-oracle requires apt-pkg >= 3.2.0");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("src/repository/catalog/parity/debian/apt_pkg_shim.cpp")
        .warnings(true)
        .extra_warnings(true)
        .flag_if_supported("-Werror")
        .compile("conary_apt_pkg_shim");
}
