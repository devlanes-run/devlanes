#!/bin/sh
# Dry run of the release scripts against fake binaries: package.sh and package.ps1
# for every target, manifest.sh, the Homebrew formula, the npm packages (packed,
# then installed from the tarballs on macOS and Linux arm64/x64), the PyPI wheels
# (installed into a venv) and install.sh from a local release directory. Needs tar,
# gzip, unzip, jq, node, npm, python3 with venv, curl and pwsh, and on Linux glibc
# 2.35 or later for pip to take the wheel; CI runs it on every push.
# check.ps1 covers package.ps1, install.ps1, the launcher and the wheel on Windows.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
cleanup() { rm -rf "$work"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# A devlanes installed on this machine is not another installer's to install.sh here.
clean_path=
old_ifs=$IFS
IFS=:
for entry in $PATH; do
	[ -e "$entry/devlanes" ] || clean_path=${clean_path:+$clean_path:}$entry
done
IFS=$old_ifs
PATH=$clean_path
export PATH

# A prerelease that PyPI also takes, so the wheels come from the same archives.
version=0.0.0-rc.1
mkdir "$work/bin"
printf '#!/bin/sh\necho "%s %s"\n' devlanes-hosts "$version" >"$work/bin/devlanes-hosts"
# The fake devlanes prints its version. It has `update` and a running daemon only when
# DEVLANES_CHECK_LOG is set: then it logs those calls there, answers the probes
# (`update --help`, `daemon status`) and exits with DEVLANES_CHECK_EXIT (default 0)
# otherwise. Without it, it fails them like a devlanes from before `devlanes update`
# with no daemon. DEVLANES_CHECK_NO_UPDATE=1 keeps the daemon but fails `update`
# unlogged, like a release from before `devlanes update` whose daemon runs.
cat >"$work/bin/devlanes" <<EOF
#!/bin/sh
case \${1:-} in
update | daemon)
	[ -n "\${DEVLANES_CHECK_LOG:-}" ] || exit 2
	if [ "\$1" = update ] && [ -n "\${DEVLANES_CHECK_NO_UPDATE:-}" ]; then exit 2; fi
	printf '%s\n' "\$*" >>"\$DEVLANES_CHECK_LOG"
	case "\$*" in
	'update --help' | 'daemon status') exit 0 ;;
	esac
	exit "\${DEVLANES_CHECK_EXIT:-0}"
	;;
esac
echo "devlanes $version"
EOF
chmod 755 "$work/bin/devlanes" "$work/bin/devlanes-hosts"

for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
	sh "$root/packaging/scripts/package.sh" "$version" "$target" "$work/dist" "$work/bin" >/dev/null
done
mkdir "$work/winbin"
printf 'fake devlanes.exe %s\n' "$version" >"$work/winbin/devlanes.exe"
for target in x86_64-pc-windows-msvc aarch64-pc-windows-msvc; do
	pwsh -NoProfile -NonInteractive -File "$root/packaging/scripts/package.ps1" \
		-Version "$version" -Target "$target" -OutDir "$work/dist" -BinDir "$work/winbin" >/dev/null
	listed=$(unzip -Z1 "$work/dist/devlanes-$version-$target.zip" | tr '\n' ' ')
	[ "$listed" = "devlanes-$version-$target/LICENSE devlanes-$version-$target/devlanes.exe " ] || {
		echo "unexpected entries in the $target zip: $listed" >&2
		exit 1
	}
done
sh "$root/packaging/scripts/manifest.sh" "$version" "$work/dist" "https://example.invalid/download/v$version" >/dev/null
jq -e '.artifacts | length == 4' "$work/dist/release-manifest.json" >/dev/null
[ "$(grep -c '\.tar\.gz$' "$work/dist/SHA256SUMS")" -eq 4 ]
[ "$(grep -c '\.zip$' "$work/dist/SHA256SUMS")" -eq 2 ]

sh "$root/packaging/homebrew/render.sh" "$version" "$work/dist/SHA256SUMS" >"$work/devlanes.rb"
if command -v ruby >/dev/null 2>&1; then ruby -c "$work/devlanes.rb" >/dev/null; fi

sh "$root/packaging/npm/build.sh" "$version" "$work/dist" "$work/npm" >/dev/null
for cpu in x64 arm64; do
	cmp -s "$work/npm/devlanes-win32-$cpu/bin/devlanes.exe" "$work/winbin/devlanes.exe"
	jq -e '.os == ["win32"] and .cpu == ["'"$cpu"'"]' "$work/npm/devlanes-win32-$cpu/package.json" >/dev/null
