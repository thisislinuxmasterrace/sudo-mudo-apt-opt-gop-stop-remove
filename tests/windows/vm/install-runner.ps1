# installs the job runner in the windows vm, once, from an administrator powershell:
#   powershell -ExecutionPolicy Bypass -File \\host.lan\Data\runner\install-runner.ps1
# it installs pwsh 7 if missing, turns on native sudo in inline mode and registers a logon task that
# runs runner.ps1 from the shared folder with the highest privileges.

$ErrorActionPreference = 'Stop'

$pwshDir = 'C:\pwsh'
$pwsh = Join-Path $pwshDir 'pwsh.exe'
if (-not (Test-Path -LiteralPath $pwsh)) {
    $zip = Join-Path $env:TEMP 'pwsh.zip'
    Invoke-WebRequest -UseBasicParsing -OutFile $zip `
        -Uri 'https://github.com/PowerShell/PowerShell/releases/download/v7.4.6/PowerShell-7.4.6-win-x64.zip'
    Expand-Archive -LiteralPath $zip -DestinationPath $pwshDir -Force
    Remove-Item -LiteralPath $zip
}

# inline mode keeps the elevated command in the same console, so its output can be redirected.
& "$env:SystemRoot\System32\sudo.exe" config --enable normal
if ($LASTEXITCODE -ne 0) { throw "sudo config failed: $LASTEXITCODE" }

$runner = Join-Path $PSScriptRoot 'runner.ps1'
$action = New-ScheduledTaskAction -Execute $pwsh -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$runner`""
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName 'mudo-runner' -Action $action -Trigger $trigger -Principal $principal `
    -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'mudo-runner'
Write-Output 'mudo-runner installed and started'
