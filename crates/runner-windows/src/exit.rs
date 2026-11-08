//! What a unit leader's exit code means, the Windows counterpart of decoding
//! a wait status on Unix.

use devlanes_core::ExitOutcome;

use crate::CONTROL_C_EXIT;

/// What the leader is, fixed at launch; it decides how its exit code reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Leader {
    /// A Windows program: `STATUS_CONTROL_C_EXIT` means a console event
    /// ended it.
    Native,
    /// Linked against the MSYS or Cygwin runtime: `s << 8` means signal `s`
    /// ended it.
    Msys,
}

/// SIGINT, numbered as by POSIX, MSYS and the Windows CRT.
const SIGINT: i32 = 2;
/// SIGKILL, numbered as by POSIX and MSYS.
const SIGKILL: i32 = 9;

/// The outcome of a leader that exited with `code` (`None` when it could not
/// be read). `terminated` says the runner terminated the unit's job before
/// the leader exited.
///
/// | Leader | Condition | Outcome |
/// | --- | --- | --- |
/// | any | the runner terminated the job first | signal 9 |
/// | any | `0xC000013A` (a console event, Ctrl-C or Ctrl-Break) | signal 2 |
/// | `Native` | any other code | that code (just and Git's `bin\bash.exe` pass on `sh`'s `512`) |
/// | `Msys` | `0..=0xFF` | that code |
/// | `Msys` | `0x100..=0xFFFF` | signal `(code >> 8) & 0x7F` |
/// | `Msys` | above `0xFFFF` | that code (a crash before the runtime ran) |
///
/// MSYS reports to a Windows parent a child that exited with `n` as `n` and
/// one a signal `s` ended as `s << 8`. It also cuts a native child's code to
/// its low 8 bits before passing it on (exit 1000 arrives as 232), so for an
/// MSYS leader a value in `0x100..=0xFFFF` can only be a signal.
pub(crate) fn outcome(code: Option<u32>, leader: Leader, terminated: bool) -> ExitOutcome {
    let (code, signal) = match code {
        _ if terminated => (None, Some(SIGKILL)),
        None => (None, None),
        Some(CONTROL_C_EXIT) => (None, Some(SIGINT)),
        Some(code @ 0x100..=0xFFFF) if leader == Leader::Msys => {
            (None, Some(((code >> 8) & 0x7F).cast_signed()))
        }
        Some(code) => (Some(code.cast_signed()), None),
    };
    ExitOutcome { code, signal }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected values come from Windows and from what Git's MSYS `sh`
    /// returns to a native parent: `exit 3` as 3, `kill -INT $$` as 512,
    /// `kill -TERM $$` as 3840, `kill -SEGV $$` as 2816, and a native
    /// child's `exit 1000` as 232.
    #[test]
    fn outcomes() {
        let code = |code| ExitOutcome {
            code: Some(code),
            signal: None,
        };
        let signal = |signal| ExitOutcome {
            code: None,
            signal: Some(signal),
        };
        let cases = [
            (
                "native console event",
                Some(CONTROL_C_EXIT),
                Leader::Native,
                false,
                signal(2),
            ),
            (
                "just or Git's bash launcher passing on 512",
                Some(512),
                Leader::Native,
                false,
                code(512),
            ),
            ("native exit 3", Some(3), Leader::Native, false, code(3)),
            (
                "MSYS console event",
                Some(CONTROL_C_EXIT),
                Leader::Msys,
                false,
                signal(2),
            ),
            (
                "MSYS killed by SIGINT",
                Some(512),
                Leader::Msys,
                false,
                signal(2),
            ),
            (
                "MSYS killed by SIGTERM",
                Some(3840),
                Leader::Msys,
                false,
                signal(15),
            ),
            (
                "MSYS killed by SIGSEGV",
                Some(2816),
                Leader::Msys,
                false,
                signal(11),
            ),
            ("MSYS exit 3", Some(3), Leader::Msys, false, code(3)),
            (
                "MSYS passing on a native 1000",
                Some(232),
                Leader::Msys,
                false,
                code(232),
            ),
            (
                "MSYS crash before the runtime",
                Some(0xC000_0005),
                Leader::Msys,
                false,
                code(0xC000_0005_u32.cast_signed()),
            ),
            (
                "native job terminated",
                Some(1),
                Leader::Native,
                true,
                signal(9),
            ),
            (
                "MSYS job terminated",
                Some(512),
                Leader::Msys,
                true,
                signal(9),
            ),
            (
                "unreadable",
                None,
                Leader::Native,
                false,
                ExitOutcome {
                    code: None,
                    signal: None,
                },
            ),
        ];
        for (case, raw, leader, terminated, expected) in cases {
            assert_eq!(outcome(raw, leader, terminated), expected, "{case}");
        }
    }
}
