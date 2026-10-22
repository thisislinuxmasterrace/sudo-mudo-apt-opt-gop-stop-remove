# locked-file scenarios for the windows erase path.
# needs pwsh 7+ (ProcessStartInfo.ArgumentList).
# scenarios touch only files in a fresh temp directory, so they are safe to run locally.

param(
    [Parameter(Mandatory)]
    [string]$Binary
)

$ErrorActionPreference = 'Stop'

$Binary = (Resolve-Path -LiteralPath $Binary).ProviderPath

$tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
$scratch = Join-Path $tempRoot ('locks-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null

$holders = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()
$results = [System.Collections.Generic.List[object]]::new()

function Add-Result([string]$Name, [string]$Status, [string]$Message) {
    $results.Add([pscustomobject]@{ Name = $Name; Status = $Status; Message = $Message })
    Write-Output "[$Status] $($Name): $Message"
}

function Show-Result([string]$Name, $Result) {
    Write-Output "----- $Name (exit $($Result.ExitCode))"
    Write-Output 'stdout:'
    Write-Output $Result.Stdout
    Write-Output 'stderr:'
    Write-Output $Result.Stderr
}

function Invoke-Binary([string[]]$Arguments) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($Binary)
    foreach ($arg in $Arguments) { $psi.ArgumentList.Add($arg) }
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true

    $proc = [System.Diagnostics.Process]::Start($psi)
    # closed stdin: a prompt gets end of input instead of waiting for a key
    $proc.StandardInput.Close()
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    if (-not $proc.WaitForExit(120000)) {
        $proc.Kill()
        throw "binary did not exit within 120 s: $Arguments"
    }
    [pscustomobject]@{
        ExitCode = $proc.ExitCode
        Stdout   = $stdoutTask.Result
        Stderr   = $stderrTask.Result
    }
}

# random bytes so an overwrite is detectable.
function New-RandomFile([string]$FileName) {
    $path = Join-Path $scratch $FileName
    $bytes = [System.Security.Cryptography.RandomNumberGenerator]::GetBytes(65536)
    [System.IO.File]::WriteAllBytes($path, $bytes)
    return $path
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
}

# keeps a handle open in a child pwsh. returns after the child writes its ready
# marker (so the handle is really open), or throws if the child dies first.
function Start-Holder([string]$Path, [System.IO.FileShare]$Share, [string]$Name) {
    $ready = Join-Path $scratch "$Name.ready"
    $errLog = Join-Path $scratch "$Name.holder.err"
    $outLog = Join-Path $scratch "$Name.holder.out"

    $template = @'
$ErrorActionPreference = 'Stop'
$fs = [System.IO.FileStream]::new('__PATH__', [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::__SHARE__)
New-Item -ItemType File -Path '__READY__' | Out-Null
Start-Sleep -Seconds 600
'@
    $script = $template.Replace('__PATH__', $Path.Replace("'", "''")).Replace('__SHARE__', $Share.ToString()).Replace('__READY__', $ready.Replace("'", "''"))
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))

    $shell = (Get-Process -Id $PID).Path
    $holder = Start-Process -FilePath $shell -ArgumentList @('-NoProfile', '-EncodedCommand', $encoded) -PassThru -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    $holders.Add($holder)

    $deadline = (Get-Date).AddSeconds(30)
    while (-not (Test-Path -LiteralPath $ready)) {
        $holder.Refresh()
        if ($holder.HasExited) {
            throw "holder exited early (code $($holder.ExitCode)): $(Get-Content -LiteralPath $errLog -Raw -ErrorAction SilentlyContinue)"
        }
        if ((Get-Date) -gt $deadline) { throw 'holder did not open the file within 30 s' }
        Start-Sleep -Milliseconds 200
    }
    return $holder
}

# $Name is read from the caller's scope inside the scenario bodies.
function Invoke-Scenario([string]$Name, [scriptblock]$Body) {
    try {
        & $Body
    }
    catch {
        Add-Result $Name 'FAIL' "exception: $($_.Exception.Message)"
    }
}

try {
    Invoke-Scenario '1. no holder, plain erase' {
        $file = New-RandomFile 'no-holder.bin'
        $result = Invoke-Binary @('-f', $file)
        Show-Result $Name $result
        $gone = -not (Test-Path -LiteralPath $file)
        if ($result.ExitCode -eq 0 -and $gone) {
            Add-Result $Name 'PASS' 'exit 0, file removed'
        } else {
            Add-Result $Name 'FAIL' "exit $($result.ExitCode), file removed: $gone"
        }
    }

    Invoke-Scenario '2. FileShare.Read holder, plain erase' {
        $file = New-RandomFile 'share-read.bin'
        $hashBefore = Get-Sha256 $file
        $null = Start-Holder -Path $file -Share ([System.IO.FileShare]::Read) -Name 'share-read'
        $result = Invoke-Binary @('-f', $file)
        Show-Result $Name $result
        $exists = Test-Path -LiteralPath $file
        $unchanged = (Get-Sha256 $file) -eq $hashBefore
        $text = "$($result.Stdout)$($result.Stderr)"
        $lockReported = $text -match 'locked|being used'
        if ($result.ExitCode -ne 0 -and $exists -and $unchanged -and $lockReported) {
            Add-Result $Name 'PASS' "exit $($result.ExitCode), lock reported, file kept, bytes unchanged"
        } else {
            Add-Result $Name 'FAIL' "exit $($result.ExitCode), lock reported: $lockReported, file exists: $exists, bytes unchanged: $unchanged"
        }
    }

    Invoke-Scenario '3. FileShare.Read holder, --kill-holders' {
        $file = New-RandomFile 'kill-holders.bin'
        $holder = Start-Holder -Path $file -Share ([System.IO.FileShare]::Read) -Name 'kill-holders'
        $result = Invoke-Binary @('--kill-holders', '-f', $file)
        Show-Result $Name $result
        # restart manager may not terminate a plain pwsh process. if it does not, this fails on purpose.
        $holderExited = $holder.WaitForExit(10000)
        $gone = -not (Test-Path -LiteralPath $file)
        if ($result.ExitCode -eq 0 -and $gone -and $holderExited) {
            Add-Result $Name 'PASS' 'exit 0, file removed, holder exited'
        } else {
            Add-Result $Name 'FAIL' "exit $($result.ExitCode), file removed: $gone, holder exited: $holderExited"
        }
    }
}
finally {
    foreach ($holder in $holders) {
        try {
            if (-not $holder.HasExited) {
                $holder.Kill()
                [void]$holder.WaitForExit(5000)
            }
        }
        catch {
            # the holder may have exited between the check and the kill
        }
    }
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}

$passed = @($results | Where-Object Status -eq 'PASS').Count
$failed = @($results | Where-Object Status -eq 'FAIL').Count
$skipped = @($results | Where-Object Status -eq 'SKIPPED').Count
Write-Output ''
Write-Output "summary: $passed passed, $failed failed, $skipped skipped"
if ($failed -gt 0) { exit 1 }
