fn main() {
    println!("cargo:rerun-if-env-changed=CSILCTL_VERSION");

    let version =
        std::env::var("CSILCTL_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned());
    println!("cargo:rustc-env=CSILCTL_VERSION={version}");
}
