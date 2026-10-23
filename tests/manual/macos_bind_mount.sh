#!/bin/bash
# manual check for macos: a walk with -r must not enter a bind mount or touch its source.
#
# what it checks:
#   A. --dry-run -r: exit 0; the summary skips the mount point ("skipped mount points or other
#      filesystems: 1") and counts only own.txt (files: 1, directories: 1); the contents of source
#      are not counted; nothing changes.
#   B. -f -r: exit 4 (tree is kept because a mount point inside it was skipped); own.txt is removed;
#      bind is still a directory and a mount point; keep-a.txt and keep-b.txt are there, sha256 unchanged.
#
# requirements:
#   - run by hand, on macos only. the script mounts a directory and runs erase inside its own temporary
#     directory. on linux it exits at once: mount_nullfs exists only on macos.
#   - mount_nullfs ships with macos, macfuse is not needed.
#   - sudo is needed: mount_nullfs and umount run as root. sudo asks once at the start (sudo -v).
#   - on EXIT (and on INT/TERM) the script unmounts the bind point with sudo umount and removes its own
#     temporary directory (mktemp, under $TMPDIR or /var/folders). if the unmount fails, the directory is
#     kept: rm -rf would otherwise go through a live mount point.
#   - compatible with bash 3.2 (the default bash on macos).
#
# usage: tests/manual/macos_bind_mount.sh [path/to/binary]
#   default: target/release/sudo-mudo-apt-opt-gop-stop-remove from the repository root.
#   build first: cargo build --release

set -euo pipefail

NAME=sudo-mudo-apt-opt-gop-stop-remove
REPO=$(cd "$(dirname "$0")/../.." && pwd -P)
BIN=${1:-$REPO/target/release/$NAME}

work=""
bind=""
tmp_canon=""
fails=""

die() {
    echo "error: $*" >&2
    exit 1
}

fail() {
    fails="${fails:+$fails; }$1"
}

# true when `mount` lists $1 as a mount point.
is_mounted() {
    [ -n "$1" ] && mount | grep -F " on $1 (" >/dev/null 2>&1
}

# true only for a directory that mktemp created in this script, so rm -rf never gets anything else.
safe_work() {
    case "$work" in
        "") return 1 ;;
    esac
    case "$work" in
        "$tmp_canon"/macos-bind-mount.* | /private/var/folders/*/macos-bind-mount.* | /var/folders/*/macos-bind-mount.*)
            return 0
            ;;
    esac
    return 1
}

cleanup() {
    local status=$?
    if is_mounted "$bind"; then
        echo "unmounting $bind" >&2
        if ! sudo umount "$bind"; then
            echo "could not unmount $bind: unmount it by hand (sudo umount \"$bind\")" >&2
            echo "directory $work is kept while the mount point is alive" >&2
            exit "$status"
        fi
    fi
    if [ -n "$work" ]; then
        if safe_work; then
            rm -rf "$work"
        else
            echo "not removing $work: the path does not look like this script's temporary directory" >&2
        fi
    fi
    exit "$status"
}

trap cleanup EXIT
trap 'exit 130' INT TERM

[ "$(uname -s)" = Darwin ] || die "this script is for macos only (mount_nullfs exists only there)"
[ -x /sbin/mount_nullfs ] || die "/sbin/mount_nullfs not found"
[ -x "$BIN" ] || die "executable not found: $BIN (build first: cargo build --release)"

echo "sudo is needed: mount_nullfs and umount run as root" >&2
sudo -v || die "sudo refused: the mount point cannot be created without it"

tmp_base=${TMPDIR:-/tmp}
tmp_base=${tmp_base%/}
tmp_canon=$(cd "$tmp_base" && pwd -P) || die "cannot open $tmp_base"
work=$(mktemp -d "$tmp_canon/macos-bind-mount.XXXXXX")
work=$(cd "$work" && pwd -P)
safe_work || die "temporary directory is outside the expected location: $work"
echo "working directory: $work"

mkdir -p "$work/source" "$work/tree/bind"
printf 'keep-a\n' > "$work/source/keep-a.txt"
printf 'keep-b\n' > "$work/source/keep-b.txt"
printf 'own\n' > "$work/tree/own.txt"
bind="$work/tree/bind"

hash_a=$(shasum -a 256 "$work/source/keep-a.txt" | cut -d' ' -f1)
hash_b=$(shasum -a 256 "$work/source/keep-b.txt" | cut -d' ' -f1)

check_sources() {
    local label=$1
    [ "$(shasum -a 256 "$work/source/keep-a.txt" 2>/dev/null | cut -d' ' -f1)" = "$hash_a" ] \
        || fail "$label: keep-a.txt changed or is missing"
    [ "$(shasum -a 256 "$work/source/keep-b.txt" 2>/dev/null | cut -d' ' -f1)" = "$hash_b" ] \
        || fail "$label: keep-b.txt changed or is missing"
}

echo "mounting $work/source on $bind (mount_nullfs)"
if ! sudo mount_nullfs "$work/source" "$bind"; then
    die "mount_nullfs failed"
fi
is_mounted "$bind" || die "after mount_nullfs $bind is not listed in the mount table"

echo
echo "== A: --dry-run -r $work/tree"
code=0
out_a=$("$BIN" --dry-run -r "$work/tree" 2>&1) || code=$?
echo "$out_a"
echo "exit code: $code (expected 0)"
[ "$code" -eq 0 ] || fail "dry run: exit $code, expected 0"
# anchored at the end of the line so that ": 1" does not also match ": 10"
grep -q 'skipped mount points or other filesystems: 1$' <<< "$out_a" \
    || fail "dry run: no line \"skipped mount points or other filesystems: 1\""
case "$out_a" in
    *"files: 1, directories: 1, links: 0, "*) ;;
    *) fail "dry run: summary is not \"files: 1, directories: 1\" (the bind mount or the source contents were counted)" ;;
esac
[ -f "$work/tree/own.txt" ] || fail "dry run: own.txt is gone, but a dry run must not change anything"
check_sources "dry run"

echo
echo "== B: -f -r $work/tree (erase)"
code=0
out_b=$("$BIN" -f -r "$work/tree" 2>&1) || code=$?
echo "$out_b"
echo "exit code: $code (expected 4: tree is kept, the mount point inside it was skipped)"
[ "$code" -eq 4 ] || fail "erase: exit $code, expected 4"
check_sources "erase"
[ ! -e "$work/tree/own.txt" ] || fail "erase: own.txt was not removed"
[ -d "$work/tree" ] || fail "erase: tree is gone, but a mount point inside it was skipped"
[ -d "$bind" ] || fail "erase: bind is gone"
is_mounted "$bind" || fail "erase: bind is no longer a mount point"

echo
if [ -n "$fails" ]; then
    echo "FAIL: $fails"
    exit 1
fi
echo "OK"
