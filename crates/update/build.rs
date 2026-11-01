//! Records the target triple this crate is built for, which names the release
//! archive an update downloads (`devlanes-<version>-<target>.tar.gz`).

fn main() {
    let target = std::env::var("TARGET").expect("set by cargo");
    println!("cargo:rustc-env=DEVLANES_TARGET={target}");
}
