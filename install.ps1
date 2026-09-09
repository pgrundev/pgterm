# pgterm installer for Windows.
#
#   irm https://pgterm.dev/install.ps1 | iex
#
# Downloads the latest release zip, verifies its SHA-256 against the release's
# checksums.txt, and installs pgterm.exe into $env:LOCALAPPDATA\pgterm\bin
# (override with $env:PGTERM_INSTALL_DIR), adding that directory to the user
# PATH when it is not there yet. pgterm drives pgbot for every diagnostic;
# install that separately.
$ErrorActionPreference = "Stop"

$repo = "pgrundev/pgterm"
$dir = if ($env:PGTERM_INSTALL_DIR) { $env:PGTERM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "pgterm\bin" }

Write-Host "Fetching the latest pgterm release..."
$release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
$version = $release.tag_name.TrimStart("v")
$zip = "pgterm_${version}_windows_amd64.zip"

$asset = $release.assets | Where-Object name -eq $zip
if (-not $asset) { throw "release $version has no Windows build ($zip)" }
$sums = ($release.assets | Where-Object name -eq "checksums.txt").browser_download_url
if (-not $sums) { throw "release $version has no checksums.txt" }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "pgterm-install-$version"
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
  Invoke-WebRequest $asset.browser_download_url -OutFile (Join-Path $tmp $zip)
  Invoke-WebRequest $sums -OutFile (Join-Path $tmp "checksums.txt")

  $want = (Select-String -Path (Join-Path $tmp "checksums.txt") -Pattern ([regex]::Escape($zip))).Line.Split(" ")[0]
  $got = (Get-FileHash (Join-Path $tmp $zip) -Algorithm SHA256).Hash.ToLower()
  if ($want -ne $got) { throw "checksum mismatch for ${zip}: published $want, downloaded $got" }

  Expand-Archive (Join-Path $tmp $zip) -DestinationPath $tmp -Force
  New-Item -ItemType Directory -Force $dir | Out-Null
  Copy-Item (Join-Path $tmp "pgterm_${version}_windows_amd64\pgterm.exe") (Join-Path $dir "pgterm.exe") -Force
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$path = [Environment]::GetEnvironmentVariable("Path", "User")
if (($path -split ";") -notcontains $dir) {
  [Environment]::SetEnvironmentVariable("Path", "$path;$dir", "User")
  Write-Host "Added $dir to your user PATH (open a new terminal to pick it up)."
}

Write-Host "Installed pgterm $version to $dir"
Write-Host ""
Write-Host "pgterm drives pgbot, the diagnostic engine. Install it too:"
Write-Host "  https://pgbot.dev"
