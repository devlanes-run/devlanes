#!/bin/sh
# Installs devlanes and devlanes-hosts from a GitHub release after checking the
# archive against the release's SHA256SUMS.
#
#   curl -fsSL https://devlanes.run/install.sh | sh
#   curl -fsSL https://devlanes.run/install.sh | sh -s -- --version 0.4.0
#   curl -fsSL https://devlanes.run/install.sh | sh -s -- --uninstall
#
# It installs the latest stable release, or with -v, --version X.Y.Z that release.
# -d, --dir DIR sets the target directory (default: ~/.local/bin). DEVLANES_VERSION
# (X.Y.Z) and DEVLANES_INSTALL_DIR do the same when the option isn't given. The
# latest build of main comes from `devlanes update --next` after installing.
# When that directory already holds a devlanes with the `update` command, the script
# runs `devlanes update --yes [--version X.Y.Z]` from it instead and exits with its
# status, unless the directory is not writable (the script may use sudo there) or
# X.Y.Z is older than that devlanes (`devlanes update` refuses it). After installing, a running daemon is restarted into the new version
# (`devlanes update --finish`).
# When the first devlanes on PATH comes from another installer (Homebrew, npm, pnpm,
# Bun, Yarn, uv, pipx or cargo), the script stops and names it, since that one would
# keep running; --force installs anyway.
# --uninstall runs `devlanes uninstall` from that directory (or, without --dir and
# DEVLANES_INSTALL_DIR, the devlanes on PATH) on the terminal; when there is none, it
# deletes the files the script installs there and names the data directory.
# DEVLANES_INSTALL_SUDO=1 allows sudo when that directory is not writable.
# DEVLANES_INSTALL_BASE_URL replaces the release URL; it exists only so the
# script can be tested against a local directory of fake releases.
set -eu

base=${DEVLANES_INSTALL_BASE_URL:-https://github.com/devlanes-run/devlanes/releases}
proto='=https'
if [ -n "${DEVLANES_INSTALL_BASE_URL:-}" ]; then proto='=https,file'; fi
version=${DEVLANES_VERSION:-}
dir=${DEVLANES_INSTALL_DIR:-$HOME/.local/bin}
dir_set=${DEVLANES_INSTALL_DIR:+1}

fail() {
	printf 'devlanes install: %s\n' "$1" >&2
	exit 1
}

usage='usage: sh install.sh [-v, --version X.Y.Z] [-d, --dir DIR] [--force] [--uninstall]'
usage() {
	printf '%s\n' "$usage" >&2
	exit 2
}

# Runs devlanes with the terminal as stdin when there is one: under `curl | sh` stdin
# is the script. A subshell tests it, since a failed redirection may end a shell
# running `:`.
on_tty() {
	if (: </dev/tty) 2>/dev/null; then
		"$@" </dev/tty
	else
		"$@"
	fi
}

version_set=
uninstall=
force=
while [ "$#" -gt 0 ]; do
	case $1 in
	--uninstall)
		uninstall=1
		shift
		continue
		;;
	--force)
		force=1
		shift
		continue
		;;
	-v | --version | -d | --dir)
		[ "$#" -ge 2 ] || usage
		option=$1 value=$2
		shift 2
		;;
	--version=* | --dir=*)
		option=${1%%=*} value=${1#*=}
		shift
		;;
	-h | --help)
		printf '%s\n' "$usage"
		exit 0
		;;
	*)
		printf 'devlanes install: unknown option %s\n' "$1" >&2
		usage
		;;
	esac
	[ -n "$value" ] || usage
	case $option in
	-v | --version) version=$value version_set=1 ;;
	-d | --dir) dir=$value dir_set=1 ;;
	esac
done

