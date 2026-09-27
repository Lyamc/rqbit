<#
  Registers rqbit-gpui.exe for the current user as a handler for magnet: links
  and .torrent files (same registry entries as `rqbit-gpui.exe --register-handlers`),
  or removes them with -Unregister. No admin rights needed (HKCU only).

  Windows 10/11 don't let programs make themselves the default silently:
  afterwards, choose rqbit in Settings > Apps > Default apps (opened for you).

  Usage:
    powershell -ExecutionPolicy Bypass -File register-handlers.ps1 -Exe "C:\path\rqbit-gpui.exe"
    powershell -ExecutionPolicy Bypass -File register-handlers.ps1 -Unregister
#>
param(
  [string]$Exe = (Join-Path $PSScriptRoot "rqbit-gpui.exe"),
  [switch]$Unregister
)
$ErrorActionPreference = "Stop"

$Classes = "HKCU:\Software\Classes"
$Caps = "HKCU:\Software\rqbit\Capabilities"

function Set-Value($Key, $Name, $Data) {
  if (-not (Test-Path $Key)) { New-Item -Path $Key -Force | Out-Null }
  if ($null -eq $Name) {
    Set-Item -Path $Key -Value $Data
  } else {
    New-ItemProperty -Path $Key -Name $Name -Value $Data -PropertyType String -Force | Out-Null
  }
}

if ($Unregister) {
  foreach ($k in @("$Classes\rqbit.Magnet", "$Classes\rqbit.Torrent", "HKCU:\Software\rqbit")) {
    if (Test-Path $k) { Remove-Item -Path $k -Recurse -Force }
  }
  Remove-ItemProperty -Path "HKCU:\Software\RegisteredApplications" -Name "rqbit" -ErrorAction SilentlyContinue
  Remove-ItemProperty -Path "$Classes\.torrent\OpenWithProgids" -Name "rqbit.Torrent" -ErrorAction SilentlyContinue
  $m = "$Classes\magnet\shell\open\command"
  if ((Test-Path $m) -and ((Get-Item $m).GetValue("") -like "*rqbit*")) {
    Remove-Item -Path "$Classes\magnet" -Recurse -Force
  }
  if ((Test-Path "$Classes\.torrent") -and ((Get-Item "$Classes\.torrent").GetValue("") -eq "rqbit.Torrent")) {
    Remove-ItemProperty -Path "$Classes\.torrent" -Name "(default)" -ErrorAction SilentlyContinue
  }
  Write-Host "rqbit is no longer registered for magnet links and .torrent files."
  exit 0
}

if (-not (Test-Path $Exe)) { throw "rqbit-gpui.exe not found at $Exe (pass -Exe)" }
$Exe = (Resolve-Path $Exe).Path
$Cmd = "`"$Exe`" `"%1`""
$Icon = "`"$Exe`",0"

Set-Value "$Classes\rqbit.Magnet" $null "URL:Magnet link (rqbit)"
Set-Value "$Classes\rqbit.Magnet" "URL Protocol" ""
Set-Value "$Classes\rqbit.Magnet\DefaultIcon" $null $Icon
Set-Value "$Classes\rqbit.Magnet\shell\open\command" $null $Cmd
Set-Value "$Classes\rqbit.Torrent" $null "BitTorrent file (rqbit)"
Set-Value "$Classes\rqbit.Torrent\DefaultIcon" $null $Icon
Set-Value "$Classes\rqbit.Torrent\shell\open\command" $null $Cmd
Set-Value "$Classes\.torrent\OpenWithProgids" "rqbit.Torrent" ""
Set-Value $Caps "ApplicationName" "rqbit"
Set-Value $Caps "ApplicationDescription" "Adds magnet links and .torrent files to your rqbit server."
Set-Value $Caps "ApplicationIcon" $Icon
Set-Value "$Caps\URLAssociations" "magnet" "rqbit.Magnet"
Set-Value "$Caps\FileAssociations" ".torrent" "rqbit.Torrent"
Set-Value "HKCU:\Software\RegisteredApplications" "rqbit" "Software\rqbit\Capabilities"

# Nothing handles magnet links / .torrent files yet: take them directly.
if (-not (Test-Path "Registry::HKEY_CLASSES_ROOT\magnet\shell\open\command")) {
  Set-Value "$Classes\magnet" $null "URL:Magnet link"
  Set-Value "$Classes\magnet" "URL Protocol" ""
  Set-Value "$Classes\magnet\shell\open\command" $null $Cmd
}
$t = Get-Item "Registry::HKEY_CLASSES_ROOT\.torrent" -ErrorAction SilentlyContinue
if (-not $t -or -not $t.GetValue("")) {
  Set-Value "$Classes\.torrent" $null "rqbit.Torrent"
}

Write-Host "Registered $Exe for magnet links and .torrent files."
Write-Host "Windows doesn't let apps make themselves the default: choose rqbit for MAGNET and .torrent in the Settings page that opens now."
Start-Process "ms-settings:defaultapps?registeredAppUser=rqbit"
