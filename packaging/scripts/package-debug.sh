#!/bin/sh
# Packs the debug files of one target's release binaries (.dSYM bundles, .debug files
# or .pdb files) into OUT_DIR/devlanes-VERSION-TARGET-debug.tar.gz, one directory of
# the same name. The `-debug` goes after the target so that install.sh, install.ps1
# and `devlanes update`, which take the SHA256SUMS line ending in `-TARGET.tar.gz` (or
# `.zip`), never pick it as the archive to install.
set -eu

if [ $# -lt 4 ]; then
	echo "usage: $0 VERSION TARGET OUT_DIR PATH..." >&2
	exit 2
fi
version=$1
target=$2
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$3"
out=$(cd "$3" && pwd)
shift 3
name=devlanes-$version-$target-debug

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
for path in "$@"; do
	if [ ! -e "$path" ]; then
		echo "missing $path" >&2
		exit 1
	fi
	# -L: Cargo may uplift a .dSYM bundle as a symlink into deps/.
	cp -RL "$path" "$stage/$name/"
done

# Fixed owners, order and mtimes, as in package.sh.
epoch=${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct 2>/dev/null || echo 0)}
if tar --version 2>/dev/null | grep -q 'GNU tar'; then
	set -- --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$epoch"
else
	stamp=$(date -u -r "$epoch" +%Y%m%d%H%M.%S 2>/dev/null || date -u -d "@$epoch" +%Y%m%d%H%M.%S)
	find "$stage/$name" -exec touch -h -t "$stamp" {} +
	set -- --uid 0 --gid 0 --uname root --gname root
fi
COPYFILE_DISABLE=1 tar --no-xattrs "$@" -C "$stage" -cf "$stage/$name.tar" "$name"
gzip -n -9 -c "$stage/$name.tar" >"$out/$name.tar.gz"
echo "$out/$name.tar.gz"
