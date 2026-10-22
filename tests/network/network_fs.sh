#!/usr/bin/env bash
# checks the network filesystem behaviour on a real mount (linux, run as root by the ci job).
# usage: network_fs.sh <binary> <mount directory> <filesystem name as reported, e.g. nfs or cifs>
#   - a file on the mount gets the network warning
#   - the file is still erased (warnings never block an erase)
#   - --wipe-free on the mount is refused before anything is written

set -euo pipefail

bin=$1
mnt=$2
kind=$3
# statfs reports smb2 for smb 2.x and later, so cifs also matches smb2.
case "$kind" in cifs) warn_kind='(cifs|smb2)' ;; *) warn_kind=$kind ;; esac

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

probe="$mnt/probe-$RANDOM.bin"
head -c 65536 /dev/urandom > "$probe"

dry=$("$bin" --dry-run "$probe" 2>&1) || fail "dry run failed: $dry"
echo "$dry"
grep -Eq "$warn_kind is a network filesystem" <<<"$dry" || fail "no network warning for $kind"

erase=$("$bin" -f "$probe" 2>&1) || fail "erase failed: $erase"
echo "$erase"
grep -Eq "$warn_kind is a network filesystem" <<<"$erase" || fail "erase did not warn for $kind"
[ ! -e "$probe" ] || fail "file still present after erase"

set +e
wipe=$("$bin" --wipe-free "$mnt" -f 2>&1)
rc=$?
set -e
echo "$wipe"
[ "$rc" -ne 0 ] || fail "--wipe-free on $kind was not refused"
grep -q "refusing to fill a network filesystem" <<<"$wipe" || fail "--wipe-free refusal text missing"

echo "ok: $kind mount checked"
