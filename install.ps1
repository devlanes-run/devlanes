# Installs devlanes for Windows from a GitHub release after checking the archive
# against the release's SHA256SUMS.
#
#   irm https://devlanes.run/install.ps1 | iex
#   iex "& {$(irm https://devlanes.run/install.ps1)} -Version 0.4.0"
#   iex "& {$(irm https://devlanes.run/install.ps1)} -Uninstall"
#
# It installs the latest stable release, or with -Version X.Y.Z that release.
# -Dir DIR sets the target directory (default: %LOCALAPPDATA%\Programs\devlanes).
# The latest build of main comes from `devlanes update --next` after installing.
# -Uninstall runs `devlanes uninstall` from that directory (or, without -Dir and
# DEVLANES_INSTALL_DIR, the devlanes.exe on PATH); when there is none, it deletes
# what the script left there and its user PATH entry, and names the data folder.
# `irm | iex` passes no arguments, so $env:DEVLANES_VERSION (X.Y.Z) and
# $env:DEVLANES_INSTALL_DIR, the names install.sh reads, do the same when the option
# isn't given.
# DEVLANES_INSTALL_BASE_URL replaces the release URL; it exists only so the script
# can be tested against a local directory of fake releases (file:// URLs).
#
# When that directory already holds a devlanes.exe with the `update` command, the
# script runs `devlanes update --yes [--version X.Y.Z]` from it instead, unless this
# user cannot write to the directory or X.Y.Z is older than that devlanes (the
# script then warns about the state database and installs it). When the first
# devlanes on PATH comes from another installer (npm, pnpm, Bun, Yarn, uv, pipx or
# cargo), it stops and names it, since that one would keep running; -Force installs
# anyway.
#
# The directory goes first in the user PATH. A devlanes.exe that is running
# (usually the daemon) is renamed to devlanes.exe.old-TIMESTAMP and keeps running;
# when a daemon answers, `devlanes update --finish` (or `devlanes daemon restart` for
# a release without it) restarts it into the new version, and `devlanes doctor` or
# the next install.ps1 run deletes the old file afterwards. It works in Windows
# PowerShell 5.1 and PowerShell 7, on Windows 11 24H2 (build 26100) or later.