done
jq -e '.optionalDependencies | all(. == "'"$version"'")' "$work/npm/devlanes/package.json" >/dev/null
mkdir "$work/tgz"
for dir in "$work"/npm/*/; do
	jq -e .name "$dir/package.json" >/dev/null
	(cd "$dir" && npm pack --pack-destination "$work/tgz" >/dev/null 2>&1) || {
		echo "npm pack failed in $dir" >&2
		exit 1
	}
done

# Installs the launcher and this host's platform package from the local tarballs.
# The other platforms' optional dependencies are not on any registry; npm skips them.
# npm 12 blocks install scripts by default and matches --allow-scripts against the
# tarball path rather than the name, so this local-only install allows them all.
npm_install() {
	prefix=$1
	shift
	npm install -g --offline --no-audit --no-fund --no-update-notifier --loglevel=error --dangerously-allow-all-scripts \
		--cache "$work/npm-cache" --prefix "$prefix" "$@" \
		"$work/tgz/devlanes-$version.tgz" "$work/tgz/devlanes-$host-$version.tgz" >/dev/null
}
host=$(node -p 'process.platform + "-" + process.arch')
# npm's global layout and postinstall differ on Windows; check.ps1 covers the launcher there.
if [ -f "$work/tgz/devlanes-$host-$version.tgz" ] && [ "${host%%-*}" != win32 ]; then
	# postinstall swaps the launcher for the binary, with devlanes-hosts next to it.
	npm_install "$work/prefix"
	installed=$work/prefix/lib/node_modules/devlanes/bin
	[ "$("$work/prefix/bin/devlanes" --version)" = "devlanes $version" ]
	cmp -s "$installed/devlanes" "$work/bin/devlanes"
	cmp -s "$installed/devlanes-hosts" "$work/bin/devlanes-hosts"
	# A second install over the first replaces the files again.
	npm_install "$work/prefix"
	cmp -s "$installed/devlanes" "$work/bin/devlanes"
	# Without scripts the launcher stays and runs the binary.
	npm_install "$work/prefix-noscripts" --ignore-scripts
	[ "$(head -n 1 "$work/prefix-noscripts/lib/node_modules/devlanes/bin/devlanes")" = "#!/usr/bin/env node" ]
	[ "$("$work/prefix-noscripts/bin/devlanes" --version)" = "devlanes $version" ]
fi

pypi=$root/packaging/pypi/build.py
pep440() { python3 "$pypi" --pep440 "$1"; }
[ "$(pep440 1.2.3)" = 1.2.3 ]
[ "$(pep440 1.2.3-alpha)" = 1.2.3a0 ]
[ "$(pep440 1.2.3-beta.2)" = 1.2.3b2 ]
[ "$(pep440 1.2.3-rc.10)" = 1.2.3rc10 ]
for wrong in 1.2.3-next.4 1.2.3-check 1.2.3-rc1 1.2.3-rc.0 1.2; do
	if pep440 "$wrong" >/dev/null 2>&1; then
		echo "build.py --pep440 accepted $wrong" >&2
		exit 1
	fi
done
wheel_version=$(pep440 "$version")
python3 "$pypi" "$version" "$work/dist" "$work/wheels" >/dev/null
python3 "$pypi" "$version" "$work/dist" "$work/wheels-again" >/dev/null
for wheel in "$work"/wheels/*.whl; do
	cmp -s "$wheel" "$work/wheels-again/${wheel##*/}" || {
		echo "${wheel##*/} differs between two builds" >&2
		exit 1
	}
