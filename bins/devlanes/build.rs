use std::{error::Error, fs, path::Path};

/// Generate served schema artifacts from the registered integration types.
fn main() -> Result<(), Box<dyn Error>> {
    // Cargo tracks build-dependency changes. Exclude generated outputs to avoid rebuild loops.
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").ok_or("missing CARGO_MANIFEST_DIR")?;
    let config = Path::new(&manifest).join("../../crates/config");
    let project = devlanes_config::project_schema_with_presets(&[
        (
            devlanes_preset_vite::NAME,
            devlanes_preset_vite::options_schema(),
        ),
        (
            devlanes_preset_next::NAME,
            devlanes_preset_next::options_schema(),
        ),
        (
            devlanes_preset_wrangler::NAME,
            devlanes_preset_wrangler::options_schema(),
        ),
        (
            devlanes_preset_tauri::NAME,
            devlanes_preset_tauri::options_schema(),
        ),
    ]);
    // These publish=false workspace packages keep served artifacts in the source tree.
    // The JSON $comment marks them as generated; do not edit them by hand.
    write_changed(
        &config.join("schema.json"),
        &devlanes_config::schema_artifact(project)?,
    )?;
    write_changed(
        &config.join("user-schema.json"),
        &devlanes_config::schema_artifact(devlanes_config::user_schema())?,
    )?;
    embed_manifest(Path::new(&manifest));
    embed_version_info()?;
    embed_notes(Path::new(&manifest))?;
    Ok(())
}

/// The devlanes website, named in the Windows version resource.
const HOMEPAGE: &str = "https://devlanes.run";

/// Embeds a version resource into the `devlanes` binary on Windows (MSVC):
/// the product, description and version that Explorer's Details tab and Task
/// Manager show. Antivirus heuristics also count an executable without one
/// against it. This package's version is the devlanes release version, the one
/// `devlanes_version::VERSION` reads from this manifest.
fn embed_version_info() -> Result<(), Box<dyn Error>> {
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return Ok(());
    }
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
    // rc.exe strings escape a quote by doubling it.
    let comments = format!(
        "{}. Website {HOMEPAGE}, docs {HOMEPAGE}/docs/, source {}",
        std::env::var("CARGO_PKG_DESCRIPTION")?,
        std::env::var("CARGO_PKG_REPOSITORY")?
    )
    .replace('"', "\"\"");
    // Task Manager names the process by FileDescription, so it stays the bare name.
    let resource = format!(
        r#"#include <winver.h>

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
            VALUE "FileDescription", "devlanes"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "devlanes"
            VALUE "LegalCopyright", "Copyright (C) Devlanes. Licensed under Apache-2.0."
            VALUE "OriginalFilename", "devlanes.exe"
            VALUE "ProductName", "devlanes-cli"
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
    let path = Path::new(&out).join("devlanes.rc");
    write_changed(&path, &resource)?;
    // Required: the Windows SDK that links devlanes.exe also ships rc.exe, so a
    // missing compiler is a broken setup, not a reason to ship without the resource.
    embed_resource::compile_for(&path, ["devlanes"], embed_resource::NONE).manifest_required()?;
    Ok(())
}

/// Writes `OUT_DIR/notes.rs`: every release in `CHANGELOG.md`, newest first,
/// as `(version, [(level, first line, highlight)])` with commit links
/// stripped and the highlight as `(title, body)`, for the release highlights
/// dialog (`src/notes.rs`). Other entry bodies stay out of the binary.
fn embed_notes(package: &Path) -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=CHANGELOG.md");
    let changelog = fs::read_to_string(package.join("CHANGELOG.md"))?;
    let mut code = String::from(
        "/// Generated by build.rs from CHANGELOG.md.\n\
         pub static RELEASES: &[(&str, &[Embedded])] = &[\n",
    );
    for release in devlanes_update::notes::parse(&changelog) {
        code.push_str(&format!("    ({:?}, &[\n", release.version));
        for entry in release.entries {
            let level = match entry.level {
                devlanes_update::notes::Level::Minor => "Minor",
                devlanes_update::notes::Level::Patch => "Patch",
            };
            let highlight = match entry.highlight {
                Some(found) => format!("Some(({:?}, {:?}))", found.title, found.body),
                None => "None".to_owned(),
            };
            code.push_str(&format!(
                "        (Level::{level}, {:?}, {highlight}),\n",
                entry.line
            ));
        }
        code.push_str("    ]),\n");
    }
    code.push_str("];\n");
    let out = std::env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?;
    write_changed(&Path::new(&out).join("notes.rs"), &code)?;
    Ok(())
}

/// Embeds `devlanes.manifest` into the `devlanes` binary on Windows (MSVC),
/// so the logon Run key starts `daemon run` without a console window. Only
/// that binary: this package's test executables keep the default console.
fn embed_manifest(package: &Path) {
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return;
    }
    let file = package.join("devlanes.manifest");
    println!("cargo:rerun-if-changed=devlanes.manifest");
    println!("cargo:rustc-link-arg-bin=devlanes=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=devlanes=/MANIFESTINPUT:{}",
        file.display()
    );
}

/// Preserve file mtimes when generation produces identical bytes.
fn write_changed(path: &Path, content: &str) -> std::io::Result<()> {
    match fs::read(path) {
        Ok(existing) if existing == content.as_bytes() => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::write(path, content)
}
