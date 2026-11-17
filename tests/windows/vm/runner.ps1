# job runner inside the windows vm. it watches the shared folder for jobs that `run.sh` writes on the
# host and runs them one at a time with the highest privileges, in the logged-on user's console, so
# native sudo works as it does in a terminal. installed by install-runner.ps1 as a logon task.

param([string]$Jobs = '\\host.lan\Data\jobs')

$ErrorActionPreference = 'Continue'
$log = Join-Path $env:ProgramData 'mudo-runner.log'
$shell = (Get-Process -Id $PID).Path

function Write-Log([string]$Text) {
    Add-Content -LiteralPath $log -Value "$(Get-Date -Format o) $Text"
}

Write-Log "runner started, watching $Jobs"
while ($true) {
    try {
        if (Test-Path -LiteralPath $Jobs) {
            foreach ($dir in Get-ChildItem -LiteralPath $Jobs -Directory | Sort-Object Name) {
                $ready = Join-Path $dir.FullName 'ready'
                $started = Join-Path $dir.FullName 'started'
                if (-not (Test-Path -LiteralPath $ready) -or (Test-Path -LiteralPath $started)) { continue }
                New-Item -ItemType File -Path $started | Out-Null
                Write-Log "job $($dir.Name) started"
                # binaries run from a local copy: running them from the share is slow and locks files there.
                $work = Join-Path $env:TEMP "mudo-job-$($dir.Name)"
                Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
                Copy-Item -LiteralPath $dir.FullName -Destination $work -Recurse
                $output = Join-Path $dir.FullName 'output.txt'
                Push-Location -LiteralPath $work
                try {
                    & $shell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $work 'job.ps1') *>&1 |
                        Out-File -LiteralPath $output -Encoding utf8
                    $code = $LASTEXITCODE
                } finally {
                    Pop-Location
                }
                Set-Content -LiteralPath (Join-Path $dir.FullName 'exit') -Value $code
                Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
                Write-Log "job $($dir.Name) finished with $code"
            }
        }
    } catch {
        Write-Log "error: $($_.Exception.Message)"
    }
    Start-Sleep -Seconds 2
}
