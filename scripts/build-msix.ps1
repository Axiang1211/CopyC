# 把 tauri build 產生的 copyc.exe 封裝成 .msix。
# 用法：pwsh scripts/build-msix.ps1 [-IdentityName ...] [-Publisher "CN=..."] [-PublisherDisplayName ...] [-CertPath cert.pfx -CertPassword ***]
# 上架 Microsoft Store：三個身分值填 Partner Center「產品管理 → 產品身分識別」裡的值，不需要簽章（Store 會代簽）。
# 自行側載：需要以 -CertPath 簽章，且 Publisher 必須與憑證 Subject 相同。
param(
  [string]$Exe = "src-tauri/target/release/copyc.exe",
  [string]$Arch = "x64",
  [string]$IdentityName = "CopyC",
  [string]$Publisher = "CN=CopyC",
  [string]$PublisherDisplayName = "CopyC",
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

Copy-Item $Exe "$layout/copyc.exe"
# 各尺寸 / 縮放比例的圖示（由 store/source/render.mjs icons 產生）
Copy-Item "packaging/msix/Assets/*.png" "$layout/Assets/"
(Get-Content "packaging/msix/AppxManifest.xml" -Raw) `
  -replace "__VERSION__", $version `
  -replace "__ARCH__", $Arch `
  -replace "__IDENTITY_NAME__", $IdentityName `
  -replace "__PUBLISHER_DISPLAY_NAME__", $PublisherDisplayName `
  -replace "__PUBLISHER__", $Publisher |
  Set-Content "$layout/AppxManifest.xml" -Encoding utf8

# resources.pri 讓 Windows 依縮放比例與工作列尺寸挑選對應的圖示（scale-200、targetsize-32…）
$makepri = Find-SdkTool "makepri.exe"
$priConfig = Join-Path $OutDir "priconfig.xml"
& $makepri createconfig /cf $priConfig /dq en-US /pv 10.0.0 /o
if ($LASTEXITCODE -ne 0) { throw "makepri createconfig 失敗" }
# 預設設定會把非預設縮放比例拆成 resources.scale-*.pri（給資源套件 / bundle 用）；
# 我們只出單一 .msix，所以移除 <packaging>，讓所有尺寸都索引在同一個 resources.pri
[xml]$cfg = Get-Content $priConfig
$packaging = $cfg.SelectSingleNode("//packaging")
if ($packaging) { [void]$packaging.ParentNode.RemoveChild($packaging) }
$cfg.Save((Resolve-Path $priConfig).Path)
& $makepri new /pr $layout /cf $priConfig /mn "$layout/AppxManifest.xml" /of "$layout/resources.pri" /o
if ($LASTEXITCODE -ne 0) { throw "makepri new 失敗" }

$msix = Join-Path $OutDir "CopyC_$($conf.version)_$Arch.msix"
& (Find-SdkTool "makeappx.exe") pack /o /d $layout /p $msix
if ($LASTEXITCODE -ne 0) { throw "makeappx 失敗" }

if ($CertPath) {
  & (Find-SdkTool "signtool.exe") sign /fd SHA256 /a /f $CertPath /p $CertPassword $msix
  if ($LASTEXITCODE -ne 0) { throw "signtool 簽章失敗" }
}
Write-Host "已產生 $msix"
