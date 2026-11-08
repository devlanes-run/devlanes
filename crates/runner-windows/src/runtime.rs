//! Whether a program links the MSYS runtime, read from its image file.

use std::{
    collections::HashMap,
    fs::File,
    io,
    os::windows::fs::FileExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::SystemTime,
};

/// The runtime DLLs of MSYS2 and Git for Windows (`msys-2.0.dll`) and of
/// Cygwin (`cygwin1.dll`).
const RUNTIMES: [&str; 2] = ["msys-2.0.dll", "cygwin1.dll"];

/// Most import descriptors read from one image; real programs have a few
/// dozen, so more only comes from a malformed file.
const MAX_IMPORTS: usize = 4096;

/// Longest DLL name read from an import descriptor.
const MAX_NAME: usize = 256;

/// Answers of [`links_msys`] by image path and modification time, so a
/// program is read once while it stays the same file. Cheap to clone; clones
/// share the answers.
#[derive(Debug, Clone, Default)]
pub struct Runtimes(Arc<Mutex<HashMap<(PathBuf, SystemTime), bool>>>);

impl Runtimes {
    /// [`links_msys`] for `image`, read once per modification time. The
    /// image is read without holding the lock, so a slow read never delays
    /// another lookup. Blocking.
    pub(crate) fn links_msys(&self, image: &Path) -> io::Result<bool> {
        let key = (image.to_path_buf(), std::fs::metadata(image)?.modified()?);
        let known = self.lock().get(&key).copied();
        if let Some(links) = known {
            return Ok(links);
        }
        let links = links_msys(image)?;
        self.lock().insert(key, links);
        Ok(links)
    }

    /// The answers, ignoring poisoning: each entry is inserted whole.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(PathBuf, SystemTime), bool>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether the program at `image` imports `msys-2.0.dll` or `cygwin1.dll`,
/// the runtime MSYS2, Git for Windows' `usr\bin` tools and Cygwin programs
/// link against, whatever the program is called.
///
/// Reads the import table of the file, not the modules a running process has
/// loaded, so the answer does not depend on how far the loader got. Git's
/// `bin\sh.exe` and `bin\bash.exe` are native launchers without the import;
/// the `usr\bin` program they start has it. A file that is not a PE image,
/// or whose headers point outside it, is `InvalidData`. Blocking.
pub(crate) fn links_msys(image: &Path) -> io::Result<bool> {
    let file = Image(File::open(image)?);
    for name in file.imports()? {
        if RUNTIMES.iter().any(|dll| name.eq_ignore_ascii_case(dll)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A PE file read at offsets.
struct Image(File);

/// A section header: where its bytes sit in memory and in the file.
struct Section {
    virtual_address: u32,
    raw_offset: u32,
    raw_size: u32,
}

impl Image {
    /// The DLL names in the import directory, in table order.
    fn imports(&self) -> io::Result<Vec<String>> {
        if self.bytes::<2>(0)? != *b"MZ" {
            return Err(invalid("no DOS header"));
        }
        let pe = u64::from(self.u32(0x3C)?);
        if self.bytes::<4>(pe)? != *b"PE\0\0" {
            return Err(invalid("no PE signature"));
        }
        let coff = pe + 4;
        let sections = self.u16(coff + 2)?;
        let optional = coff + 20;
        let optional_size = u64::from(self.u16(coff + 16)?);
        // The data directories' count and the directories themselves sit at
        // different offsets in PE32 and PE32+ optional headers.
        let (count_at, directories) = match self.u16(optional)? {
            0x10B => (92, 96),
            0x20B => (108, 112),
            _ => return Err(invalid("unknown optional header")),
        };
        if self.u32(optional + count_at)? < 2 {
            return Ok(Vec::new());
        }
        let import = optional + directories + 8;
        if import + 8 > optional + optional_size {
            return Err(invalid("import directory outside the optional header"));
        }
        let table = self.u32(import)?;
        if table == 0 {
            return Ok(Vec::new());
        }
        let sections = (0..u64::from(sections))
            .map(|index| self.section(optional + optional_size + index * 40))
            .collect::<io::Result<Vec<_>>>()?;
        let mut names = Vec::new();
        for index in 0..MAX_IMPORTS as u32 {
            let descriptor = offset(&sections, table + index * 20)?;
            let name = self.u32(descriptor + 12)?;
            if name == 0 && self.u32(descriptor + 16)? == 0 {
                return Ok(names);
            }
            names.push(self.name(offset(&sections, name)?)?);
        }
        Err(invalid("too many imports"))
    }

    /// The section header at `at`.
    fn section(&self, at: u64) -> io::Result<Section> {
        Ok(Section {
            virtual_address: self.u32(at + 12)?,
            raw_size: self.u32(at + 16)?,
            raw_offset: self.u32(at + 20)?,
        })
    }

    /// The NUL-terminated ASCII name at `at`.
    fn name(&self, at: u64) -> io::Result<String> {
        let mut buffer = [0_u8; MAX_NAME];
        let read = self.read_some(at, &mut buffer)?;
        let end = buffer[..read]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| invalid("unterminated DLL name"))?;
        Ok(String::from_utf8_lossy(&buffer[..end]).into_owned())
    }

    /// The little-endian `u16` at `at`.
    fn u16(&self, at: u64) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(at)?))
    }

    /// The little-endian `u32` at `at`.
    fn u32(&self, at: u64) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(at)?))
    }

    /// `N` bytes at `at`; `InvalidData` past the end of the file.
    fn bytes<const N: usize>(&self, at: u64) -> io::Result<[u8; N]> {
        let mut buffer = [0_u8; N];
        if self.read_some(at, &mut buffer)? < N {
            return Err(invalid("truncated image"));
        }
        Ok(buffer)
    }

    /// Fills as much of `buffer` from `at` as the file holds; returns how
    /// much that is.
    fn read_some(&self, at: u64, buffer: &mut [u8]) -> io::Result<usize> {
        let mut read = 0;
        while read < buffer.len() {
            match self.0.seek_read(&mut buffer[read..], at + read as u64) {
                Ok(0) => break,
                Ok(n) => read += n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(read)
    }
}

/// The file offset of `rva`, through the section that holds it.
fn offset(sections: &[Section], rva: u32) -> io::Result<u64> {
    sections
        .iter()
        .find_map(|s| {
            let within = rva.checked_sub(s.virtual_address)?;
            (within < s.raw_size).then(|| u64::from(s.raw_offset) + u64::from(within))
        })
        .ok_or_else(|| invalid("address outside every section"))
}

/// A malformed image.
fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Git's `usr\bin\sh.exe` links the MSYS runtime; its `bin\bash.exe`
    /// launcher and this test binary do not; a cut-off copy is malformed.
    #[tokio::test]
    async fn links_msys_reads_imports() {
        let shell = devlanes_shell_windows::windows_shell()
            .await
            .expect("Git for Windows' sh.exe");
        assert!(links_msys(&shell).expect("sh.exe"));
        let git = shell.ancestors().nth(3).expect("Git's install directory");
        assert!(!links_msys(&git.join(r"bin\bash.exe")).expect("bash launcher"));
        let this = std::env::current_exe().expect("test binary");
        assert!(!links_msys(&this).expect("test binary"));

        let dir = tempfile::tempdir().expect("tempdir");
        let cut = dir.path().join("cut.exe");
        let bytes = std::fs::read(&shell).expect("read sh.exe");
        std::fs::write(&cut, &bytes[..256]).expect("write");
        let error = links_msys(&cut).expect_err("truncated");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    }
}
