#!/bin/sh
# Packs the release binaries for one target into two archives in OUT_DIR:
# devlanes-VERSION-TARGET.tar.gz with devlanes, devlanes-hosts and LICENSE, and
# devlanes-VERSION-TARGET-gui.tar.gz with devlanes-gui and LICENSE. Both hold one
# directory named devlanes-VERSION-TARGET; release-manifest.json refers to devlanes
# by that member path. devlanes-gui has an archive of its own because every released
# installer and `devlanes update` refuses a main archive with any other entry.
set -eu

if [ $# -lt 3 ]; then
	echo "usage: $0 VERSION TARGET OUT_DIR [BIN_DIR]" >&2
	exit 2
fi
version=$1
target=$2
bin_dir=${4:-target/$target/release}
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$3"
out=$(cd "$3" && pwd)
name=devlanes-$version-$target

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
epoch=${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct 2>/dev/null || echo 0)}

# pack ARCHIVE BIN...: writes OUT_DIR/ARCHIVE.tar.gz holding $name/ with the BINs
# and LICENSE.
pack() {
	archive=$1
	shift
	rm -rf "${stage:?}/$name"
	mkdir "$stage/$name"
	for bin in "$@"; do
		if [ ! -x "$bin_dir/$bin" ]; then
			echo "missing executable $bin_dir/$bin" >&2
			exit 1
		fi
		cp "$bin_dir/$bin" "$stage/$name/$bin"
		chmod 755 "$stage/$name/$bin"
	done
	cp "$root/LICENSE" "$stage/$name/LICENSE"
	chmod 644 "$stage/$name/LICENSE"

	# Same inputs give the same archive: fixed owners, order and mtimes
	# (SOURCE_DATE_EPOCH, else the last commit's time) and a gzip header without a timestamp.
	if tar --version 2>/dev/null | grep -q 'GNU tar'; then
		set -- --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$epoch"
	else
		# bsdtar (macOS) has no --sort or --mtime; one directory with fixed names keeps
		# the order stable, and touch fixes the times.
		stamp=$(date -u -r "$epoch" +%Y%m%d%H%M.%S 2>/dev/null || date -u -d "@$epoch" +%Y%m%d%H%M.%S)
		find "$stage/$name" -exec touch -h -t "$stamp" {} +
		set -- --uid 0 --gid 0 --uname root --gname root
	fi
	# macOS tar would otherwise store extended attributes (com.apple.provenance) as
	# pax headers that GNU tar warns about, and AppleDouble ._ files.
	COPYFILE_DISABLE=1 tar --no-xattrs "$@" -C "$stage" -cf "$stage/$archive.tar" "$name"
	gzip -n -9 -c "$stage/$archive.tar" >"$out/$archive.tar.gz"
	echo "$out/$archive.tar.gz"
}

pack "$name" devlanes devlanes-hosts
pack "$name-gui" devlanes-gui
