<p align="center"><img src="docs/logo.png" alt="CopyC" width="160"></p>

<h1 align="center">CopyC</h1>

<p align="center"><b>每一次複製，都看得見。</b><br>按下 Ctrl+C 的瞬間，螢幕上就告訴你：「已複製」，以及複製了什麼。</p>

<p align="center">
  <a href="https://www.copyc.app.dreamsworld.top/">官網</a> ·
  <a href="https://github.com/Axiang1211/CopyC/releases/latest">下載最新版</a> ·
  <a href="https://github.com/Axiang1211/CopyC/issues">回報問題</a> ·
  <a href="https://www.copyc.app.dreamsworld.top/en/">English</a>
</p>

---

按了 Ctrl+C，卻不確定到底有沒有複製成功，只好再按兩三次；貼上時才發現貼的是舊內容。CopyC 就是為了解決這件事：它安靜地待在系統匣（Windows）或選單列（macOS），每次複製成功，螢幕上方就會滑下一個小小的提示，約 1.5 秒後自動消失。

Copy 的發音很像馬來語的 **Kopi**（咖啡），所以它叫 CopyC，也是一杯在你需要時回應你的咖啡。

## 下載

| 平台 | 下載 | 系統需求 |
| --- | --- | --- |
| Windows | [CopyC-windows-x64-setup.exe](https://github.com/Axiang1211/CopyC/releases/latest/download/CopyC-windows-x64-setup.exe) | Windows 10 / 11，64 位元 |
| macOS | [CopyC-macos-universal.dmg](https://github.com/Axiang1211/CopyC/releases/latest/download/CopyC-macos-universal.dmg) | macOS 11 以上，Apple Silicon 與 Intel 都可用 |

免費、無廣告。Microsoft Store 版即將上架。

### 安裝
**Windows**
1. 執行下載的 `.exe` 安裝程式。
2. 如果出現 SmartScreen「Windows 已保護您的電腦」，按「其他資訊 → 仍要執行」。
3. 安裝完成後，CopyC 會出現在右下角的系統匣（杯子圖示）。

**macOS**
1. 打開 `.dmg`，把 CopyC 拖進「應用程式」資料夾。
2. 第一次開啟如果被擋下，在 Finder 對 CopyC 按右鍵 →「打開」。
3. CopyC 只會出現在頂部選單列，不會出現在 Dock。

> 目前的安裝包還沒有付費的程式碼簽章，所以系統會出現一次安全性提醒，照上面的步驟操作即可。

## 功能

- **每次複製都有回應**：快捷鍵、右鍵選單、網頁上的「複製」按鈕都算，複製成功就會提示。
- **一眼看懂複製了什麼**
  - 文字：開頭幾個字與字數
  - 圖片：縮圖與尺寸
  - 檔案：檔名、圖示與數量
  - 色碼：直接顯示顏色
  - 網址：標出網域
- **連按也不閃爍**：同一份內容連續複製時，提示不會一直關掉重開，而是原地晃一下，並顯示「已確認 ×3」。
- **密碼不會出現在螢幕上**：從 1Password、Bitwarden 等密碼管理器複製的內容，或看起來像密碼、金鑰的內容，會自動遮蔽。
- **隱私模式**：開會分享螢幕時打開，提示只顯示「已複製」，不顯示內容。
- **App 黑名單**：指定的 App 複製時，可以選擇遮蔽內容或完全不提示。
- **不打擾專注**：玩遊戲、全螢幕看影片或簡報時，提示會縮成一個小點，也可以設定成暫停。
- **不擋路**：提示不會搶走鍵盤焦點，滑鼠也能直接點穿它。
- **放在你喜歡的位置**：螢幕頂部、游標旁，或螢幕四個角落。
- **最近 3 筆紀錄**：把滑鼠移到提示上，就能看到最近複製的內容。
- **繁體中文 / English 介面**：預設跟隨系統語言，可以隨時切換。

## 使用方式

安裝後就會開始運作，不需要任何設定。

- **右鍵點杯子圖示**（Windows 系統匣／macOS 選單列）可以：暫停、切換隱私模式、開關提示音、調整位置與停留時間、切換語言、打開偏好設定。
- **偏好設定**可以調整更多細節：顯示位置、停留秒數、透明度、要辨識哪些內容類型、黑名單、全螢幕時的行為等。按「試一下提示」可以直接預覽各種提示的樣子。
- **開機自動啟動**：預設開啟，可以在「偏好設定 → 一般 → 開機自動啟動」關閉。
- **自動檢查更新**：開啟 CopyC 時會檢查有沒有新版本，有的話會提醒你前往官網下載，新版會直接覆蓋安裝並保留設定。可以在「偏好設定 → 一般」關閉。

## 隱私

- CopyC 完全在你的電腦上運作，**不會上傳你複製的任何內容**，不需要帳號，也沒有廣告或追蹤程式。
- 它使用系統的剪貼簿事件來偵測複製，**不會記錄鍵盤輸入**。
- 最近的複製紀錄只存在記憶體裡，關閉 CopyC 就會清除。
- 唯一的連網動作是檢查新版本（只查詢版本號，可以關閉）。

完整內容請見[隱私權政策](https://www.copyc.app.dreamsworld.top/privacy/)。

## 解除安裝

- **Windows**：設定 → 應用程式 → 已安裝的應用程式 → CopyC → 解除安裝。
- **macOS**：先從選單列的杯子圖示選「結束」，再把「應用程式」資料夾裡的 CopyC 移到垃圾桶。

## 常見問題

**它會把我複製的內容傳到網路上嗎？**
不會。複製的內容只用來在你的電腦上顯示提示。

**它是靠監聽鍵盤來偵測的嗎？**
不是。CopyC 監聽的是系統剪貼簿的變更，不記錄任何按鍵。所以沒選取內容就按 Ctrl+C 時，不會出現誤報。

**提示擋到我了怎麼辦？**
提示不會攔截滑鼠點擊，可以直接點穿。也可以在右鍵選單改到游標旁或螢幕角落，或者縮短停留時間。

**有問題或建議？**
歡迎到 [GitHub Issues](https://github.com/Axiang1211/CopyC/issues) 留言。

---

<details>
<summary>從原始碼建置（開發者）</summary>

需要 Node 22、Rust stable，以及 Tauri 的[平台相依套件](https://v2.tauri.app/start/prerequisites/)。

```bash
npm install
npm run dev         # 開發模式
npm test            # 單元測試
npm run build:win   # Windows .exe
npm run build:mac   # macOS Universal .dmg
```

</details>
