//! Finding devlanes on the box. The box installs devlanes itself; a box
//! without it is reported, never changed.
//!
//! The probe first runs as an `sh -c` script. A Windows box's sshd runs
//! commands with `cmd.exe` (Win32-OpenSSH's default) or PowerShell, which have
//! no `sh`, so when that probe prints nothing usable, or reports a Windows
//! uname (Git Bash), a PowerShell probe follows. It reports the same tagged
//! lines plus the shell that ran it, which decides how the tunnel command is
//! quoted ([`BoxShell::tunnel_command`]).

use base64::Engine as _;

use crate::sh_quote;

/// Where the probe looks for the binary after `PATH`, relative to the remote `$HOME`.
pub const INSTALL_DIR: &str = ".devlanes/bin";

/// Where the error for a box without devlanes points the user.
pub(crate) const SETUP_GUIDE: &str =
    "https://github.com/devlanes-run/devlanes/blob/main/docs/guides/remote-setup.md#on-the-box";

/// [`RemotePlatform::os`] of a Windows box.
pub(crate) const WINDOWS: &str = "windows";

/// Normalized OS and architecture reported by `uname`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePlatform {
    /// `linux`, `macos`, `windows`, or the raw lowercase `uname -s` value.
    pub os: String,
    /// `x86_64`, `aarch64`, or the raw `uname -m` value.
    pub arch: String,
}

impl RemotePlatform {
    /// Maps `uname -s` / `uname -m` output, or the Windows probe's
    /// `Windows` and `PROCESSOR_ARCHITECTURE`, onto short names. Git Bash,
    /// MSYS2 and Cygwin report `MINGW64_NT-10.0-26100` and the like, which
    /// are Windows too.
    pub fn from_uname(sysname: &str, machine: &str) -> Self {
        let sysname = sysname.trim();
        let os = match sysname {
            "Linux" => "linux".to_owned(),
            "Darwin" => "macos".to_owned(),
            "Windows" | "Windows_NT" => WINDOWS.to_owned(),
            other
                if ["MINGW", "MSYS", "CYGWIN"]
                    .iter()
                    .any(|p| other.starts_with(p)) =>
            {
                WINDOWS.to_owned()
            }
            other => other.to_ascii_lowercase(),
        };
        let arch = match machine.trim() {
            "x86_64" | "amd64" | "AMD64" => "x86_64".to_owned(),
            "aarch64" | "arm64" | "ARM64" => "aarch64".to_owned(),
            other => other.to_owned(),
        };
        Self { os, arch }
    }
}

/// The shell the box's sshd runs commands with, which decides how the tunnel
/// command is quoted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BoxShell {
    /// A POSIX shell: every Linux and macOS box, and a Windows box whose
    /// default shell is Git Bash or another `sh`.
    #[default]
    Sh,
    /// `cmd.exe`, Win32-OpenSSH's default shell.
    Cmd,
    /// Windows PowerShell or PowerShell 7.
    PowerShell,
}

