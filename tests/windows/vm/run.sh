#!/usr/bin/env bash
# runs the windows checks in a local windows 11 vm under native sudo, like the windows ci jobs plus
# the locked-file and fat32 scenarios that hosted runners skip. builds with the x86_64-pc-windows-gnu
# target (needs mingw-w64), hands the binaries to the vm's runner through the shared folder and
# waits for the result. exit code 0 when every check passed.
#
# the vm is a dockurr/windows container whose shared folder is $MUDO_WIN_SHARED; set it up once as
# described in docs/development.md (windows vm).
set -euo pipefail

container=${MUDO_WIN_CONTAINER:-mudo-windows}
shared=${MUDO_WIN_SHARED:-$HOME/win/shared}
target_dir=${MUDO_WIN_TARGET_DIR:-/var/tmp/mudo-windows-target}
timeout=${MUDO_WIN_TIMEOUT:-1800}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
target=x86_64-pc-windows-gnu

if ! state=$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null); then
    echo "run.sh: no container $container; see docs/development.md (windows vm)" >&2
    exit 1
fi
if [ "$state" != true ]; then
    docker start "$container" >/dev/null
    echo "started $container; windows boots and logs on before the runner picks the job up"
fi

cd "$repo"
export CARGO_TARGET_DIR=$target_dir
cargo build --release --locked --target "$target"
artifacts=$(cargo test --no-run --locked --target "$target" --bin mudo --test invocation \
    --message-format=json | jq -r 'select(.reason == "compiler-artifact" and .profile.test
        and .executable != null) | "\(.target.name) \(.executable)"')
unit=$(awk '$1 == "mudo" { print $2 }' <<<"$artifacts")
invocation=$(awk '$1 == "invocation" { print $2 }' <<<"$artifacts")
debug="$target_dir/$target/debug/mudo.exe"
for file in "$unit" "$invocation" "$debug"; do
    [ -f "$file" ] || { echo "run.sh: missing build output: $file" >&2; exit 1; }
done

# the runner script is read from the shared folder; a changed one applies after the next logon.
mkdir -p "$shared/runner" "$shared/jobs" "$shared/results"
cp "$here/runner.ps1" "$here/install-runner.ps1" "$shared/runner/"

id="$(date -u +%Y%m%dT%H%M%SZ)-$(git rev-parse --short HEAD)"
job="$shared/jobs/$id"
mkdir "$job"
cp "$target_dir/$target/release/mudo.exe" "$job/mudo.exe"
cp "$debug" "$job/mudo-debug.exe"
cp "$unit" "$job/unit.exe"
cp "$invocation" "$job/invocation.exe"
cp "$here/job.ps1" tests/windows/locks.ps1 tests/windows/fat32.ps1 "$job/"
printf '%s\n' "$debug" >"$job/debug-path.txt"
touch "$job/ready"
echo "job $id queued"

start=$SECONDS
shown=0
while [ ! -f "$job/exit" ]; do
    if [ ! -f "$job/started" ] && [ $((SECONDS - start)) -gt 600 ]; then
        echo "run.sh: the runner did not pick up the job in 10 minutes; is it installed? see docs/development.md (windows vm)" >&2
        exit 1
    fi
    if [ $((SECONDS - start)) -gt "$timeout" ]; then
        echo "run.sh: no result after $timeout seconds; the job stays in $job" >&2
        exit 1
    fi
    if [ -f "$job/output.txt" ]; then
        lines=$(wc -l <"$job/output.txt")
        if [ "$lines" -gt "$shown" ]; then
            sed -n "$((shown + 1)),${lines}p" "$job/output.txt"
            shown=$lines
        fi
    fi
    sleep 3
done

sed -n "$((shown + 1)),\$p" "$job/output.txt"
code=$(tr -d '\r\n\357\273\277' <"$job/exit")
cp "$job/output.txt" "$shared/results/$id.txt"
rm -rf "$job"
echo "result: $shared/results/$id.txt, exit $code"
exit "$code"
