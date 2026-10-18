# 把 tauri build 產生的 ctrl-c.exe 封裝成 .msix。
# 用法：pwsh scripts/build-msix.ps1 [-Exe <路徑>] [-Publisher "CN=..."] [-CertPath cert.pfx -CertPassword ***]
# 沒有提供憑證時只產生未簽章的 .msix（安裝前需自行以 signtool 簽章，或開啟開發人員模式側載）。
param(
  [string]$Exe = "src-tauri/target/release/ctrl-c.exe",
  [string]$Arch = "x64",
  [string]$Publisher = "CN=Ctrl-C",
  [string]$OutDir = "dist",
  [string]$CertPath = "",
  [string]$CertPassword = ""
)
$ErrorActionPreference = "Stop"

$conf = Get-Content "src-tauri/tauri.conf.json" -Raw | ConvertFrom-Json
$version = "$($conf.version).0"   # MSIX 需要四段式版本號

function Find-SdkTool($name) {
  $tool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\$name" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1
  if (-not $tool) { throw "找不到 $name，請安裝 Windows 10/11 SDK" }
  $tool.FullName
}

if (-not (Test-Path $Exe)) { throw "找不到 $Exe，請先執行 npm run build:win" }

$layout = Join-Path $OutDir "msix-layout"
Remove-Item $layout -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path "$layout/Assets" -Force | Out-Null

Copy-Item $Exe "$layout/ctrl-c.exe"
foreach ($asset in "StoreLogo.png", "Square150x150Logo.png", "Square44x44Logo.png") {
  Copy-Item "src-tauri/icons/$asset" "$layout/Assets/$asset"
}
(Get-Content "packaging/msix/AppxManifest.xml" -Raw) `
  -replace "__VERSION__", $version `
  -replace "__ARCH__", $Arch `
  -replace "__PUBLISHER__", $Publisher |
  Set-Content "$layout/AppxManifest.xml" -Encoding utf8

$msix = Join-Path $OutDir "Ctrl-C_$($conf.version)_$Arch.msix"
& (Find-SdkTool "makeappx.exe") pack /o /d $layout /p $msix
if ($LASTEXITCODE -ne 0) { throw "makeappx 失敗" }

if ($CertPath) {
  & (Find-SdkTool "signtool.exe") sign /fd SHA256 /a /f $CertPath /p $CertPassword $msix
  if ($LASTEXITCODE -ne 0) { throw "signtool 簽章失敗" }
}
Write-Host "已產生 $msix"