impl BoxShell {
    /// The shell the Windows probe named (the program that ran it, else
    /// Win32-OpenSSH's `DefaultShell`). A name it does not know is taken as
    /// `cmd.exe`, the default whose quoting most Windows shells share.
    fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "powershell" | "pwsh" => Self::PowerShell,
            "sh" | "bash" | "dash" | "zsh" | "ksh" | "ash" => Self::Sh,
            _ => Self::Cmd,
        }
    }

    /// How messages name the shell.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sh => "sh",
            Self::Cmd => "cmd.exe",
            Self::PowerShell => "PowerShell",
        }
    }

    /// The command that runs `<program> tunnel --stdio` in this shell; see
    /// [`BoxShell::command`].
    pub fn tunnel_command(self, program: &str) -> Option<String> {
        self.command(program, "tunnel --stdio")
    }

    /// The command that runs `program` with `args` in this shell, with
    /// `program` as one word whatever spaces, parentheses or quotes it holds;
    /// `None` when this shell cannot take it as it is. `args` are plain words
    /// that need no quoting in any of the shells, such as `daemon status`.
    ///
    /// - sh: single quotes.
    /// - cmd.exe: double quotes. sshd passes the whole command as
    ///   `cmd.exe /c "<command>"`, and cmd strips that outer pair since the
    ///   command holds more quotes, so the program's pair stays. Inside quotes
    ///   cmd still expands `%NAME%`, and `!NAME!` where delayed expansion is
    ///   on by default (a registry setting), and a path cannot hold `"`, so a
    ///   program with `%`, `!`, `"` or a control character is refused.
    /// - PowerShell: the call operator on a single-quoted string, and the
    ///   program's exit status passed on, since PowerShell's `-c` otherwise
    ///   exits 1 for any failure (the box daemon's stopped status included).
    pub fn command(self, program: &str, args: &str) -> Option<String> {
        match self {
            Self::Sh => Some(format!("{} {args}", sh_quote(program))),
            Self::Cmd => {
                if program
                    .chars()
                    .any(|c| matches!(c, '"' | '%' | '!') || c.is_control())
                {
                    return None;
                }
                Some(format!("\"{program}\" {args}"))
            }
            Self::PowerShell => Some(format!(
                "& {} {args}; exit $LASTEXITCODE",
                ps_quote(program)
            )),
        }
    }
}

/// What the box reported about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteProbe {
    /// Box platform.
    pub platform: RemotePlatform,
    /// Path or name to run, or `None` when devlanes is not installed.
    pub program: Option<String>,
    /// The shell that runs the box's commands; [`BoxShell::Sh`] unless the
    /// Windows probe named another.
    pub shell: BoxShell,
}

/// What every probe line starts with, so output a login shell or profile file
/// prints around it is told apart.
const PROBE_TAG: &str = "devlanes-probe:";

/// Builds the probe script: it prints tagged `os=`, `arch=` and `program=`
/// lines, the last one empty when devlanes is not found, and leaves the program
/// found in `p`. The newline first puts the tags on lines of their own even
/// after login output without a final newline.
///
/// The script holds none of cmd.exe's operators (`< > & | ^`): on a Windows
/// box cmd.exe parses the whole `sh -c '<script>'` line first, knows nothing
/// of single quotes, and would carry out a `>/dev/null` itself (creating
/// `\dev\null` on the current drive) or split the line at `&&`. `command -v`
/// prints nothing when the program is missing, so its output alone decides.
pub(crate) fn probe_script(program: &str) -> String {
    let program = sh_quote(program);
    format!(
        "p=$(command -v {program}); \
         if [ -z \"$p\" ]; then if [ -x \"$HOME/{INSTALL_DIR}/devlanes\" ]; then \
         p=\"$HOME/{INSTALL_DIR}/devlanes\"; fi; fi; \
         printf '\\n{PROBE_TAG}os=%s\\n{PROBE_TAG}arch=%s\\n{PROBE_TAG}program=%s\\n' \
         \"$(uname -s)\" \"$(uname -m)\" \"$p\""
    )
}

/// Parses probe output, ignoring lines that are not the probe's own; `None`
/// when the platform lines are missing.
pub(crate) fn parse_probe(output: &str) -> Option<RemoteProbe> {
    let mut sysname = None;
    let mut machine = None;
    let mut program = None;
    let mut shell = BoxShell::Sh;
    for line in output.lines() {
        let Some((key, value)) = line
            .trim_end_matches('\r')
            .strip_prefix(PROBE_TAG)
            .and_then(|rest| rest.split_once('='))
        else {
            continue;
        };
        // The last of each wins: the probe prints after any login output.
        match key {
            "os" => sysname = Some(value),
            "arch" => machine = Some(value),
            "program" => program = Some(value.trim()),
            "shell" => shell = BoxShell::from_name(value),
            _ => {}
        }
    }
    Some(RemoteProbe {
        platform: RemotePlatform::from_uname(sysname?, machine?),
        program: program.filter(|p| !p.is_empty()).map(str::to_owned),
        shell,
    })
}

