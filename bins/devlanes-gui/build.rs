use std::{error::Error, fs, path::Path};

/// The devlanes website, named in the Windows version resource.
const HOMEPAGE: &str = "https://devlanes.run";

/// Embeds the Windows manifest and version resource; nothing on other OSes.
fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return Ok(());
    }
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").ok_or("missing CARGO_MANIFEST_DIR")?;
    embed_manifest(Path::new(&manifest));
    embed_version_info()
}

/// Embeds `devlanes-gui.manifest` into the `devlanes-gui` binary only, as
/// `bins/devlanes/build.rs` does for `devlanes`: per-monitor DPI awareness,
/// which the Windows host needs to capture and click in physical pixels,
/// and no console window when the parent passes none down.
fn embed_manifest(package: &Path) {
    let file = package.join("devlanes-gui.manifest");
    println!("cargo:rerun-if-changed=devlanes-gui.manifest");
    println!("cargo:rustc-link-arg-bin=devlanes-gui=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=devlanes-gui=/MANIFESTINPUT:{}",
        file.display()
    );
}

/// Embeds a version resource into `devlanes-gui.exe`, as `bins/devlanes/build.rs`
/// does for `devlanes.exe`: Explorer, Task Manager and antivirus heuristics read
/// it. This package is in the release's `fixed` version group, so its version
/// is the devlanes release version.
fn embed_version_info() -> Result<(), Box<dyn Error>> {
    let version = std::env::var("CARGO_PKG_VERSION")?;
    let part = |name: &str| -> Result<u16, Box<dyn Error>> { Ok(std::env::var(name)?.parse()?) };
    let numeric = format!(
        "{},{},{},0",
        part("CARGO_PKG_VERSION_MAJOR")?,
        part("CARGO_PKG_VERSION_MINOR")?,
        part("CARGO_PKG_VERSION_PATCH")?
    );
    let flags = if std::env::var("CARGO_PKG_VERSION_PRE")?.is_empty() {
        "0"
    } else {
        "VS_FF_PRERELEASE"
    };
    // Release builds set the commit they built (build.yml), so the source link
    // names it; other builds link the repository.
    println!("cargo:rerun-if-env-changed=DEVLANES_BUILD_COMMIT");
    let repository = std::env::var("CARGO_PKG_REPOSITORY")?;
    let source = match std::env::var("DEVLANES_BUILD_COMMIT") {
        Ok(commit) if commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()) => {
            format!("{repository}/tree/{commit}")
        }
        Ok(commit) if !commit.is_empty() => {
            return Err(format!("DEVLANES_BUILD_COMMIT is no commit hash: {commit:?}").into());
        }
        _ => repository,
    };
    // rc.exe strings escape a quote by doubling it and read `\` as an escape.
    let comments = format!(
        "{}. Website {HOMEPAGE}, docs {HOMEPAGE}/docs/, source {source}",
        std::env::var("CARGO_PKG_DESCRIPTION")?,
    )
    .replace('\\', "\\\\")
    .replace('"', "\"\"");
    // Task Manager names the process by FileDescription, so it stays the bare
    // name. The file is UTF-8, which rc.exe reads only with the pragma.
    let resource = format!(
        r#"#pragma code_page(65001)
#include <winver.h>

VS_VERSION_INFO VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
FILEFLAGS {flags}
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
FILESUBTYPE VFT2_UNKNOWN
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "Comments", "{comments}"
            VALUE "CompanyName", "Devlanes"
            VALUE "FileDescription", "devlanes-gui"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "devlanes-gui"
            VALUE "LegalCopyright", "Copyright (C) Devlanes. Licensed under Apache-2.0."
            VALUE "OriginalFilename", "devlanes-gui.exe"
            VALUE "ProductName", "devlanes-gui"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );
    let out = std::env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?;
    let path = Path::new(&out).join("devlanes-gui.rc");
    write_changed(&path, &resource)?;
    // Required: the Windows SDK that links the executable also ships rc.exe, so
    // a missing compiler is a broken setup, not a reason to ship without it.
    embed_resource::compile_for(&path, ["devlanes-gui"], embed_resource::NONE)
        .manifest_required()?;
    Ok(())
}

/// Writes `content` unless the file already holds it, so its mtime stays.
fn write_changed(path: &Path, content: &str) -> std::io::Result<()> {
    match fs::read(path) {
        Ok(existing) if existing == content.as_bytes() => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::write(path, content)
}
