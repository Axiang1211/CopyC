<p align="center"><img src="docs/logo.png" alt="CopyC" width="160"></p>

<h1 align="center">CopyC</h1>

<p align="center"><b>每一次複製，都看得見。</b><br>按下 Ctrl+C 的瞬間，螢幕上就告訴你：「已複製」，以及複製了什麼。<br>現在還能在一台電腦複製、在另一台直接貼上。</p>

<p align="center">
  <a href="https://www.copyc.app.dreamsworld.top/">官網</a> ·
  <a href="https://github.com/Axiang1211/CopyC/releases/latest">下載最新版</a> ·
  <a href="https://apps.microsoft.com/detail/9PGS1H8Z2N1P">Microsoft Store</a> ·
  <a href="https://github.com/Axiang1211/CopyC/issues">回報問題</a> ·
  <a href="https://www.copyc.app.dreamsworld.top/en/">English</a>
</p>

---

按了 Ctrl+C，卻不確定到底有沒有複製成功，只好再按兩三次；貼上時才發現貼的是舊內容。CopyC 就是為了解決這件事：它安靜地待在系統匣（Windows）或選單列（macOS），每次複製成功，螢幕上方就會滑下一個小小的提示，約 1.5 秒後自動消失。

Copy 的發音很像馬來語的 **Kopi**（咖啡），所以它叫 CopyC，也是一杯在你需要時回應你的咖啡。