/// Whether the `sh` probe's answer calls for the Windows probe: there was
/// none (cmd.exe and PowerShell have no `sh` and print only their error on
/// stderr), or it came from Git Bash on Windows, whose paths and quoting
/// cmd.exe could not use if that `sh` was only found on `PATH`.
pub(crate) fn needs_windows_probe(probe: Option<&RemoteProbe>) -> bool {
    probe.is_none_or(|probe| probe.platform.os == WINDOWS)
}

/// Bytes of the Windows probe's stderr a probe error keeps, from the end.
const PROBE_STDERR_TAIL: usize = 2048;

/// The text of a probe error: what the `sh` probe printed, then the end of
/// what the Windows probe wrote on stderr when it ran and said something,
/// which is where a box that blocks PowerShell explains why.
pub(crate) fn probe_failure(sh_stdout: &str, windows_stderr: Option<&[u8]>) -> String {
    let sh = sh_stdout.trim();
    let Some(stderr) = windows_stderr else {
        return sh.to_owned();
    };
    let start = stderr.len().saturating_sub(PROBE_STDERR_TAIL);
    let tail = String::from_utf8_lossy(&stderr[start..]);
    let tail = tail.trim();
    if tail.is_empty() {
        sh.to_owned()
    } else {
        format!("{sh}\nthe PowerShell probe printed on stderr: {tail}")
    }
}

