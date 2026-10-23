#!/bin/bash
# ручной сценарий для macOS: обход с -r не должен заходить в bind mount и трогать его источник.
#
# что проверяет:
#   A. --dry-run -r: код 0; сводка пропускает точку монтирования ("skipped mount points or other
#      filesystems: 1") и считает только own.txt (files: 1, directories: 1); содержимое source в сводку
#      не попадает; ничего не изменилось.
#   B. -f -r: код 4 (tree не стёрт, потому что внутри него пропущена точка монтирования); own.txt удалён;
#      bind остался каталогом и точкой монтирования; keep-a.txt и keep-b.txt есть, sha256 не изменился.
#
# требования:
#   - запускает только владелец, только на macOS. сценарий монтирует каталог и вызывает erase в своём
#     временном каталоге. на linux он не работает: mount_nullfs есть только в macOS, сценарий сразу выходит.
#   - mount_nullfs встроен в macOS, macFUSE не нужен.
#   - нужен sudo: mount_nullfs и umount выполняются от root. sudo спрашивается один раз (sudo -v) в начале.
#   - в EXIT (и при INT/TERM) сценарий размонтирует bind-точку через sudo umount и удаляет свой временный
#     каталог (mktemp, внутри $TMPDIR или /var/folders). если размонтировать не удалось, каталог не удаляется:
#     rm -rf пошёл бы через живую точку монтирования.
#   - совместим с bash 3.2 (стандартный bash macOS).
#
# запуск: tests/manual/macos_bind_mount.sh [путь/к/бинарнику]
#   по умолчанию: target/release/sudo-mudo-apt-opt-gop-stop-remove от корня репозитория.
#   сборка: cargo build --release

set -euo pipefail

NAME=sudo-mudo-apt-opt-gop-stop-remove
REPO=$(cd "$(dirname "$0")/../.." && pwd -P)
BIN=${1:-$REPO/target/release/$NAME}

work=""
bind=""
tmp_canon=""
fails=""

die() {
    echo "ошибка: $*" >&2
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
        echo "размонтирую $bind" >&2
        if ! sudo umount "$bind"; then
            echo "не удалось размонтировать $bind: сделайте это вручную (sudo umount \"$bind\")" >&2
            echo "каталог $work не удалён, пока точка монтирования жива" >&2
            exit "$status"
        fi
    fi
    if [ -n "$work" ]; then
        if safe_work; then
            rm -rf "$work"
        else
            echo "не удаляю $work: путь не похож на временный каталог этого сценария" >&2
        fi
    fi
    exit "$status"
}

trap cleanup EXIT
trap 'exit 130' INT TERM

[ "$(uname -s)" = Darwin ] || die "сценарий только для macOS (mount_nullfs есть только там)"
[ -x /sbin/mount_nullfs ] || die "нет /sbin/mount_nullfs"
[ -x "$BIN" ] || die "нет исполняемого бинарника: $BIN (сначала: cargo build --release)"

echo "нужен sudo: mount_nullfs и umount выполняются от root" >&2
sudo -v || die "sudo отказал: без него точка монтирования не создаётся"

tmp_base=${TMPDIR:-/tmp}
tmp_base=${tmp_base%/}
tmp_canon=$(cd "$tmp_base" && pwd -P) || die "не открывается $tmp_base"
work=$(mktemp -d "$tmp_canon/macos-bind-mount.XXXXXX")
work=$(cd "$work" && pwd -P)
safe_work || die "временный каталог вне ожидаемого места: $work"
echo "рабочий каталог: $work"

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
        || fail "$label: keep-a.txt изменился или пропал"
    [ "$(shasum -a 256 "$work/source/keep-b.txt" 2>/dev/null | cut -d' ' -f1)" = "$hash_b" ] \
        || fail "$label: keep-b.txt изменился или пропал"
}

echo "монтирую $work/source в $bind (mount_nullfs)"
if ! sudo mount_nullfs "$work/source" "$bind"; then
    die "mount_nullfs не сработал"
fi
is_mounted "$bind" || die "после mount_nullfs $bind не виден в таблице монтирования"

echo
echo "== A: --dry-run -r $work/tree"
code=0
out_a=$("$BIN" --dry-run -r "$work/tree" 2>&1) || code=$?
echo "$out_a"
echo "код выхода: $code (ожидался 0)"
[ "$code" -eq 0 ] || fail "dry run: код $code, ожидался 0"
grep -q 'skipped mount points or other filesystems: 1$' <<< "$out_a" \
    || fail "dry run: нет строки «skipped mount points or other filesystems: 1» (проверяется до конца строки, чтобы не совпасть с «: 10»)"
case "$out_a" in
    *"files: 1, directories: 1, links: 0, "*) ;;
    *) fail "dry run: сводка не «files: 1, directories: 1» (bind или содержимое source учтены)" ;;
esac
[ -f "$work/tree/own.txt" ] || fail "dry run: own.txt пропал, хотя dry run ничего не меняет"
check_sources "dry run"

echo
echo "== B: -f -r $work/tree (erase)"
code=0
out_b=$("$BIN" -f -r "$work/tree" 2>&1) || code=$?
echo "$out_b"
echo "код выхода: $code (ожидался 4: tree сохранён, пропущена точка монтирования)"
[ "$code" -eq 4 ] || fail "erase: код $code, ожидался 4"
check_sources "erase"
[ ! -e "$work/tree/own.txt" ] || fail "erase: own.txt не удалён"
[ -d "$work/tree" ] || fail "erase: tree пропал, хотя внутри него пропущена точка монтирования"
[ -d "$bind" ] || fail "erase: bind пропал"
is_mounted "$bind" || fail "erase: bind больше не точка монтирования"

echo
if [ -n "$fails" ]; then
    echo "FAIL: $fails"
    exit 1
fi
echo "OK"
