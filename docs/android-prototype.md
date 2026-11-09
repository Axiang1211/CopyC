# CopyC Android 原型（第 1 階段）

原始碼在 `mobile/`（Tauri 2）。加密、即時同步的訊息格式與 Google Drive 存取直接沿用電腦版的程式（`src-tauri/src/cloud/`），兩邊永遠一致。

## 這一版能做什麼

- 用 Google 登入：App 顯示一組代碼，到 google.com/device 輸入後同意（裝置代碼流程，不怕 App 在背景被系統暫停）。
- App 開著時，電腦上複製的文字直接放進手機的剪貼簿，畫面上列出最近收到的內容。
- 「把剪貼簿傳到電腦」：讀手機目前的剪貼簿傳到所有登入同一帳號的電腦。
- 在其他 App 按「分享」選「傳到電腦（CopyC）」，文字會傳到電腦。

還沒有的（第 2 階段）：背景接收、通知欄快速按鈕、歷史紀錄與篩選、區域網路直傳、其他 5 種語言。

## 第一次設定（約 10 分鐘）

### 1. Google Cloud：新增「電視和輸入受限裝置」用戶端
1. 打開 <https://console.cloud.google.com/>，切換到 **CopyC** 專案（和電腦版同一個，才讀得到同一份加密金鑰）。
2. **Google Auth Platform → 用戶端（Clients）→ 建立用戶端**。
3. 應用程式類型選 **電視和輸入受限裝置（TVs and Limited Input devices）**，名稱填 `CopyC Android`，建立。
4. 記下 **用戶端 ID** 和 **用戶端密鑰**。權限範圍（openid、email、drive.appdata）是整個專案共用的，不用再設定。

### 2. GitHub Secrets
GitHub → `Axiang1211/CopyC` → **Settings → Secrets and variables → Actions → New repository secret**：

| 名稱 | 值 |
| --- | --- |
| `COPYC_ANDROID_CLIENT_ID` | 第 1 步的用戶端 ID |
| `COPYC_ANDROID_CLIENT_SECRET` | 第 1 步的用戶端密鑰 |
| `COPYC_ANDROID_KEYSTORE` | 另外傳給你的金鑰檔裡那一長串（upload.jks 的 base64） |
| `COPYC_ANDROID_KEYSTORE_PASSWORD` | 金鑰檔裡的密碼 |

上傳金鑰（upload key）之後上架 Google Play 也會用到，請備份在密碼管理器。

### 3. Cloudflare：讓中繼伺服器認得 Android 用戶端
Cloudflare 主控台 → **Workers 和 Pages → copyc → 設定 → 變數和祕密 → 新增**：
- 類型：**祕密（Secret）**
- 名稱：`GOOGLE_CLIENT_ID_ANDROID`
- 值：第 1 步的用戶端 ID

原本的 `GOOGLE_CLIENT_ID`（電腦版）不要動。

## 建置與安裝

1. GitHub → **Actions → Android (prototype) → Run workflow**（約 10～15 分鐘）。
2. 完成後在該次執行頁面下載 **copyc-android**，解壓縮得到：
   - `CopyC-android-arm64.apk`：裝在自己手機上測試。
   - `CopyC-android.aab`：之後上架 Google Play 用（封閉測試）。
3. 把 APK 傳到手機打開安裝（第一次會要求允許「安裝不明應用程式」）。

## 測試步驟

1. 電腦上的 CopyC 已用 Google 登入並開啟「即時同步」。
2. 手機打開 CopyC → **用 Google 登入** → 代碼已自動複製 → **開啟 google.com/device** → 貼上代碼並同意 → 回到 CopyC。
3. 右上角顯示「已連線」後，在電腦上複製一段文字 → 手機跳出「收到來自 … 的內容」，到任何 App 長按就能貼上。
4. 在手機上複製文字 → 回到 CopyC 按 **把剪貼簿傳到電腦** → 電腦上就能 Ctrl+V。
5. 在瀏覽器或記事 App 選一段文字 → **分享 → 傳到電腦（CopyC）**。

如果有問題，把手機畫面截圖或錯誤訊息貼給我。