/// Finds devlanes and the shell, as Windows PowerShell 5.1 and PowerShell 7
/// both run it, into `$p` (the program, empty when not found), `$a` (the
/// architecture) and `$s` (the shell), and defines `Out-Probe`, which writes
/// UTF-8 bytes to stdout untranslated. `@NAME@` is the quoted program name.
///
/// On a machine that is not Windows it exits, so a `powershell` on a Linux
/// box never reports one. Of what `Get-Command` finds, only files whose
/// extension is in `PATHEXT` count (it also lists a `devlanes` file without
/// one, which cmd.exe cannot start), and an `.exe` comes before a `.cmd`
/// shim. The shell is the program that started this PowerShell: sshd runs the
/// probe through it. When that is not a known shell (`Get-CimInstance` failed,
/// or something else stands between), Win32-OpenSSH's `DefaultShell` value
/// names it, and without one sshd uses cmd.exe.
const WINDOWS_PROBE: &str = r#"$ProgressPreference='SilentlyContinue'
$ErrorActionPreference='SilentlyContinue'
if([Environment]::OSVersion.Platform -ne 'Win32NT'){exit 1}
$p=''
$e=@(if($env:PATHEXT){$env:PATHEXT -split ';'}else{'.COM','.EXE','.BAT','.CMD'})
$c=@(Get-Command -Name @NAME@ -CommandType Application|Where-Object{$_.Extension -and $e -contains $_.Extension})
$x=@($c|Where-Object{$_.Extension -eq '.exe'})
if($x.Count){$p=$x[0].Path}elseif($c.Count){$p=$c[0].Path}
$f=Join-Path $env:USERPROFILE '.devlanes\bin\devlanes.exe'
if(-not $p -and (Test-Path -LiteralPath $f -PathType Leaf)){$p=$f}
$a=$env:PROCESSOR_ARCHITEW6432
if(-not $a){$a=$env:PROCESSOR_ARCHITECTURE}
$s=''
try{$q=(Get-CimInstance Win32_Process -Filter "ProcessId=$PID").ParentProcessId;$s=(Get-Process -Id $q).ProcessName.ToLowerInvariant()}catch{}
if($s -notin 'cmd','powershell','pwsh','sh','bash','dash','zsh','ksh','ash'){$s=[IO.Path]::GetFileNameWithoutExtension((Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\OpenSSH').DefaultShell)}
if(-not $s){$s='cmd'}
function Out-Probe($t){$b=[Text.Encoding]::UTF8.GetBytes($t);$o=[Console]::OpenStandardOutput();$o.Write($b,0,$b.Length);$o.Flush()}
$t="`n@TAG@os=Windows`n@TAG@arch=$a`n@TAG@program=$p`n@TAG@shell=$s`n"
"#;

/// The probe body for `program`, ending with the tagged lines in `$t`.
fn windows_probe_body(program: &str) -> String {
    WINDOWS_PROBE
        .replace("@NAME@", &ps_quote(program))
        .replace("@TAG@", PROBE_TAG)
}

/// The command that runs the Windows probe for `program`: PowerShell with
/// the script base64-encoded, which needs no quoting in cmd.exe, PowerShell
/// or sh, so it runs the same whichever of them sshd uses.
pub(crate) fn windows_probe_command(program: &str) -> String {
    encoded_powershell(&format!("{}Out-Probe $t\n", windows_probe_body(program)))
}

/// The command a Windows client runs on a Windows box: a `start` line, what
/// [`windows_probe_command`] prints, an `end` line, then `<program> tunnel
/// --stdio` with its exit status when devlanes was found, as
/// [`session_script`] does on a POSIX box. PowerShell starts the tunnel
/// with this session's stdin and stdout, so its bytes pass untouched.
///
/// `shell` is the box's shell as far as it is known. sshd runs the command
/// through it, and a PowerShell there would turn the tunnel's exit status
/// (the box daemon's stopped status included) into 1, so the command then
/// passes the status on itself.
#[cfg(windows)]
pub(crate) fn windows_session_command(
    program: &str,
    start: &str,
    end: &str,
    shell: BoxShell,
) -> String {
    let command = encoded_powershell(&format!(
        "{}Out-Probe (\"`n{start}`n\"+$t+\"{end}`n\")\n\
         if($p){{& $p tunnel --stdio; exit $LASTEXITCODE}}\n",
        windows_probe_body(program)
    ));
    match shell {
        BoxShell::PowerShell => format!("{command}; exit $LASTEXITCODE"),
        BoxShell::Sh | BoxShell::Cmd => command,
    }
}

/// `powershell -NoProfile -NonInteractive -EncodedCommand <script>`: the
/// script as base64 of its UTF-16LE form, as `-EncodedCommand` takes it.
fn encoded_powershell(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// Quotes `value` as a PowerShell single-quoted string. PowerShell also
/// reads the typographic single quotes as quotes, so each of them is doubled
/// too.
fn ps_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for c in value.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            quoted.push(c);
        }
        quoted.push(c);
    }
    quoted.push('\'');
    quoted
}

/// Builds the script a Windows session runs: a `start` line, what
/// [`probe_script`] prints, an `end` line, then `exec <program> tunnel
/// --stdio` when devlanes was found. Everything after `end` belongs to the
/// tunnel, which writes its preamble first, so probe and tunnel share one ssh
/// connection and one authentication. The newline before `start` puts it on a
/// line of its own even after login output without a final newline.
#[cfg(windows)]
pub(crate) fn session_script(program: &str, start: &str, end: &str) -> String {
    let probe = probe_script(program);
    let start = sh_quote(start);
    let end = sh_quote(end);
    format!(
        "printf '\\n%s\\n' {start}; {probe}; printf '%s\\n' {end}; \
         if [ -n \"$p\" ]; then exec \"$p\" tunnel --stdio; fi"
    )
}

/// Parses the probe lines a [`session_script`] printed after its last `start`
/// line, so whatever a login shell's startup files print first is ignored.
#[cfg(windows)]
pub(crate) fn parse_session_header(lines: &[String], start: &str) -> Option<RemoteProbe> {
    let at = lines.iter().rposition(|line| line == start)?;
    parse_probe(&lines[at + 1..].join("\n"))
}

#[cfg(test)]
mod tests;