# `irm | iex` runs this text in the caller's session, so the body is a script block
# called with `&`: its variables and functions end with it, and `exit` (which would
# close the window) is used only when the script runs as a file. Failures throw, which
# ends a file run with exit code 1 and leaves an iex session open. The options are the
# block's parameters, so they never reach the caller's session either. `@args`
# forwards them by name from the `& { ... } -Version 0.4.0` wrapper above, and is empty
# under plain `irm | iex`.
& {
    param(
        [string]$Version,
        [string]$Dir,
        [switch]$Force,
        [switch]$Uninstall
    )
    $ErrorActionPreference = 'Stop'
    # Windows PowerShell's progress bar slows Invoke-WebRequest down by an order of magnitude.
    $ProgressPreference = 'SilentlyContinue'
    Set-StrictMode -Version 3.0

    # The files of every Windows release archive, below its devlanes-VERSION-TARGET/ directory.
    $DevlanesFiles = @('LICENSE', 'devlanes.exe')

    function Get-DevlanesWindowsBuild {
        <#
        The Windows build from the registry. [Environment]::OSVersion comes from
        GetVersionEx, whose answer depends on the host program's manifest.
        #>
        [int](Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -Name CurrentBuildNumber).CurrentBuildNumber
    }

    function Assert-DevlanesWindowsBuild {
        <# Throws when Build is older than Windows 11 24H2 (build 26100), the oldest devlanes supports. #>
        param([int]$Build)
        $minimum = 26100
        if ($Build -lt $minimum) {
            throw "devlanes install: devlanes needs Windows 11 24H2 (build $minimum) or later, and this is build $Build"
        }
    }

    function Get-DevlanesArchitecture {
        <# The machine's architecture name (AMD64, ARM64, x86). #>
        # The process value says AMD64 inside x64 PowerShell emulated on an arm64 machine
        # and x86 in 32-bit PowerShell; the machine value in the registry names the hardware.
        $machine = [Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE', 'Machine')
        if ($machine) { return $machine }
        if ($env:PROCESSOR_ARCHITEW6432) { return $env:PROCESSOR_ARCHITEW6432 }
        return $env:PROCESSOR_ARCHITECTURE
    }

    function Get-DevlanesTarget {
        <# The release target for an architecture name. #>
        param([string]$Architecture)
        switch ($Architecture) {
            'AMD64' { return 'x86_64-pc-windows-msvc' }
            'ARM64' { return 'aarch64-pc-windows-msvc' }
            default { throw "devlanes install: no release build for Windows $Architecture" }
        }
    }

    function Find-DevlanesArchive {
        <# The archive line for Target in a SHA256SUMS text: Hash, Archive and Version. #>
        param([string]$Sums, [string]$Target, [string]$Version, [string]$Source)
        $suffix = "-$Target.zip"
        foreach ($line in ($Sums -split "`r?`n")) {
            $fields = @($line -split '\s+' | Where-Object { $_ -ne '' })
            if ($fields.Count -ne 2) { continue }
            $archive = $fields[1]
            if (-not $archive.StartsWith('devlanes-') -or -not $archive.EndsWith($suffix)) { continue }
            # The latest release is found through its SHA256SUMS, which names the archive
            # and so the version.
            $found = $archive.Substring('devlanes-'.Length, $archive.Length - 'devlanes-'.Length - $suffix.Length)
            if ($Version -and $found -ne $Version) {
                throw "devlanes install: $Source lists version $found, expected $Version"
            }
            if ($found -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$') {
                throw "devlanes install: unexpected version $found in $Source"
            }
            $hash = $fields[0]
            if ($hash -cnotmatch '^[0-9a-f]{64}$') {
                throw "devlanes install: malformed checksum for $archive"
            }
            return @{ Hash = $hash; Archive = $archive; Version = $found }
        }
        throw "devlanes install: no $Target archive listed in $Source"
    }

    function Get-DevlanesFileHash {
        <# The lowercase hex SHA-256 of a file. #>
        param([string]$Path)
        # Not Get-FileHash: Windows PowerShell defines it in its own Utility module, and
        # with a module path inherited from pwsh (or a shell pwsh started, such as Git
        # Bash) it loads PowerShell 7's Utility module instead, which lacks it.
        $sha = [Security.Cryptography.SHA256]::Create()
        $stream = [IO.File]::OpenRead($Path)
        try {
            return [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLowerInvariant()
        } finally {
            $stream.Dispose()
            $sha.Dispose()
        }
    }

    function Get-DevlanesDownload {
        <# Downloads Url to OutFile; only the listed URL schemes are accepted. #>
        param([string]$Url, [string]$OutFile, [string[]]$Schemes)
        $uri = [Uri]$Url
        if ($Schemes -notcontains $uri.Scheme) {
            throw "devlanes install: refusing $Url (allowed: $($Schemes -join ', '))"
        }
        try {
            if ($uri.Scheme -eq 'file') {
                Copy-Item -LiteralPath $uri.LocalPath -Destination $OutFile
            } else {
                Invoke-WebRequest -Uri $uri -OutFile $OutFile -UseBasicParsing
            }
        } catch {
            throw "devlanes install: cannot download ${Url}: $($_.Exception.Message)"
        }
    }

    function Get-DevlanesPathDirectory {
        <# A PATH entry as a directory to compare: variables expanded, quotes and trailing \ removed. #>
        param([string]$Entry)
        return [Environment]::ExpandEnvironmentVariables($Entry.Trim().Trim('"')).TrimEnd('\')
    }

    function Test-DevlanesPathEntry {
        <# Whether a PATH value has an entry naming Directory (case and trailing \ ignored). #>
        param([AllowEmptyString()][string]$PathValue, [string]$Directory)
        $wanted = $Directory.Trim().TrimEnd('\')
        foreach ($entry in ($PathValue -split ';')) {
            $expanded = Get-DevlanesPathDirectory $entry
            if ($expanded -and $expanded -ieq $wanted) { return $true }
        }
        return $false
    }

    function Find-DevlanesShadow {
        <# The first devlanes command (any PATHEXT extension) in a PATH entry before Directory, or $null. #>
        param([AllowEmptyString()][string]$PathValue, [string]$Directory)
        $wanted = $Directory.Trim().TrimEnd('\')
        $extensions = @(([string]$env:PATHEXT -split ';') | Where-Object { $_ })
        if ($extensions.Count -eq 0) { $extensions = @('.COM', '.EXE', '.BAT', '.CMD') }
        foreach ($entry in ($PathValue -split ';')) {
            $expanded = Get-DevlanesPathDirectory $entry
            if (-not $expanded) { continue }
            if ($expanded -ieq $wanted) { return $null }
            foreach ($extension in $extensions) {
                # File.Exists, unlike Join-Path and Test-Path, returns false for entries
                # with characters a path cannot hold.
                $candidate = "$expanded\devlanes$extension"
                if ([IO.File]::Exists($candidate)) { return $candidate }
            }
        }
        return $null
    }

    function Add-DevlanesPathEntry {
        <# PathValue with Directory in front, or $null when an entry already names it. #>
        param([AllowEmptyString()][string]$PathValue, [string]$Directory)
        if (Test-DevlanesPathEntry $PathValue $Directory) { return $null }
        # The rest stays byte for byte, %VARIABLE% references included.
        if ($PathValue) { return "$Directory;$PathValue" }
        return $Directory
    }

    function Add-DevlanesUserPath {
        <# Puts Directory first in the user PATH; returns whether the value changed. #>
        param([string]$Directory)
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        try {
            # Read unexpanded and written back with the same kind, so entries like
            # %USERPROFILE%\bin in a REG_EXPAND_SZ value keep working;
            # [Environment]::SetEnvironmentVariable would store expanded text as REG_SZ.
            $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
            $current = ''
            if ($key.GetValueNames() -contains 'Path') {
                $kind = $key.GetValueKind('Path')
                $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            }
            $updated = Add-DevlanesPathEntry $current $Directory
            if ($null -eq $updated) { return $false }
            $key.SetValue('Path', $updated, $kind)
        } finally {
            $key.Dispose()
        }
        # Setting a user variable through .NET broadcasts WM_SETTINGCHANGE, so Explorer and
        # terminals started from it afterwards see the new PATH; the registry write alone
        # does not. The variable is removed again at once.
        [Environment]::SetEnvironmentVariable('DEVLANES_INSTALL_PATH_CHANGED', '1', 'User')
        [Environment]::SetEnvironmentVariable('DEVLANES_INSTALL_PATH_CHANGED', $null, 'User')
        return $true
    }

    function Get-DevlanesPathWithoutEntry {
        <# PathValue without its entries naming Directory (compared as Test-DevlanesPathEntry does), or $null when none does. #>
        param([AllowEmptyString()][string]$PathValue, [string]$Directory)
        if (-not (Test-DevlanesPathEntry $PathValue $Directory)) { return $null }
        $wanted = $Directory.Trim().TrimEnd('\')
        # The other entries stay byte for byte, %VARIABLE% references included.
        $kept = @($PathValue -split ';' | Where-Object { (Get-DevlanesPathDirectory $_) -ine $wanted })
        return $kept -join ';'
    }

    function Clear-DevlanesUserPath {
        <# Removes Directory from the user PATH, keeping the value's kind; returns whether the value changed. #>
        param([string]$Directory)
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        try {
            if ($key.GetValueNames() -notcontains 'Path') { return $false }
            $kind = $key.GetValueKind('Path')
            $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $updated = Get-DevlanesPathWithoutEntry $current $Directory
            if ($null -eq $updated) { return $false }
            if ($updated.Trim(';')) {
                $key.SetValue('Path', $updated, $kind)
            } else {
                $key.DeleteValue('Path')
            }
        } finally {
            $key.Dispose()
        }
        # The same WM_SETTINGCHANGE broadcast as Add-DevlanesUserPath.
        [Environment]::SetEnvironmentVariable('DEVLANES_INSTALL_PATH_CHANGED', '1', 'User')
        [Environment]::SetEnvironmentVariable('DEVLANES_INSTALL_PATH_CHANGED', $null, 'User')
        return $true
    }

    function Uninstall-DevlanesRelease {
        <#
        Runs `devlanes uninstall` from the target directory, or the devlanes on PATH when
        no directory was given, in this console so it can ask. Without one, removes the
        files install.ps1 puts in the directory and its user PATH entry.
        #>
        [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '', Justification = 'An installer reports to the console, not to the pipeline.')]
        param([string]$Dir)
        if (-not $Dir) { $Dir = [string]$env:DEVLANES_INSTALL_DIR }
        # An explicit directory is the only place looked at, so a devlanes on PATH that
        # another installer put there is never removed by mistake.
        $explicit = [bool]$Dir
        if (-not $Dir) {
            if (-not $env:LOCALAPPDATA) { throw 'devlanes install: LOCALAPPDATA is not set; pass -Dir' }
            $Dir = Join-Path $env:LOCALAPPDATA 'Programs\devlanes'
        }
        $Dir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Dir)
        $exe = Join-Path $Dir 'devlanes.exe'
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
            $exe = $null
            if (-not $explicit) {
                $found = Get-Command devlanes.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
                if ($found) { $exe = $found.Source }
            }
        }
        if ($exe) {
            & $exe uninstall
            return
        }

        $removed = @()
        $files = @(Get-ChildItem -LiteralPath $Dir -Force -File -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -like 'devlanes.exe.old-*' -or $_.Name -like '.devlanes.exe.new.*' })
        foreach ($file in $files) {
            Remove-Item -LiteralPath $file.FullName -Force
            $removed += $file.FullName
        }
        if (Clear-DevlanesUserPath $Dir) { $removed += "$Dir in your user PATH" }
        if ((Test-Path -LiteralPath $Dir -PathType Container) -and -not (Get-ChildItem -LiteralPath $Dir -Force)) {
            Remove-Item -LiteralPath $Dir -Force
            $removed += $Dir
        }
        if ($removed) {
            Write-Host "No devlanes.exe in $Dir; removed what install.ps1 left:"
            foreach ($item in $removed) { Write-Host "  $item" }
        } else {
            Write-Host "No devlanes.exe in $Dir and nothing install.ps1 left there."
        }
        $data = $env:DEVLANES_HOME
        if (-not $data) { $data = Join-Path $env:USERPROFILE '.devlanes' }
        Write-Host "The devlanes data stays in $data; delete that folder to remove it."
    }

    function Test-DevlanesCommand {
        <#
        Whether Exe exits with 0 for Arguments (single words), run without a window,
        with stdin closed and its output discarded. A program that does not start or
        takes over a minute counts as failing.
        #>
        param([string]$Exe, [string[]]$Arguments)
        $start = New-Object Diagnostics.ProcessStartInfo $Exe
        $start.Arguments = $Arguments -join ' '
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardInput = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        try {
            $process = [Diagnostics.Process]::Start($start)
        } catch {
            return $false
        }
        try {
            $process.StandardInput.Close()
            # Both pipes drain in the background, so a full one never blocks the child.
            $null = $process.StandardOutput.ReadToEndAsync()
            $null = $process.StandardError.ReadToEndAsync()
            if (-not $process.WaitForExit(60000)) {
                $process.Kill()
                return $false
            }
            $process.WaitForExit()
            return $process.ExitCode -eq 0
        } finally {
            $process.Dispose()
        }
    }

    function Get-DevlanesInstalledVersion {
        <#
        The version `Exe --version` prints after `devlanes `, run as Test-DevlanesCommand
        runs it, or $null when it fails or prints anything else.
        #>
        param([string]$Exe)
        $start = New-Object Diagnostics.ProcessStartInfo $Exe
        $start.Arguments = '--version'
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardInput = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        try {
            $process = [Diagnostics.Process]::Start($start)
        } catch {
            return $null
        }
        try {
            $process.StandardInput.Close()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $null = $process.StandardError.ReadToEndAsync()
            if (-not $process.WaitForExit(60000)) {
                $process.Kill()
                return $null
            }
            $process.WaitForExit()
            if ($process.ExitCode -ne 0) { return $null }
            if ($stdout.Result.Trim() -match '^devlanes ([0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?)$') { return $Matches[1] }
            return $null
        } finally {
            $process.Dispose()
        }
    }

    function Test-DevlanesOlderVersion {
        <# Whether release version A sorts before B, by semver precedence. #>
        param([string]$A, [string]$B)
        $left = $A.Split([char[]]'-', 2)
        $right = $B.Split([char[]]'-', 2)
        $leftCore = $left[0].Split('.')
        $rightCore = $right[0].Split('.')
        for ($i = 0; $i -lt 3; $i++) {
            if ([decimal]$leftCore[$i] -ne [decimal]$rightCore[$i]) { return [decimal]$leftCore[$i] -lt [decimal]$rightCore[$i] }
        }
        # A prerelease sorts before its release.
        if ($left.Count -lt 2) { return $false }
        if ($right.Count -lt 2) { return $true }
        $leftIds = $left[1].Split('.')
        $rightIds = $right[1].Split('.')
        for ($i = 0; $i -lt [Math]::Min($leftIds.Count, $rightIds.Count); $i++) {
            $leftNumber = $leftIds[$i] -match '^[0-9]+$'
            $rightNumber = $rightIds[$i] -match '^[0-9]+$'
            if ($leftNumber -and $rightNumber) {
                if ([decimal]$leftIds[$i] -ne [decimal]$rightIds[$i]) { return [decimal]$leftIds[$i] -lt [decimal]$rightIds[$i] }
            } elseif ($leftNumber -or $rightNumber) {
                # Numeric identifiers sort before alphanumeric ones.
                return $leftNumber
            } elseif ($leftIds[$i] -cne $rightIds[$i]) {
                return [string]::CompareOrdinal($leftIds[$i], $rightIds[$i]) -lt 0
            }
        }
        return $leftIds.Count -lt $rightIds.Count
    }

    function Test-DevlanesWritable {
        <# Whether this user can create and delete a file in Directory. #>
        param([string]$Directory)
        $probe = Join-Path $Directory ".devlanes-write-probe.$PID"
        try {
            [IO.File]::WriteAllText($probe, '')
            Remove-Item -LiteralPath $probe -Force
            return $true
        } catch {
            return $false
        }
    }

    function Test-DevlanesHandOff {
        <#
        Whether to run `devlanes update` from Exe instead of installing here: Exe has
        `update`, this user can write to Dir, and Version (when one is asked for) is not
        older than the one Exe runs, which `devlanes update --version` refuses.
        #>
        param([string]$Exe, [string]$Dir, [string]$Version)
        if (-not (Test-Path -LiteralPath $Exe -PathType Leaf) -or -not (Test-DevlanesWritable $Dir)) { return $false }
        if ($Version) {
            $installed = Get-DevlanesInstalledVersion $Exe
            if ($installed -and (Test-DevlanesOlderVersion $Version $installed)) { return $false }
        }
        return Test-DevlanesCommand $Exe @('update', '--help')
    }

    function Test-DevlanesOtherInstaller {
        <#
        Whether a devlanes path, links followed, belongs to a package manager or cargo:
        by its path, or for the copies uv and pipx put in their bin directory, by the
        uv receipt that lists it or the pipx environment that holds the same file.
        #>
        param([string]$Path)
        $text = $Path.Replace('/', '\')
        foreach ($pattern in @('*\npm\*', '*\pnpm\*', '*\.bun\*', '*\Yarn\bin\*', '*node_modules*', '*\uv\tools\*', '*\uv\data\tools\*',
                '*\pipx\venvs\*', '*.cargo\bin\*', '*\Cellar\devlanes\*')) {
            if ($text -like $pattern) { return $true }
        }
        # A pipx environment outside the default pipx home.
        $root = [IO.Path]::GetDirectoryName([IO.Path]::GetDirectoryName($text))
        if ($root -and [IO.File]::Exists((Join-Path $root 'pipx_metadata.json'))) { return $true }

        $uvTools = @($env:UV_TOOL_DIR)
        if ($env:APPDATA) { $uvTools += @((Join-Path $env:APPDATA 'uv\data\tools'), (Join-Path $env:APPDATA 'uv\tools')) }
        foreach ($tools in @($uvTools | Where-Object { $_ })) {
            $receipt = Join-Path $tools 'devlanes\uv-receipt.toml'
            if (-not [IO.File]::Exists($receipt)) { continue }
            # uv writes `install-path = "C:\\...\\devlanes.exe"` for each entry point.
            foreach ($line in [IO.File]::ReadAllLines($receipt)) {
                if ($line -match 'install-path\s*=\s*["'']([^"'']+)["'']' -and $Matches[1].Replace('\\', '\').Replace('/', '\') -ieq $text) { return $true }
            }
        }

        $pipxHomes = @($env:PIPX_HOME)
        if ($env:USERPROFILE) { $pipxHomes += Join-Path $env:USERPROFILE 'pipx' }
        if ($env:LOCALAPPDATA) { $pipxHomes += Join-Path $env:LOCALAPPDATA 'pipx\pipx' }
        $file = Get-Item -LiteralPath $text -Force -ErrorAction SilentlyContinue
        foreach ($pipxHome in @($pipxHomes | Where-Object { $_ })) {
            $venvExe = Get-Item -LiteralPath (Join-Path $pipxHome 'venvs\devlanes\Scripts\devlanes.exe') -Force -ErrorAction SilentlyContinue
            # pipx copies the environment's devlanes.exe when it cannot link to it.
            if ($file -and $venvExe -and $file.Length -eq $venvExe.Length -and
                (Get-DevlanesFileHash $file.FullName) -eq (Get-DevlanesFileHash $venvExe.FullName)) { return $true }
        }
        return $false
    }

    function Resolve-DevlanesLink {
        <# Path with its symbolic links followed (pipx and Scoop-style installs link to their copy). #>
        param([string]$Path)
        for ($n = 0; $n -lt 16; $n++) {
            $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
            # Windows PowerShell also reports a file's other hard links as HardLink
            # targets, which are no link to follow.
            if (-not $item -or $item.LinkType -notin @('SymbolicLink', 'Junction') -or -not $item.Target) { break }
            $target = @($item.Target)[0]
            if (-not [IO.Path]::IsPathRooted($target)) { $target = Join-Path (Split-Path $Path) $target }
            $Path = $target
        }
        return $Path
    }

    function Get-DevlanesDowngradeWarning {
        <#
        The state database warning when Version is older than the devlanes.exe Exe
        runs, the one `devlanes update --stable --force` prints, else $null.
        #>
        param([string]$Exe, [string]$Version)
        if (-not $Version -or -not (Test-Path -LiteralPath $Exe -PathType Leaf)) { return $null }
        $installed = Get-DevlanesInstalledVersion $Exe
        if (-not $installed -or -not (Test-DevlanesOlderVersion $Version $installed)) { return $null }
        return "devlanes $Version is older than the installed $installed. If its daemon refuses the state database the newer build migrated, devlanes daemon reset-state empties it; lanes then get new numbers and ports."
    }

    function Invoke-DevlanesDaemonRestart {
        <#
        Restarts the running daemon into Exe, in this console: `devlanes update --finish`,
        or `devlanes daemon restart --yes` for a release from before `devlanes update`.
        Throws when the restart fails.
        #>
        param([string]$Exe, [string]$Version)
        if (Test-DevlanesCommand $Exe @('update', '--help')) {
            & $Exe update --finish
        } else {
            & $Exe daemon restart --yes
        }
        if ($LASTEXITCODE -ne 0) {
            throw "devlanes install: the daemon did not restart into devlanes $Version; run devlanes daemon restart"
        }
    }

    function Invoke-DevlanesExit {
        <#
        Ends with Code from a devlanes the script ran: `exit` when this runs as a file,
        else (under iex, where `exit` would close the window) a throw when it failed;
        $LASTEXITCODE keeps the code either way.
        #>
        param([int]$Code, [string]$What)
        if ($PSCommandPath) { exit $Code }
        if ($Code -ne 0) { throw "devlanes install: $What exited with $Code" }
    }

    function Clear-DevlanesOldCopy {
        <# Deletes the devlanes.exe.old-* files in Directory that nothing runs any more. #>
        param([string]$Directory)
        foreach ($old in @(Get-ChildItem -LiteralPath $Directory -Filter 'devlanes.exe.old-*' -File -ErrorAction SilentlyContinue)) {
            Remove-Item -LiteralPath $old.FullName -Force -ErrorAction SilentlyContinue
        }
    }

    function Install-DevlanesFile {
        <#
        Renames Staged to Directory\Name. An existing file is renamed to Name.old-STAMP
        first, which Windows allows even while the file runs, and deleted when nothing
        holds it. Returns the old file's path when it is still in use, else $null.
        #>
        param([string]$Staged, [string]$Directory, [string]$Name)
        $target = Join-Path $Directory $Name
        $old = $null
        if (Test-Path -LiteralPath $target) {
            $stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddHHmmss')
            $old = "$target.old-$stamp"
            $n = 1
            while (Test-Path -LiteralPath $old) { $old = "$target.old-$stamp-$n"; $n++ }
            Move-Item -LiteralPath $target -Destination $old
        }
        try {
            Move-Item -LiteralPath $Staged -Destination $target
        } catch {
            if ($old) { Move-Item -LiteralPath $old -Destination $target }
            throw
        }
        if (-not $old) { return $null }
        try {
            Remove-Item -LiteralPath $old -Force
            return $null
        } catch {
            return $old
        }
    }

    function Install-DevlanesRelease {
        <# Downloads, checks and installs a release; see the comment at the top for the settings. #>
        [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '', Justification = 'An installer reports to the console, not to the pipeline.')]
        param([string]$Version, [string]$Dir, [switch]$Force)
        # $version is the -Version parameter (PowerShell names are case-insensitive).
        if (-not $version) { $version = [string]$env:DEVLANES_VERSION }
        if ($version.StartsWith('v')) { $version = $version.Substring(1) }
        if ($version -eq 'next') {
            throw 'devlanes install: next is no longer a version here; install a stable release, then run devlanes update --next'
        }
        if ($version -and $version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$') {
            throw "devlanes install: $version is not a release version (X.Y.Z)"
        }
        # $dir is the -Dir parameter.
        if (-not $dir) { $dir = [string]$env:DEVLANES_INSTALL_DIR }
        if (-not $dir) {
            if (-not $env:LOCALAPPDATA) { throw 'devlanes install: LOCALAPPDATA is not set; pass -Dir' }
            $dir = Join-Path $env:LOCALAPPDATA 'Programs\devlanes'
        }
        # Relative to PowerShell's location, which .NET's current directory does not follow.
        $dir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($dir)

        # A devlanes that another installer manages keeps running from PATH and updates
        # itself its own way, so a copy here would only shadow it or be shadowed.
        $first = Get-Command devlanes -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $Force -and $first) {
            $resolved = Resolve-DevlanesLink $first.Source
            if (Test-DevlanesOtherInstaller $resolved) {
                throw "devlanes install: the devlanes on PATH, $resolved, comes from another installer; update it with: devlanes update (or pass -Force to install here anyway)"
            }
        }

        # A devlanes that can update itself does it, the same way `devlanes update` would,
        # unless this script has to: to install where devlanes cannot write, or an older
        # release than it runs.
        $exe = Join-Path $dir 'devlanes.exe'
        if (Test-DevlanesHandOff -Exe $exe -Dir $dir -Version $version) {
            $arguments = @('update', '--yes')
            if ($version) { $arguments += @('--version', $version) }
            & $exe @arguments
            Invoke-DevlanesExit $LASTEXITCODE 'devlanes update'
            return
        }
        $downgrade = Get-DevlanesDowngradeWarning -Exe $exe -Version $version
        if ($downgrade) { Write-Warning $downgrade }

        Assert-DevlanesWindowsBuild (Get-DevlanesWindowsBuild)
        $target = Get-DevlanesTarget (Get-DevlanesArchitecture)

        $base = 'https://github.com/devlanes-run/devlanes/releases'
        $schemes = @('https')
        if ($env:DEVLANES_INSTALL_BASE_URL) {
            $base = $env:DEVLANES_INSTALL_BASE_URL.TrimEnd('/')
            $schemes = @('https', 'file')
        }

        $temporary = Join-Path ([IO.Path]::GetTempPath()) ('devlanes-install-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $temporary | Out-Null
        $staged = Join-Path $dir ".devlanes.exe.new.$PID"
        try {
            if ($version) {
                $sumsUrl = "$base/download/v$version/SHA256SUMS"
            } else {
                $sumsUrl = "$base/latest/download/SHA256SUMS"
            }
            $sumsFile = Join-Path $temporary 'SHA256SUMS'
            Get-DevlanesDownload -Url $sumsUrl -OutFile $sumsFile -Schemes $schemes
            $release = Find-DevlanesArchive -Sums ([IO.File]::ReadAllText($sumsFile)) -Target $target -Version $version -Source $sumsUrl
            $version = $release.Version

            # The archive comes from that version's own release.
            $url = "$base/download/v$version/$($release.Archive)"
            $archive = Join-Path $temporary $release.Archive
            Get-DevlanesDownload -Url $url -OutFile $archive -Schemes $schemes
            $actual = Get-DevlanesFileHash $archive
            if ($actual -ne $release.Hash) {
                throw "devlanes install: checksum mismatch for $($release.Archive): expected $($release.Hash), got $actual"
            }

            # Only the exact entries release archives contain. devlanes.exe is extracted
            # straight next to its target, so replacing it is a rename on one volume and
            # a full disk or a failed write leaves the installed version untouched.
            $top = "devlanes-$version-$target"
            $expected = @($DevlanesFiles | ForEach-Object { "$top/$_" })
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
            Add-Type -AssemblyName System.IO.Compression
            Add-Type -AssemblyName System.IO.Compression.FileSystem
            $zip = [IO.Compression.ZipFile]::OpenRead($archive)
            try {
                $names = @($zip.Entries | ForEach-Object { $_.FullName } | Where-Object { $_ -ne "$top/" } | Sort-Object)
                if (($names -join '|') -cne (($expected | Sort-Object) -join '|')) {
                    throw "devlanes install: unexpected entries in $($release.Archive): $($names -join ', ')"
                }
                $entry = $zip.GetEntry("$top/devlanes.exe")
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $staged, $true)
            } finally {
                $zip.Dispose()
            }

            Clear-DevlanesOldCopy $dir
            $inUse = Install-DevlanesFile -Staged $staged -Directory $dir -Name 'devlanes.exe'
        } finally {
            Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
        }

        Write-Host "Installed devlanes $version to $dir"
        # A daemon keeps running the binary it started from until it restarts into this one.
        if (Test-DevlanesCommand $exe @('daemon', 'status')) {
            Invoke-DevlanesDaemonRestart -Exe $exe -Version $version
        } elseif ($inUse) {
            Write-Host "The previous devlanes.exe is still running and keeps running as $inUse."
            Write-Host 'Run `devlanes daemon restart` when convenient (it stops services); `devlanes doctor` then deletes the old file.'
        }

        if (Add-DevlanesUserPath $dir) {
            Write-Host "Added $dir to your user PATH; terminals opened from now on find devlanes."
        }
        if (-not (Test-DevlanesPathEntry $env:Path $dir)) {
            $env:Path = "$dir;$env:Path"
        }
        # New terminals get the machine PATH followed by the user PATH, so a devlanes in
        # the machine PATH (or before this directory in the user PATH) runs instead.
        $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $shadow = Find-DevlanesShadow "$machinePath;$userPath" $dir
        if ($shadow) {
            Write-Host "Another devlanes comes first on PATH: $shadow"
        }
    }

    # A script block without [CmdletBinding()] puts unknown options in $args instead of
    # refusing them, and a removed one such as -Channel must not install silently.
    if ($args) { throw "devlanes install: unknown option $($args -join ' ')" }
    # Windows PowerShell 5.1 may not offer TLS 1.2, which GitHub requires, when the
    # .NET defaults were changed; the process-wide setting is restored afterwards.
    if ($Uninstall) {
        if ($Version -or $Force) { throw "devlanes install: -Uninstall takes no -Version or -Force" }
        Uninstall-DevlanesRelease -Dir $Dir
        return
    }
    $protocols = [Net.ServicePointManager]::SecurityProtocol
    try {
        if ($PSVersionTable.PSVersion.Major -lt 6 -and $protocols -ne [Net.SecurityProtocolType]::SystemDefault) {
            [Net.ServicePointManager]::SecurityProtocol = $protocols -bor [Net.SecurityProtocolType]::Tls12
        }
        Install-DevlanesRelease -Version $Version -Dir $Dir -Force:$Force
    } finally {
        [Net.ServicePointManager]::SecurityProtocol = $protocols
    }
} @args
