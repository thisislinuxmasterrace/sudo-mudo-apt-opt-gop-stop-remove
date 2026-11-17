# disposable fat32 volume checks; run only in the isolated windows vm as administrator.
param(
    [Parameter(Mandatory)] [string]$Binary,
    [Parameter(Mandatory)] [string]$ScratchRoot
)

$ErrorActionPreference = 'Stop'
$Binary = (Resolve-Path -LiteralPath $Binary).ProviderPath
$sudo = Join-Path $env:SystemRoot 'System32\sudo.exe'
if (-not (Test-Path -LiteralPath $sudo -PathType Leaf)) { throw 'native windows sudo is unavailable' }
$ScratchRoot = (Resolve-Path -LiteralPath $ScratchRoot).ProviderPath
$scratch = Join-Path $ScratchRoot ('sudo-mudo-fat32-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null

function Invoke-VolumeCheck([int]$SizeMiB) {
    $image = Join-Path $scratch ("volume-$SizeMiB.vhd")
    $commands = Join-Path $scratch ("diskpart-$SizeMiB.txt")
    $output = Join-Path $scratch ("stdout-$SizeMiB.txt")
    $errors = Join-Path $scratch ("stderr-$SizeMiB.txt")
    $diskNumber = $null
    $process = $null
    try {
        if (Test-Path -LiteralPath $image) { throw 'image already exists' }
        @("create vdisk file=`"$image`" maximum=$SizeMiB type=expandable", 'exit') |
            Set-Content -LiteralPath $commands -Encoding Ascii
        & diskpart.exe /s $commands
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $image -PathType Leaf)) {
            throw "diskpart create failed: $LASTEXITCODE"
        }
        Mount-DiskImage -ImagePath $image -ErrorAction Stop | Out-Null
        $imageInfo = Get-DiskImage -ImagePath $image
        $disks = @($imageInfo | Get-Disk)
        if (-not $imageInfo.Attached -or $disks.Count -ne 1) { throw 'image disk identity unavailable' }
        $disk = $disks[0]
        $diskNumber = $disk.Number
        if ($disk.PartitionStyle -ne 'RAW' -or $disk.Size -ne ($SizeMiB * 1MB) -or
            $disk.BusType -ne 'File Backed Virtual') { throw 'unexpected image disk identity' }
        $disk | Initialize-Disk -PartitionStyle MBR -PassThru | Out-Null
        $disk = @(Get-DiskImage -ImagePath $image | Get-Disk)
        if ($disk.Count -ne 1 -or $disk[0].Number -ne $diskNumber -or
            $disk[0].Size -ne ($SizeMiB * 1MB)) { throw 'image disk identity changed' }
        $partition = New-Partition -DiskNumber $diskNumber -UseMaximumSize -AssignDriveLetter
        if (-not $partition.DriveLetter) { throw 'no drive letter assigned' }
        $disk = @(Get-DiskImage -ImagePath $image | Get-Disk)
        if ($disk.Count -ne 1 -or $disk[0].Number -ne $diskNumber) { throw 'image disk identity changed' }
        $volume = $partition | Format-Volume -FileSystem FAT32 -NewFileSystemLabel 'SMTEST' -Confirm:$false
        if ($volume.FileSystem -ne 'FAT32' -or $volume.DriveLetter -ne $partition.DriveLetter) {
            throw 'formatted volume identity mismatch'
        }
        $root = "$($partition.DriveLetter):\"
        $sentinel = Join-Path $root 'sentinel.txt'
        [System.IO.File]::WriteAllText($sentinel, 'disposable volume sentinel')
        $target = Join-Path $root 'wipe-test'
        New-Item -ItemType Directory -Path $target | Out-Null
        $before = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
        $freeBefore = (Get-Volume -DriveLetter $partition.DriveLetter).SizeRemaining
        Write-Output "volume=$SizeMiB MiB filesystem=$($volume.FileSystem) free-before=$freeBefore"
        $disk = @(Get-DiskImage -ImagePath $image | Get-Disk)
        $currentPartition = Get-Partition -DriveLetter $partition.DriveLetter
        if ($disk.Count -ne 1 -or $disk[0].Number -ne $diskNumber -or
            $currentPartition.DiskNumber -ne $diskNumber -or
            (Get-Volume -DriveLetter $partition.DriveLetter).FileSystem -ne 'FAT32') {
            throw 'wipe target identity changed'
        }
        $arguments = @(('"' + $Binary + '"'), 'apt', 'opt', 'gop', 'stop', 'remove', '--wipe-free', ('"' + $target + '"'), '-f', '-v')
        $process = Start-Process -FilePath $sudo -ArgumentList $arguments -PassThru -NoNewWindow `
            -RedirectStandardOutput $output -RedirectStandardError $errors
        [void]$process.Handle
        $clock = [System.Diagnostics.Stopwatch]::StartNew()
        $maxFillFiles = 0
        $maxFileBytes = [long]0
        $minFree = $freeBefore
        while (-not $process.WaitForExit(250)) {
            $fills = @(Get-ChildItem -LiteralPath $target -Force | Where-Object { $_.Name -like '.fill-*' })
            $maxFillFiles = [Math]::Max($maxFillFiles, $fills.Count)
            foreach ($fill in $fills) { $maxFileBytes = [Math]::Max([long]$maxFileBytes, [long]$fill.Length) }
            $minFree = [Math]::Min($minFree, (Get-Volume -DriveLetter $partition.DriveLetter).SizeRemaining)
            if ($clock.Elapsed.TotalSeconds -ge 900) {
                $process.Kill()
                $process.WaitForExit()
                throw 'wipe exceeded 900 second deadline and was terminated'
            }
        }
        $exitCode = $process.ExitCode
        Write-Output "wipe-exit=$exitCode min-free=$minFree max-visible-fill-files=$maxFillFiles max-visible-file-bytes=$maxFileBytes"
        Get-Content -LiteralPath $output
        Get-Content -LiteralPath $errors
        $after = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
        $freeAfter = (Get-Volume -DriveLetter $partition.DriveLetter).SizeRemaining
        $remaining = @(Get-ChildItem -LiteralPath $target -Force | Where-Object { $_.Name -like '.fill-*' })
        Write-Output "free-after=$freeAfter sentinel-preserved=$($before -eq $after) fill-entries=$($remaining.Count)"
        if ($before -ne $after -or $remaining.Count -ne 0) { throw 'sentinel or cleanup check failed' }
        if ($freeAfter -lt ($freeBefore - 2MB)) { throw 'free space was not restored' }
        # the sampling above can miss a short-lived second file, so the proof is the amount the tool
        # wrote: one fat32 file holds less than 4 gib, and a full pass writes about the free space.
        # the printed amount has one decimal, hence the margin.
        $done = Select-String -LiteralPath $output, $errors -Pattern 'pass 1/1 done: ([0-9.]+) (gib|mib) written' |
            Select-Object -First 1
        if (-not $done) { throw 'the pass total is missing from the output' }
        $unit = if ($done.Matches[0].Groups[2].Value -eq 'gib') { 1GB } else { 1MB }
        $written = [double]::Parse($done.Matches[0].Groups[1].Value, [cultureinfo]::InvariantCulture) * $unit
        if ($written -lt ($freeBefore - 128MB)) {
            throw "the volume was not filled: written=$written free-before=$freeBefore"
        }
        if ($SizeMiB -eq 5120 -and $written -le (4GB - 1MB)) {
            throw "no rollover to a second file: written=$written"
        }
        if ($exitCode -ne 0) {
            throw "wipe failed with exit $exitCode"
        }
    } finally {
        if ($process -and -not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
        if (Test-Path -LiteralPath $image) {
            $info = Get-DiskImage -ImagePath $image -ErrorAction Stop
            if ($info.Attached) {
                $disks = @($info | Get-Disk)
                if ($null -eq $diskNumber -or $disks.Count -ne 1 -or $disks[0].Number -ne $diskNumber -or
                    $disks[0].Size -ne ($SizeMiB * 1MB)) { throw "cannot verify image for dismount: $image" }
                Dismount-DiskImage -ImagePath $image -ErrorAction Stop | Out-Null
            }
            if ((Get-DiskImage -ImagePath $image).Attached) { throw "image still attached: $image" }
            Remove-Item -LiteralPath $image -ErrorAction Stop
        }
    }
}

try {
    Invoke-VolumeCheck 128
    Invoke-VolumeCheck 5120
} finally {
    $images = @(Get-ChildItem -LiteralPath $scratch -Filter '*.vhd' -ErrorAction SilentlyContinue)
    Write-Output "retained evidence directory: $scratch"
    if ($images.Count -ne 0) { Write-Error "retained image for inspection: $scratch" }
}