done
set -- "$work"/wheels/*.whl
[ $# -eq 6 ]
data=devlanes-$wheel_version.data/scripts
info=devlanes-$wheel_version.dist-info
listed=$(unzip -Z1 "$work/wheels/devlanes-$wheel_version-py3-none-manylinux_2_35_x86_64.whl" | tr '\n' ' ')
[ "$listed" = "$data/devlanes $data/devlanes-hosts $info/METADATA $info/WHEEL $info/licenses/LICENSE $info/RECORD " ] || {
	echo "unexpected entries in the Linux wheel: $listed" >&2
	exit 1
}
listed=$(unzip -Z1 "$work/wheels/devlanes-$wheel_version-py3-none-win_arm64.whl" | tr '\n' ' ')
[ "$listed" = "$data/devlanes.exe $info/METADATA $info/WHEEL $info/licenses/LICENSE $info/RECORD " ] || {
	echo "unexpected entries in the Windows wheel: $listed" >&2
	exit 1
}
# Every file is in RECORD with its hash and size, as installers expect.
python3 - "$work"/wheels/*.whl <<'EOF'
import base64, csv, hashlib, io, sys, zipfile
for path in sys.argv[1:]:
    with zipfile.ZipFile(path) as wheel:
        names = wheel.namelist()
        record = next(name for name in names if name.endswith(".dist-info/RECORD"))
        rows = list(csv.reader(io.StringIO(wheel.read(record).decode())))
        assert sorted(row[0] for row in rows) == sorted(names), path
        for name, digest, size in rows:
            if name == record:
                continue
            body = wheel.read(name)
            expected = base64.urlsafe_b64encode(hashlib.sha256(body).digest()).rstrip(b"=").decode()
            assert digest == "sha256=" + expected and int(size) == len(body), f"{path}: {name}"
EOF
# pip picks this machine's wheel and puts both binaries, executable, in the venv's bin.
python3 -m venv "$work/venv"
"$work/venv/bin/python" -m pip install --quiet --disable-pip-version-check --no-index \
	--find-links "$work/wheels" "devlanes==$wheel_version"
cmp -s "$work/venv/bin/devlanes" "$work/bin/devlanes"
[ "$("$work/venv/bin/devlanes" --version)" = "devlanes $version" ]
[ "$("$work/venv/bin/devlanes-hosts")" = "devlanes-hosts $version" ]

release=$work/release
mkdir -p "$release/download/v$version" "$release/latest/download"
cp "$work"/dist/*.tar.gz "$work/dist/SHA256SUMS" "$release/download/v$version/"
cp "$work/dist/SHA256SUMS" "$release/latest/download/"
DEVLANES_INSTALL_BASE_URL="file://$release" DEVLANES_INSTALL_DIR="$work/installed" \
	sh "$root/install.sh" >/dev/null
[ "$("$work/installed/devlanes")" = "devlanes $version" ]
[ -x "$work/installed/devlanes-hosts" ]

# The same through options, as `curl | sh -s -- ...` passes them. The pinned version's
# folder is the only one present, so a run that reads latest/ fails.
install_with() {
	DEVLANES_INSTALL_BASE_URL="file://$release" DEVLANES_INSTALL_DIR="$work/installed-opt" sh -s -- "$@" <"$root/install.sh"
}
mv "$release/latest" "$work/latest"
install_with --version "$version" >/dev/null
install_with --version="v$version" >/dev/null
DEVLANES_VERSION=0.0.0 install_with -v "$version" >/dev/null
[ "$("$work/installed-opt/devlanes")" = "devlanes $version" ]
mv "$work/latest" "$release/latest"
install_with -d "$work/installed-short" >/dev/null
[ "$("$work/installed-short/devlanes")" = "devlanes $version" ]
[ "$(sh "$root/install.sh" --help)" = "$(sed -n "s/^usage='\(.*\)'$/\1/p" "$root/install.sh")" ]
for wrong in '--channel next' '--channel=next' '-c next' '--version next' '--version 1.2' '--release 1' '--version' \
	"--uninstall --version $version" '--uninstall --force'; do
	# shellcheck disable=SC2086 # each case is several words
	if install_with $wrong >/dev/null 2>&1; then
		echo "install.sh accepted $wrong" >&2
		exit 1
	fi
done
# `next` comes from `devlanes update --next` now.
case $(DEVLANES_VERSION=next install_with 2>&1 || true) in
*'install a stable release, then run devlanes update --next'*) ;;
*)
	echo "install.sh accepted DEVLANES_VERSION=next" >&2
	exit 1
	;;
esac

# A devlanes in the target directory that has `update` updates itself; install.sh
# passes its exit status on and installs nothing.
log=$work/check.log
status=0
DEVLANES_CHECK_LOG=$log DEVLANES_CHECK_EXIT=7 install_with --version 9.9.9 >/dev/null 2>&1 || status=$?
[ "$status" -eq 7 ]
[ "$(cat "$log")" = "update --help
update --yes --version 9.9.9" ]
rm "$log"
DEVLANES_CHECK_LOG=$log install_with >/dev/null
[ "$(tail -n 1 "$log")" = "update --yes" ]
rm "$log"

# Not for a release older than that devlanes, which `devlanes update` refuses:
# install.sh warns about the state database, installs it itself, then restarts the
# daemon into it.
older=$work/installed-older
mkdir "$older"
sed "s/devlanes $version/devlanes 9.9.9/" "$work/bin/devlanes" >"$older/devlanes"
chmod 755 "$older/devlanes"
warned=$(DEVLANES_CHECK_LOG=$log install_with -d "$older" --version "$version" 2>&1 >/dev/null)
[ "$("$older/devlanes")" = "devlanes $version" ]
[ "$(cat "$log")" = "daemon status
update --help
update --finish" ]
case $warned in
*"warning: devlanes $version is older than the installed 9.9.9."*"devlanes daemon reset-state empties it"*) ;;
*)
	echo "install.sh of an older release: $warned" >&2
	exit 1
	;;
esac
rm "$log"

# Nor into a directory this user cannot write to, where only install.sh may use
# sudo (root writes anywhere, so this needs another user).
if [ "$(id -u)" -ne 0 ]; then
	locked=$work/installed-locked
	mkdir "$locked"
	cp "$work/bin/devlanes" "$locked/devlanes"
	chmod 555 "$locked"
	status=0
	refused=$(DEVLANES_CHECK_LOG=$log install_with -d "$locked" 2>&1) || status=$?
	chmod 755 "$locked"
	[ "$status" -ne 0 ]
	case $refused in
	*"$locked is not writable"*) ;;
	*)
		echo "install.sh into an unwritable directory: $refused" >&2
		exit 1
		;;
	esac
	[ ! -e "$log" ]
fi

# A fresh install while a daemon answers restarts it into the new version.
DEVLANES_CHECK_LOG=$log install_with -d "$work/installed-finish" >/dev/null
[ "$(cat "$log")" = "daemon status
update --help
update --finish" ]
[ "$("$work/installed-finish/devlanes")" = "devlanes $version" ]
rm "$log"
# A release without `update` restarts it with `daemon restart`.
DEVLANES_CHECK_LOG=$log DEVLANES_CHECK_NO_UPDATE=1 install_with -d "$work/installed-restart" >/dev/null
[ "$(cat "$log")" = "daemon status
daemon restart --yes" ]
rm "$log"
# A failed restart fails the install and says how to restart.
status=0
refused=$(DEVLANES_CHECK_LOG=$log DEVLANES_CHECK_EXIT=7 install_with -d "$work/installed-failed" 2>&1) || status=$?
[ "$status" -ne 0 ]
case $refused in
*"the daemon did not restart into devlanes $version; run devlanes daemon restart"*) ;;
*)
	echo "install.sh with a failed restart: $refused" >&2
	exit 1
	;;
esac
rm "$log"

# The first devlanes on PATH is Homebrew's: install.sh names it and stops, unless --force.
mkdir -p "$work/brew/Cellar/devlanes/1.0/bin" "$work/brew/bin"
cp "$work/bin/devlanes" "$work/brew/Cellar/devlanes/1.0/bin/devlanes"
ln -s ../Cellar/devlanes/1.0/bin/devlanes "$work/brew/bin/devlanes"
refused=$(PATH="$work/brew/bin:$PATH" install_with -d "$work/installed-brew" 2>&1 || true)
# The message names the link's target, symlinks resolved ($TMPDIR is one on macOS).
cellar=$(cd "$work/brew/Cellar/devlanes/1.0/bin" && pwd -P)/devlanes
case $refused in
*"the devlanes on PATH, $cellar, comes from another installer; update it with: devlanes update"*) ;;
*)
	echo "install.sh with Homebrew's devlanes first on PATH: $refused" >&2
	exit 1
	;;
esac
[ ! -e "$work/installed-brew" ]
PATH="$work/brew/bin:$PATH" install_with -d "$work/installed-brew" --force >/dev/null
[ "$("$work/installed-brew/devlanes")" = "devlanes $version" ]
# pnpm puts a shell script on PATH that runs its copy, not a link to it.
mkdir "$work/pnpm-home"
# shellcheck disable=SC2016 # the shim expands them
printf '#!/bin/sh\nbasedir=$(dirname "$0")\nexec "$basedir/global/5/node_modules/devlanes/bin/devlanes" "$@"\n' >"$work/pnpm-home/devlanes"
chmod 755 "$work/pnpm-home/devlanes"
refused=$(PATH="$work/pnpm-home:$PATH" install_with -d "$work/installed-pnpm" 2>&1 || true)
case $refused in
*"comes from another installer; update it with: devlanes update"*) ;;
*)
	echo "install.sh with pnpm's devlanes first on PATH: $refused" >&2
	exit 1
	;;
esac
[ ! -e "$work/installed-pnpm" ]

# --uninstall runs the devlanes in the target directory (the fake one prints its
# version); without one it deletes what install.sh put there and names the data.
[ "$(install_with --uninstall)" = "devlanes $version" ]
mkdir "$work/leftover"
: >"$work/leftover/devlanes-hosts"
left=$(DEVLANES_INSTALL_DIR="$work/leftover" DEVLANES_HOME="$work/data" sh "$root/install.sh" --uninstall)
[ ! -e "$work/leftover/devlanes-hosts" ]
case $left in
*"removed: $work/leftover/devlanes-hosts"*"stays in $work/data;"*) ;;
*)
	echo "install.sh --uninstall without devlanes: $left" >&2
	exit 1
	;;
esac

echo "packaging scripts: dry run passed"