> **CopyC 完全免費**，沒有廣告、付費版、會員或「啟用碼」。如果有人向你收費，或要你付款才能下載、啟用，那都不是我們，請不要付款。請只從[官網](https://www.copyc.app.dreamsworld.top/)、本頁的 [Releases](https://github.com/Axiang1211/CopyC/releases/latest) 或 [Microsoft Store](https://apps.microsoft.com/detail/9PGS1H8Z2N1P) 下載。

## 1.1.0 新功能

- **跨裝置同步（選用）**：用 Google 帳號登入後，在筆電複製的文字，桌機按 Ctrl+V 就能直接貼上，提示會告訴你「已同步，來自哪台電腦」。內容在你的電腦上就先加密，伺服器看不到。
- **雲端歷史備份**：歷史紀錄加密備份到你自己的 Google Drive，每台電腦的紀錄都能在「歷史紀錄」裡搜尋到。
- **新手引導**：第一次開啟時，一步步帶你試用、選位置、設定隱私，一分鐘完成。
- **歷史紀錄**（1.0.8）：搜尋複製過的內容、依類型篩選、一鍵重新複製。

完整的版本紀錄請見 [Releases](https://github.com/Axiang1211/CopyC/releases)。

## 下載

| 平台 | 下載 | 系統需求 |
| --- | --- | --- |
| Windows | [Microsoft Store](https://apps.microsoft.com/detail/9PGS1H8Z2N1P)（推薦，自動更新）或 .exe 安裝程式：[x64](https://github.com/Axiang1211/CopyC/releases/latest/download/CopyC-windows-x64-setup.exe)／[ARM64](https://github.com/Axiang1211/CopyC/releases/latest/download/CopyC-windows-arm64-setup.exe) | Windows 10 / 11（Intel / AMD 選 x64；Snapdragon 等 ARM 電腦選 ARM64） |
| macOS | [CopyC-macos-universal.dmg](https://github.com/Axiang1211/CopyC/releases/latest/download/CopyC-macos-universal.dmg) | macOS 11 以上，Apple Silicon 與 Intel 都可用 |

免費、無廣告。

### 安裝
**Windows：Microsoft Store（推薦）**

打開 [Copy C 的商店頁](https://apps.microsoft.com/detail/9PGS1H8Z2N1P)，按「取得」就完成，之後會自動更新。

**Windows：.exe 安裝程式**
1. 執行下載的 `.exe` 安裝程式。
2. 如果出現 SmartScreen「Windows 已保護您的電腦」，按「其他資訊 → 仍要執行」。
3. 安裝完成後，CopyC 會出現在右下角的系統匣（杯子圖示）。

**macOS**
1. 打開 `.dmg`，把 CopyC 拖進「應用程式」資料夾。
2. 第一次開啟如果被擋下，在 Finder 對 CopyC 按右鍵 →「打開」。
3. CopyC 只會出現在頂部選單列，不會出現在 Dock。

> 官網下載的安裝包還沒有付費的程式碼簽章，所以系統會出現一次安全性提醒，照上面的步驟操作即可。Microsoft Store 版不會有這個提醒。

已經裝了舊版？官網版開啟時會提醒你有新版本，下載後直接覆蓋安裝，設定與歷史紀錄都會保留；Microsoft Store 版會自動更新。

## 功能

**複製提示**
- **每次複製都有回應**：快捷鍵、右鍵選單、網頁上的「複製」按鈕都算，複製成功就會提示。
- **一眼看懂複製了什麼**：文字顯示開頭幾個字與字數、圖片顯示縮圖與尺寸、檔案顯示檔名與數量、色碼直接顯示顏色、網址標出網域。
- **連按也不閃爍**：同一份內容連續複製時，提示原地晃一下，並顯示「已確認 ×3」。
- **放在你喜歡的位置**：螢幕頂部、游標旁，或螢幕四個角落；停留時間、透明度、提示音都能調。
- **不擋路、不打擾**：提示不會搶走鍵盤焦點，滑鼠能直接點穿；玩遊戲、全螢幕看影片或簡報時會縮成一個小點，也可以設定成暫停。

**歷史紀錄**
- 在「偏好設定 → 歷史紀錄」搜尋複製過的內容，按一下就能重新複製。
- 自動分成文字、連結、Email、電話、色碼、圖片、檔案等類別，可以用標籤篩選。
- 預設保留 1,000 筆（可改 100～5,000 筆），可以逐筆刪除、一鍵清除或整個關閉。
- 圖片只保存縮圖，檔案只記錄位置，不會另外複製一份檔案。
- 把滑鼠移到提示上，也能看到最近 3 筆。

**跨裝置同步（選用）**
- 一台電腦複製的文字，其他電腦按 Ctrl+V 就能貼上（也可以設定成只記錄、不放進剪貼簿）。
- 歷史紀錄加密備份到你自己 Google Drive 的 App 專用資料夾，合併各台電腦的紀錄，保留 90 天。
- 只同步文字、連結、Email、電話與色碼；圖片、檔案與密碼永遠不會離開這台電腦。

**隱私與安全**
- **密碼不會出現在螢幕上**：從 1Password、Bitwarden 等密碼管理器複製的內容，或看起來像密碼、金鑰的內容，會自動遮蔽，預設也不會寫進歷史紀錄。
- **隱私模式**：開會分享螢幕時打開，提示與歷史紀錄只顯示類型，不顯示內容。
- **App 黑名單**：指定的 App 複製時，可以遮蔽內容或完全不提示，也不會記錄或同步。

**其他**
- **新手引導**：第一次開啟時帶你完成設定，之後可以在「偏好設定 → 一般」重看。
- **七種介面語言**：繁體中文、简体中文、English、日本語、한국어、Français、Bahasa Melayu。預設跟隨系統語言，也可在偏好設定或系統匣選單切換。

## 使用方式

安裝後第一次開啟會出現新手引導，跟著做完就能開始用；之後它會在背景運作，不需要再設定。

- **右鍵點杯子圖示**（Windows 系統匣／macOS 選單列）可以：暫停、切換隱私模式、開關提示音、調整位置與停留時間、切換語言、打開偏好設定。
- **偏好設定**可以調整更多細節，按「試一下提示」可以直接預覽各種提示的樣子。
- **開機自動啟動**：預設開啟，可以在「偏好設定 → 一般 → 開機自動啟動」關閉。
- **自動檢查更新**：官網版開啟時會檢查有沒有新版本，可以在「偏好設定 → 一般」關閉。

### 開啟跨裝置同步

1. 每台電腦都安裝 CopyC 1.1.0 以上的版本。
2. 打開「偏好設定 → 同步」，按「使用 Google 登入」，在瀏覽器選你的 Google 帳號並同意。
3. 在另一台電腦用**同一個 Google 帳號**做一樣的事。
4. 完成！在其中一台複製文字，另一台直接 Ctrl+V。

「同步」頁可以分別開關「即時同步」「直接放進剪貼簿」「備份歷史紀錄」，也可以「立即同步」「登出」或「刪除雲端資料」。

## 隱私

- 沒有登入同步時，CopyC 完全在你的電腦上運作，**不會上傳你複製的內容**，不需要帳號，也沒有廣告或追蹤程式。
- 它使用系統的剪貼簿事件來偵測複製，**不會記錄鍵盤輸入**。
- 同步是選用的，只有你登入 Google 才會啟用。同步的內容在你的電腦上就先加密（端對端加密），Cloudflare 上的中繼伺服器只負責轉送，看不到也不保存內容。
- CopyC 只要求 Google 的 `openid`、`email` 與 `drive.appdata` 權限：只能存取 CopyC 自己的隱藏資料夾，看不到你雲端硬碟裡的其他檔案。
- 隨時可以登出（會刪除其他電腦同步來的紀錄），或刪除 Google Drive 裡 CopyC 的所有資料。

完整內容請見[隱私權政策](https://www.copyc.app.dreamsworld.top/privacy/)與[使用條款](https://www.copyc.app.dreamsworld.top/terms/)。

## 解除安裝

1. 如果有用同步，先在「偏好設定 → 同步」按「刪除雲端資料」（不刪也可以，CopyC 的備份會在 90 天後自動清掉）。
2. 解除安裝：
   - **Windows**：設定 → 應用程式 → 已安裝的應用程式 → CopyC → 解除安裝。
   - **macOS**：先從選單列的杯子圖示選「結束」，再把「應用程式」資料夾裡的 CopyC 移到垃圾桶。

## Give me a Kopi ☕

CopyC 永遠免費、沒有廣告。如果它幫你少按了幾次 Ctrl+C，歡迎請我喝杯 Kopi，金額隨意：

- **馬來西亞（MYR）**：到[官網的 Give me a Kopi](https://www.copyc.app.dreamsworld.top/#kopi) 用任何銀行 App 或電子錢包掃 DuitNow QR。
- **其他地區**：透過 [GitHub Sponsors](https://github.com/sponsors/Axiang1211) 贊助。

贊助完全自願，不會解鎖任何功能；不贊助也能使用全部功能。

## 常見問題

**它會把我複製的內容傳到網路上嗎？**
預設不會。只有你自己登入 Google 開啟同步時，文字類內容才會先在你的電腦上加密，再傳到你的其他電腦與你自己的 Google Drive。圖片、檔案與密碼永遠不會同步。

**同步要付費嗎？需要什麼？**
不用付費。只要每台電腦都裝 CopyC 1.1.0 以上，並用同一個 Google 帳號登入。目前支援 Windows 與 macOS 電腦之間同步，還沒有手機版。

**另一台電腦沒收到？**
確認兩台都已登入同一個 Google 帳號、「同步」頁顯示「即時同步已連線」。即時同步只送給當下開著的電腦；沒開機的那台，之後可以在「歷史紀錄」找到（「備份歷史紀錄」要開著）。

**歷史紀錄會保存我的密碼嗎？**
預設不會。從密碼管理器複製的內容、或看起來像密碼、金鑰的內容，都不會寫入歷史紀錄，也永遠不會同步。如果你想在這台電腦保存，可以在「偏好設定 → 歷史紀錄」打開「保存密碼與敏感內容」；關掉時已保存的密碼會一併刪除。

**它是靠監聽鍵盤來偵測的嗎？**
不是。CopyC 監聽的是系統剪貼簿的變更，不記錄任何按鍵。所以沒選取內容就按 Ctrl+C 時，不會出現誤報。

**提示擋到我了怎麼辦？**
提示不會攔截滑鼠點擊，可以直接點穿。也可以在右鍵選單改到游標旁或螢幕角落，或者縮短停留時間。

**有人跟我收費，說是 CopyC 的付費版？**
那不是我們。CopyC 沒有付費版，請不要付款，並從官網、GitHub 或 Microsoft Store 重新下載正版。

**有問題或建議？**
歡迎到 [GitHub Issues](https://github.com/Axiang1211/CopyC/issues) 留言。

---

<details>
<summary>從原始碼建置（開發者）</summary>

需要 Node 22、Rust stable，以及 Tauri 的[平台相依套件](https://v2.tauri.app/start/prerequisites/)。

```bash
npm install
npm run dev         # 開發模式
npm test            # Rust 單元測試
npm run test:web    # 介面翻譯、通知框文案與同步伺服器的 JavaScript 測試
npm run build:win   # Windows .exe
npm run build:mac   # macOS Universal .dmg
```

跨裝置同步需要你自己的 Google OAuth 用戶端（類型「電腦版應用程式」，權限 `openid`、`email`、`drive.appdata`）。建置前設定環境變數 `COPYC_GOOGLE_CLIENT_ID`（以及選用的 `COPYC_GOOGLE_CLIENT_SECRET`）；沒設定時 App 仍可正常使用，同步頁會顯示「這個版本尚未開放同步」。即時同步的中繼伺服器在 `worker/`（Cloudflare Workers + Durable Objects），部署時需要設定 `GOOGLE_CLIENT_ID`。

</details>