case "$dir" in
/*) ;;
*) dir=$(pwd)/$dir ;;
esac

if [ -n "$uninstall" ]; then
	[ -z "$version_set$force" ] || fail "--uninstall takes no --version or --force"
	exe=
	if [ -x "$dir/devlanes" ]; then
		exe=$dir/devlanes
	# An explicit directory is the only place looked at, so a devlanes on PATH that
	# another installer put there is never removed by mistake.
	elif [ -z "$dir_set" ]; then
		exe=$(command -v devlanes 2>/dev/null || true)
	fi
	if [ -n "$exe" ]; then
		# Under `curl | sh` stdin is the script; devlanes asks on the terminal.
		# A subshell, since a failed redirection may end a shell running `:`.
		if (: </dev/tty) 2>/dev/null; then
			exec "$exe" uninstall </dev/tty
		fi
		exec "$exe" uninstall
	fi
	removed=
	for bin in devlanes devlanes-hosts; do
		if [ -e "$dir/$bin" ]; then
			rm -f "$dir/$bin"
			removed="$removed $dir/$bin"
		fi
	done
	if [ -n "$removed" ]; then
		printf 'No devlanes in %s; removed:%s\n' "$dir" "$removed"
	else
		printf 'No devlanes in %s and nothing install.sh left there.\n' "$dir"
	fi
	printf 'The devlanes data stays in %s; delete that directory to remove it.\n' "${DEVLANES_HOME:-$HOME/.devlanes}"
	exit 0
fi

version=${version#v}
release_version='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
if [ "$version" = next ]; then
	fail "next is no longer a version here; install a stable release, then run devlanes update --next"
fi
if [ -n "$version" ] && ! printf '%s\n' "$version" | grep -Eq "$release_version"; then
	fail "$version is not a release version (X.Y.Z)"
fi

# Follows symlinks, as Homebrew, npm, uv and pipx put a link to their copy on PATH.
resolve() {
	path=$1
	while [ -L "$path" ]; do
		link=$(readlink "$path")
		case $link in
		/*) path=$link ;;
		*) path=$(dirname "$path")/$link ;;
		esac
	done
	parent=$(cd "$(dirname "$path")" 2>/dev/null && pwd -P) && path=$parent/${path##*/}
	printf '%s\n' "$path"
}

