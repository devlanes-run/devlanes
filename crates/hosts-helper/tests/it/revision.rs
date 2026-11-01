//! Ties [`REVISION`] to the helper's sources, so a change to the code that runs
//! as root cannot ship without someone deciding whether installed helpers must
//! be replaced.

use std::fs;
use std::path::{Path, PathBuf};

use devlanes_hosts_helper::REVISION;
use sha2::{Digest, Sha256};

/// The revision and the hash of `src/` it was recorded for.
const RECORDED: (u32, &str) = (
    1,
    "f4281f549e7283c997b2035ceb847beb149168c048dd5c61086847bd02c9de4f",
);

/// Every file under `dir`, recursively.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read src") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// SHA-256 over each file's relative path and contents, in sorted path order.
/// Paths use `/` and contents `\n`, so every OS and checkout hashes alike even
/// though `.gitattributes` already checks these files out with LF endings.
fn source_hash() -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths = Vec::new();
    files(&src, &mut paths);
    let mut entries: Vec<(String, PathBuf)> = paths
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(&src).expect("under src");
            let name = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            (name, path)
        })
        .collect();
    entries.sort();
    let mut hasher = Sha256::new();
    for (name, path) in entries {
        let bytes = fs::read(&path).expect("read source");
        let text = String::from_utf8(bytes).expect("utf-8 source");
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(text.replace("\r\n", "\n").as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn revision_matches_the_recorded_sources() {
    let hash = source_hash();
    assert_eq!(
        (REVISION, hash.as_str()),
        RECORDED,
        "the helper's code changed (src/ now hashes to {hash}); decide whether installed \
         helpers must be replaced, bump REVISION in src/lib.rs if so, then update RECORDED \
         in tests/it/revision.rs to (REVISION, \"{hash}\")"
    );
}
