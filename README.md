# CopyC：桌面複製即時回饋工具

> 每一次複製，都有一杯 Kopi 的回應。告別「複製焦慮症」與連按三到五次 Ctrl+C 的習慣。
>
> 官網：<https://www.copyc.app.dreamsworld.top/>

**名字由來**：Copy 跟馬來語的 **Kopi**（咖啡）唸起來幾乎一樣。南洋 Kopitiam 裡的 **Kopi C** 是加淡奶與糖的咖啡，所以產品從原本的 Ctrl-C 改名為 **CopyC**，官網與 Logo 也以咖啡為主題設計。

CopyC 常駐在 Windows 系統匣（右下角）與 macOS 選單列（頂部）。系統剪貼簿一有變更，就在螢幕上方或游標旁彈出一個輕量提示：「你已成功複製……」。提示框不搶焦點、滑鼠可穿透，停留約 1.4 秒後自動淡出。

| 平台 | 安裝包 | 常駐位置 |
| --- | --- | --- |
| Windows 10 / 11 | `.exe`（NSIS）、`.msi`、`.msix` | 系統匣 |
| macOS 11+ | `.dmg`（Universal Binary：Apple Silicon + Intel） | 選單列（不出現在 Dock） |

技術選型依規格書建議採用 **Tauri 2（Rust 後端 + HTML/CSS 前端）**。前端是原生 HTML / CSS / ES Modules，不需要打包工具。

---

## 功能對照規格書

### 03 視覺與互動
- **不搶焦點 + 滑鼠穿透**：通知框是 Always on Top、不可聚焦的透明視窗。
  - Windows 使用 `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`，並以 `SW_SHOWNOACTIVATE` 顯示。
  - macOS 使用狀態列層級，並以 `orderFrontRegardless` 顯示；可跨所有桌面空間，也能浮在全螢幕 App 上。
  - 兩個平台都開啟 `set_ignore_cursor_events`，點擊會穿透到後方的軟體。
- **雙顯示位置**：
  - 模式 A 是「螢幕正上方膠囊」，會從頂部滑下，再像靈動島一樣由中央撐開。
  - 模式 B 是「游標右上方微氣泡」，貼近螢幕邊緣時會自動翻到左側或下方。
  - 另可選螢幕四角。提示一律出現在游標所在的螢幕。
- **連按防抖動彩蛋**：1 秒內（或提示仍在畫面上時）再次複製同一份內容，提示不會關掉重開，而是原地輕震一下，並更新為「你已成功複製！（已確認 ×3 次）／放心吧！內容已經安全躺在剪貼簿裡了」。

### 04 智慧內容識別
| 類型 | 顯示 |
| --- | --- |
| 純文字／段落 | 換行轉成空白，預覽前 25 字，附「共 N 字元 · M 行」 |
| 圖片／截圖 | 32×32 圓角縮圖（以 64px 輸出，支援高 DPI），附解析度與格式（PNG / TIFF / BMP …） |
| 檔案／資料夾 | 單一檔案顯示檔名與**系統檔案圖示**；多檔案顯示「首個檔名 等共 N 個項目」 |
| 色碼 | HEX / RGB / HSL 以正則識別，直接渲染同色圓點 |
| 網址 | 地球圖示並高亮網域 |
| 密碼／敏感資訊 | 🔒 `••••••••`，附「已啟動隱私安全遮罩保護」；明文在 Rust 端就丟棄，不會進入事件或歷史 |

敏感內容依三種來源判定：
- 密碼管理器加上的隱私標記：macOS 為 `org.nspasteboard.ConcealedType`；Windows 為 `ExcludeClipboardContentFromMonitorProcessing`、`CanIncludeInClipboardHistory = 0` 等。
- 來源 App 在黑名單內。
- 內容符合高熵值密碼特徵。

### 05 隱私與底層架構
- **監聽系統剪貼簿事件，而不是攔截按鍵**：
  - Windows 使用 `AddClipboardFormatListener`（message-only 視窗，零輪詢）。
  - macOS 使用 `NSPasteboard.changeCount`，每 150ms 檢查一次，並關閉 App Nap 以免被系統節流。
  - 因此快捷鍵、右鍵選單、網頁的「一鍵複製」按鈕都會觸發；沒選到內容就按複製時，剪貼簿不會變動，也就不會誤報。