# A devlanes that another installer manages keeps running from PATH and updates itself
# its own way, so a copy here would only shadow it or be shadowed.
first=$(command -v devlanes 2>/dev/null || true)
if [ -z "$force" ] && [ -n "$first" ]; then
	resolved=$(resolve "$first")
	case $resolved in
	*/Cellar/devlanes/* | *node_modules* | */uv/tools/* | */pipx/venvs/* | *.cargo/bin/*)
		fail "the devlanes on PATH, $resolved, comes from another installer; update it with: devlanes update (or pass --force to install here anyway)"
		;;
	esac
fi

# Whether release version $1 sorts before $2, by semver precedence.
older_than() {
	awk -v a="$1" -v b="$2" '
	function split_version(v, core, pre,   at) {
		at = index(v, "-")
		if (at) { pre[0] = substr(v, at + 1); v = substr(v, 1, at - 1) } else { pre[0] = "" }
		split(v, core, ".")
	}
	BEGIN {
		split_version(a, ac, ap)
		split_version(b, bc, bp)
		for (i = 1; i <= 3; i++) {
			if (ac[i] + 0 < bc[i] + 0) exit 0
			if (ac[i] + 0 > bc[i] + 0) exit 1
		}
		# A prerelease sorts before its release.
		if (ap[0] == "") exit 1
		if (bp[0] == "") exit 0
		n = split(ap[0], x, ".")
		m = split(bp[0], y, ".")
		for (i = 1; i <= n && i <= m; i++) {
			xn = x[i] ~ /^[0-9]+$/
			yn = y[i] ~ /^[0-9]+$/
			if (xn && yn) {
				if (x[i] + 0 < y[i] + 0) exit 0
				if (x[i] + 0 > y[i] + 0) exit 1
			} else if (xn) {
				exit 0
			} else if (yn) {
				exit 1
			} else if (x[i] != y[i]) {
				exit (x[i] < y[i]) ? 0 : 1
			}
		}
		exit (n < m) ? 0 : 1
	}'
}

# A devlanes that can update itself does it, the same way `devlanes update` would,
# unless this script has to: to install where this user cannot write (it may use
# sudo), or an older release than that devlanes, which `devlanes update` refuses.
installed=
if [ -x "$dir/devlanes" ]; then
	installed=$("$dir/devlanes" --version 2>/dev/null </dev/null | sed -n 's/^devlanes //p' || true)
fi
if [ -x "$dir/devlanes" ] && [ -w "$dir" ] &&
	{ [ -z "$version" ] || [ -z "$installed" ] || ! older_than "$version" "$installed"; } &&
	"$dir/devlanes" update --help >/dev/null 2>&1 </dev/null; then
	status=0
	if [ -n "$version" ]; then
		on_tty "$dir/devlanes" update --yes --version "$version" || status=$?
	else
		on_tty "$dir/devlanes" update --yes || status=$?
	fi
	exit "$status"
fi

for tool in curl tar uname mktemp awk; do
	command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done
if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
	sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
	fail "sha256sum or shasum is required"
fi

os=$(uname -s)
arch=$(uname -m)
# A shell running under Rosetta reports x86_64; the arm64 build is the native one.
if [ "$os" = Darwin ] && [ "$arch" = x86_64 ] &&
	[ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
	arch=arm64
fi
case "$os:$arch" in
Darwin:arm64 | Darwin:aarch64) target=aarch64-apple-darwin ;;
Darwin:x86_64) target=x86_64-apple-darwin ;;
Linux:x86_64 | Linux:amd64) target=x86_64-unknown-linux-gnu ;;
Linux:aarch64 | Linux:arm64) target=aarch64-unknown-linux-gnu ;;
*) fail "no release build for $os $arch" ;;
esac
if [ "$os" = Linux ] && ls /lib/ld-musl-* >/dev/null 2>&1; then
	fail "the Linux builds need glibc; this system uses musl"
fi

tmp=$(mktemp -d)
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

download() {
	curl -fsSL --proto "$proto" --proto-redir "$proto" --tlsv1.2 "$1" -o "$2"
}

if [ -n "$version" ]; then
	sums_url=$base/download/v$version/SHA256SUMS
else
	sums_url=$base/latest/download/SHA256SUMS
fi
download "$sums_url" "$tmp/SHA256SUMS" || fail "cannot download $sums_url"

# The latest release is found through its SHA256SUMS, which names the archive and
# so the version; the archive then comes from that version's own release.
line=$(awk -v suffix="-$target.tar.gz" '
	length($2) > length(suffix) && substr($2, length($2) - length(suffix) + 1) == suffix { print; exit }
' "$tmp/SHA256SUMS")
[ -n "$line" ] || fail "no $target archive listed in $sums_url"
expected=${line%% *}
archive=${line##* }
found=${archive#devlanes-}
found=${found%"-$target.tar.gz"}
if [ -n "$version" ] && [ "$found" != "$version" ]; then
	fail "$sums_url lists version $found, expected $version"
fi
version=$found
printf '%s\n' "$version" | grep -Eq "$release_version" ||
	fail "unexpected version $version in $sums_url"
printf '%s\n' "$expected" | grep -Eq '^[0-9a-f]{64}$' || fail "malformed checksum for $archive"

url=$base/download/v$version/$archive
download "$url" "$tmp/$archive" || fail "cannot download $url"
actual=$(sha256 "$tmp/$archive")
[ "$actual" = "$expected" ] || fail "checksum mismatch for $archive: expected $expected, got $actual"

# Extract only the exact entries release archives contain.
top=devlanes-$version-$target
listed=$(tar -tzf "$tmp/$archive" | sed 's#/$##' | LC_ALL=C sort | tr '\n' ' ')
[ "$listed" = "$top $top/LICENSE $top/devlanes $top/devlanes-hosts " ] ||
	fail "unexpected entries in $archive: $listed"
tar -xzf "$tmp/$archive" -C "$tmp"

sudo=
if ! mkdir -p "$dir" 2>/dev/null || [ ! -w "$dir" ]; then
	if [ "${DEVLANES_INSTALL_SUDO:-}" != 1 ]; then
		fail "$dir is not writable; pass --dir with a writable directory, or DEVLANES_INSTALL_SUDO=1 to install there with sudo"
	fi
	command -v sudo >/dev/null 2>&1 || fail "$dir is not writable and sudo is not available"
	printf 'devlanes install: %s is not writable; using sudo to install there\n' "$dir"
	sudo=sudo
fi
$sudo mkdir -p "$dir"
for bin in devlanes devlanes-hosts; do
	# Rename into place so a running devlanes keeps its open executable.
	$sudo cp "$tmp/$top/$bin" "$dir/.$bin.new.$$"
	$sudo chmod 755 "$dir/.$bin.new.$$"
	$sudo mv -f "$dir/.$bin.new.$$" "$dir/$bin"
done

printf 'Installed devlanes %s to %s\n' "$version" "$dir"
# A daemon keeps running the binary it started from until it restarts into this one.
if "$dir/devlanes" daemon status >/dev/null 2>&1 </dev/null; then
	on_tty "$dir/devlanes" update --finish ||
		fail "the daemon did not restart into devlanes $version; run devlanes daemon restart"
fi
case ":${PATH:-}:" in
*":$dir:"*)
	first=$(command -v devlanes 2>/dev/null || true)
	if [ -n "$first" ] && [ "$first" != "$dir/devlanes" ]; then
		printf 'Another devlanes comes first on PATH: %s\n' "$first"
	fi
	;;
*)
	printf '%s is not on PATH. Add it to your shell profile:\n  export PATH="%s:$%s"\n' "$dir" "$dir" PATH
	;;
esac
