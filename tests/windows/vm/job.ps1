# the windows checks, run elevated in the vm by runner.ps1: what the windows ci jobs run, plus the
# parts hosted runners skip because they have no native sudo. `run.sh` puts the binaries next to it.

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$shell = (Get-Process -Id $PID).Path
$failed = [System.Collections.Generic.List[string]]::new()

function Invoke-Step([string]$Name, [scriptblock]$Body) {
    Write-Output "===== $Name"
    $global:LASTEXITCODE = 0
    try {
        & $Body
        if ($LASTEXITCODE -ne 0) { throw "exit code $LASTEXITCODE" }
        Write-Output "===== $($Name): ok"
    } catch {
        Write-Output "===== $($Name): failed ($($_.Exception.Message))"
        $failed.Add($Name)
    }
}

$release = Join-Path $here 'mudo.exe'
# the integration tests run the debug binary at the path the build compiled in; a unix-style
# absolute path resolves on the system drive.
$compiled = (Get-Content -LiteralPath (Join-Path $here 'debug-path.txt') -Raw).Trim()
$debug = $env:SystemDrive + $compiled.Replace('/', '\')
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $debug) | Out-Null
Copy-Item -LiteralPath (Join-Path $here 'mudo-debug.exe') -Destination $debug -Force

Write-Output "$([Environment]::OSVersion.VersionString)"
Invoke-Step 'unit tests' { & (Join-Path $here 'unit.exe') }
Invoke-Step 'invocation tests' { & (Join-Path $here 'invocation.exe') }
Invoke-Step 'native sudo' { & "$env:SystemRoot\System32\sudo.exe" $release apt opt gop stop remove --version }
Invoke-Step 'locked file scenarios' {
    & $shell -NoProfile -File (Join-Path $here 'locks.ps1') -Binary $release
}
Invoke-Step 'fat32 volumes' {
    & $shell -NoProfile -File (Join-Path $here 'fat32.ps1') -Binary $release -ScratchRoot $env:TEMP
}

Remove-Item -LiteralPath $debug -Force -ErrorAction SilentlyContinue
Write-Output ''
if ($failed.Count -gt 0) {
    Write-Output "failed: $($failed -join ', ')"
    exit 1
}
Write-Output 'all windows checks passed'