- **隱私打碼模式**：只顯示「✓ 你已成功複製」，連先前的歷史紀錄也會一併遮蔽。
- **應用程式黑名單**：可設定為「顯示 🔒 遮罩提示」或「完全不提示」。
- **全螢幕勿擾**：偵測到前景 App 處於全螢幕時，可選擇切換為極簡微點、自動暫停或照常顯示。
  - Windows 依據 `SHQueryUserNotificationState`（D3D 全螢幕、簡報模式），以及前景視窗是否覆蓋整個螢幕來判斷。
  - macOS 依據 `CGWindowList` 中前景 App 的視窗是否與螢幕等大來判斷，不需要螢幕錄製權限。

### 06 命名與 v1.1 規劃（已一併實作）
- 產品名稱改為 **CopyC**（Copy 通 Kopi）；兩個平台的介面與選單都以 CopyC 顯示。
- 系統匣選單提供：暫停、顯示位置、停留秒數、透明度、音效、隱私打碼、試一下、偏好設定。
- **懸停展開最近 3 筆紀錄**：提示框本身是滑鼠穿透的，所以改由 Rust 在提示顯示期間輪詢游標位置。如此「點擊穿透」與「懸停展開」可以同時成立。

---

## UI/UX 設計（依 `vibe_ui_ux_skill`）

| Skill 規範 | 套用位置 |
| --- | --- |
| 暗色便當盒撞色風色票 | 偏好設定整體；品牌綠 `#10B981`，琥珀／珊瑚／天空藍／薰衣草紫作為類型色 |
| 懸浮島式側邊欄（Floating Island） | 偏好設定左側：離邊 12px、18px 圓角、多層柔和陰影 |
| 錨點導航（Scrollspy）＋滾動收縮頂欄 | 側邊欄隨捲動高亮；高亮膠囊以彈簧曲線移動；頂欄下滾後變矮並轉為毛玻璃 |
| Split Button | 「試一下提示」：主區直接預覽，箭頭展開其他類型（圖片、檔案、連按彩蛋、全螢幕微點…） |
| Select | 黑名單行為、全螢幕行為；空間不足時自動改為向上展開 |
| Filter Chips | 智慧預覽類型：選中時長出 ✓、底色漸層填滿，計數徽章跳動 |
| Removable Tags ＋ Suggest Popover | 黑名單：刪除時縮成圓點，其餘標籤以 FLIP 補位；輸入時在游標旁浮現聯想（最近使用的 App、常見密碼工具） |
| Size Dial | 停留秒數 0.8–3.0：兩側漸隱，可拖曳／滾輪／方向鍵，停下吸附 |
| Card Tray | 總覽卡片下方抽出最近紀錄，主卡片不動；HUD 懸停時的歷史托盤也是同一個模式 |
| Press Tilt | 總覽主卡片按壓時朝觸點傾斜，光斑跟著聚攏 |
| 視覺對齊 | HUD 主列與歷史托盤共用同一條文字基準線（`--text-rail`）；播放三角形右移 7%；鎖頭上移 5%；數字一律 `tabular-nums` |
| 延遲淡入 | 膠囊撐開後，內容延遲 160ms 淡入；托盤列表延遲 120ms 起逐列出現 |
| 觸控回饋 | 開關、按鈕、標籤都使用 `cubic-bezier(0.34, 1.56, 0.64, 1)` 彈簧曲線 |

偏好設定支援淺色與深色模式，並遵守 `prefers-reduced-motion`。剪貼簿內容一律以 `textContent` 寫入畫面，不會被當成 HTML 解讀。

---

## 開發

