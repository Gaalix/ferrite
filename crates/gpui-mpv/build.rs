fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        pkg_config::Config::new()
            .probe("mpv")
            .expect("Failed to find `mpv` (libmpv) via pkg-config");

        pkg_config::Config::new()
            .probe("egl")
            .expect("Failed to find `egl` (libEGL) via pkg-config");

        pkg_config::Config::new()
            .probe("wayland-client")
            .expect("Failed to find `wayland-client` via pkg-config");

        pkg_config::Config::new()
            .probe("wayland-egl")
            .expect("Failed to find `wayland-egl` via pkg-config");
    }
}
