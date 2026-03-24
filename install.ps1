# shp2geojson Windows installer — https://github.com/s19835/shp2geojson
# Usage: irm https://raw.githubusercontent.com/s19835/shp2geojson/master/install.ps1 | iex
$ErrorActionPreference = "Stop"

$Repo    = "s19835/shp2geojson"
$BinName = "shp2geojson.exe"
$Target  = "x86_64-pc-windows-msvc"
$InstallDir = "$env:USERPROFILE\.local\bin"

# ── Fetch latest release version ─────────────────────────────────────────────
Write-Host "Fetching latest shp2geojson release..."
$Release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
$Version = $Release.tag_name -replace '^v', ''

# ── Download and install ──────────────────────────────────────────────────────
$Url = "https://github.com/$Repo/releases/download/v$Version/shp2geojson-$Target.zip"
$TmpDir = [System.IO.Path]::GetTempPath() + [System.IO.Path]::GetRandomFileName()
New-Item -ItemType Directory -Path $TmpDir | Out-Null

Write-Host "Downloading shp2geojson v$Version..."
$ZipPath = "$TmpDir\shp2geojson.zip"
Invoke-WebRequest -Uri $Url -OutFile $ZipPath

Expand-Archive -Path $ZipPath -DestinationPath $TmpDir

# Create install dir and move binary
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir | Out-Null
}
Move-Item -Path "$TmpDir\$BinName" -Destination "$InstallDir\$BinName" -Force
Remove-Item $TmpDir -Recurse -Force

# ── Add to user PATH if not already there ────────────────────────────────────
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("PATH", "$UserPath;$InstallDir", "User")
    Write-Host ""
    Write-Host "  Added $InstallDir to your PATH."
    Write-Host "  Restart your terminal for the change to take effect."
    Write-Host ""
}

Write-Host "Done! Run: shp2geojson --help"