需要：Node 22、Rust stable，以及 Tauri 的[平台相依套件](https://v2.tauri.app/start/prerequisites/)。

```bash
npm install
npm run dev        # 開發模式
npm run build      # 建置桌面 App（等同 tauri build；在 Cloudflare Pages 上自動略過）
npm test           # Rust 單元測試（內容識別、敏感判斷、連按判斷、設定、縮圖…）
```

Linux 只用於開發：以輪詢替代系統剪貼簿事件，所以無法偵測「重複複製同一內容」，也沒有檔案清單。

### 打包

```bash
npm run build:mac   # macOS Universal .dmg（需先 rustup target add aarch64-apple-darwin x86_64-apple-darwin）
npm run build:win   # Windows NSIS .exe；npx tauri build --bundles nsis,msi 可同時產生 .msi
pwsh scripts/build-msix.ps1 [-Publisher "CN=..."] [-CertPath x.pfx -CertPassword ***]   # .msix
```

`.github/workflows/release.yml` 會在 GitHub Actions 上：
1. 執行測試。
2. 打包 macOS `.dmg` 與 Windows `.exe` / `.msi` / `.msix`。
3. 推送 `v*` tag 時建立 Release 草稿。

簽章所需的 secrets 都是選填：`APPLE_CERTIFICATE`、`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID`、`WINDOWS_CERTIFICATE`（base64 編碼的 pfx）、`WINDOWS_CERTIFICATE_PASSWORD`、`MSIX_PUBLISHER`。沒有設定時，產出的安裝包未簽章，安裝時會出現 Gatekeeper 或 SmartScreen 警告；未簽章的 `.msix` 則需要開啟開發人員模式才能側載。

## 官網（GitHub + Cloudflare Pages）

`website/` 是純靜態網站（HTML / CSS / ES Module，不需打包），包含：
- **主頁介紹**：可以直接在網頁上選取文字並按 Ctrl/⌘+C，體驗與 App 相同的「你已成功複製」膠囊。連按會出現 ×N 彩蛋。
- **名字由來**：以 Kopitiam 菜單板對照 Kopi / Kopi C / Kopi O / Kopi Kosong / Kopi Peng 與各項功能。
- **功能**：橫向手風琴（手機上改為直式）。
- **下載**：往下滑就能看到。點擊後直接下載 GitHub Releases 的最新安裝包。
  - Windows 用 Split Button：主鍵下載 `.exe`，箭頭展開 `.msi` 與 `.msix`。
  - Microsoft Store 欄位已預留：在 `website/index.html` 的 `#download` 設定 `data-ms-store-id="<Store 產品 ID>"` 後會自動啟用。
  - 頁面會偵測訪客的作業系統並標示「為你推薦」；版本號與檔案大小會從 GitHub API 自動讀取。
  - 還沒有任何 Release 時，下載按鈕會改連到 Releases 頁面。

下載連結使用固定檔名：`CopyC-windows-x64-setup.exe`、`CopyC-windows-x64.msi`、`CopyC-windows-x64.msix`、`CopyC-macos-universal.dmg`。推送 `v*` tag 時，`release.yml` 會用這些檔名發布正式 Release，所以官網連結永遠指向最新版本。

### 部署到 Cloudflare Pages

方式一（推薦，免 secrets）：在 Cloudflare Dashboard 進入 **Workers & Pages → Create → Pages → Connect to Git**，選擇這個 repo，設定如下：
- Build command：留空，或保留自動偵測的 `npm run build`。在 Cloudflare 上（`CF_PAGES=1`）它會直接略過，不會去建置桌面 App。
- Build output directory：`website`

方式二（GitHub Actions）：在 repo 設定兩個 secrets：
- `CLOUDFLARE_API_TOKEN`，權限需要 Cloudflare Pages: Edit。
- `CLOUDFLARE_ACCOUNT_ID`。

之後只要 `website/` 有變更，`.github/workflows/website.yml` 就會自動部署到 Pages 專案 `copyc`。

自訂網域：在 Pages 專案的 **Custom domains** 加入 `www.copyc.app.dreamsworld.top`。若 `dreamsworld.top` 的 DNS 由 Cloudflare 管理，會自動建立 CNAME；否則請自行把 `www.copyc.app` 以 CNAME 指向 `copyc.pages.dev`。

### 專案結構

```
src/                     前端（無打包工具）
  hud.html/css/js        通知框
  settings.html/css/js   偏好設定
  shared/compose.js      共用的文案與視覺組裝
src-tauri/src/
  monitor.rs             剪貼簿管線：事件 → 快照 → 識別 → 通知框
  classify.rs            智慧內容識別與敏感內容判斷
  hud.rs                 通知框定位、顯示／隱藏、連按防抖動、懸停輪詢
  tray.rs                系統匣／選單列選單
  settings.rs, state.rs  設定持久化、歷史與統計
  platform/              windows.rs、macos.rs、fallback.rs（Linux 開發用）
packaging/msix/          MSIX 清單（開機啟動改用 StartupTask）
website/                 官網（Cloudflare Pages）；assets/logo.svg 為吉祥物 × Kopi 杯 Logo 原稿
```

## 已知限制
- **記憶體**：Rust 常駐程序本身很小，但 WebView2／WKWebView 會另外啟動系統瀏覽器引擎程序，整體記憶體會高於規格的 25MB 目標。為了壓低用量，偏好設定視窗是需要時才建立、關閉即銷毀，平時只保留一個通知框 WebView。若要嚴格達到 25MB，需要把通知框改為原生繪製（WinUI / AppKit）。
- **MSIX 版的開機啟動**由清單中的 StartupTask 控制。偏好設定裡的「開機自動啟動」開關只對 `.exe`、`.msi`、`.dmg` 版本有效；MSIX 使用者需要到「設定 → 應用程式 → 啟動」調整。
- **macOS 透明視窗**需要開啟 `macOSPrivateApi`，因此無法上架 Mac App Store；以 `.dmg` 發行不受影響。
