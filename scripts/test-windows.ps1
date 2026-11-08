# End-to-end check on a Windows CI runner (needs Administrator): installs the MSI, then makes a Booto USB
# from the newest Booto release (--channel nightly) on a 4 GiB virtual disk, checks it, and raw-writes the ISO.
# Usage: scripts/test-windows.ps1 <booto-writer.msi>
param([Parameter(Mandatory)][string]$Msi)
$ErrorActionPreference = 'Stop'
function Check($what) { if ($LASTEXITCODE) { throw "$what failed (exit $LASTEXITCODE)" } }

$log = Join-Path $env:RUNNER_TEMP 'msi.log'
$p = Start-Process msiexec.exe -ArgumentList '/i', "`"$((Resolve-Path $Msi).Path)`"", '/qn', '/l*v', "`"$log`"" -Wait -PassThru
if ($p.ExitCode) { Get-Content $log -Tail 40; throw "msiexec failed (exit $($p.ExitCode))" }
$exe = Join-Path $env:ProgramFiles 'booto-writer\booto-writer.exe'
& $exe --version; Check 'booto-writer --version'
if ([Environment]::GetEnvironmentVariable('Path', 'Machine') -notlike '*\booto-writer*') { throw 'the MSI did not add booto-writer to PATH' }

$vhd = Join-Path $env:RUNNER_TEMP 'usb.vhdx'
$script = Join-Path $env:RUNNER_TEMP 'vhd.txt'
Set-Content $script "create vdisk file=`"$vhd`" maximum=4096 type=expandable`r`nattach vdisk" -Encoding ascii
diskpart /s $script; Check 'diskpart'
$disk = Get-DiskImage -ImagePath $vhd | Get-Disk
$dev = "\\.\PhysicalDrive$($disk.Number)"
& $exe list --all; Check 'list'

& $exe booto $dev --channel nightly --force-nonremovable --yes; Check 'booto'
$esp = Get-Partition -DiskNumber $disk.Number | Where-Object GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}'
if (-not $esp -or $esp.Size -ne 3GB) { Get-Partition -DiskNumber $disk.Number | Format-List; throw 'no 3 GiB EFI system partition' }

# Windows does not mount an EFI system partition, so turn it into a basic data partition to look inside.
Set-Partition -DiskNumber $disk.Number -PartitionNumber $esp.PartitionNumber -GptType '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}'
Add-PartitionAccessPath -DiskNumber $disk.Number -PartitionNumber $esp.PartitionNumber -AssignDriveLetter
$usb = (Get-Partition -DiskNumber $disk.Number -PartitionNumber $esp.PartitionNumber).DriveLetter
$vol = Get-Volume -DriveLetter $usb
if ($vol.FileSystem -ne 'FAT32' -or $vol.FileSystemLabel -ne 'BOOTO_LIVE') { throw "volume is $($vol.FileSystem) $($vol.FileSystemLabel)" }

$iso = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'booto-writer') -Filter 'booto-*.iso' | Select-Object -First 1
$src = (Mount-DiskImage -ImagePath $iso.FullName -PassThru | Get-Volume).DriveLetter
try {
    foreach ($f in 'EFI\BOOT\BOOTX64.EFI', 'boot\grub\grub.cfg', 'live\vmlinuz', 'live\initrd.img', 'live\filesystem.squashfs') {
        $a = (Get-FileHash "${src}:\$f").Hash
        $b = (Get-FileHash "${usb}:\$f").Hash
        if ($a -ne $b) { throw "$f differs between the ISO and the USB" }
    }
} finally { Dismount-DiskImage -ImagePath $iso.FullName | Out-Null }
Write-Host "Booto USB OK: $($iso.Name) on $dev"

& $exe write $iso.FullName $dev --force-nonremovable --yes; Check 'write'
Write-Host 'Raw write OK'
