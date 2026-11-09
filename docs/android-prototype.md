# CopyC Android 原型（第 1 階段）

原始碼在 `mobile/`（Tauri 2）。加密、即時同步的訊息格式與 Google Drive 存取直接沿用電腦版的程式（`src-tauri/src/cloud/`），兩邊永遠一致。

## 這一版能做什麼

- 用 Google 登入：手機系統內建的 Google 帳號選擇與同意畫面（Google Play 服務），權限和電腦版一樣（openid、email、drive.appdata）。
- App 開著時，電腦上複製的文字直接放進手機的剪貼簿，畫面上列出最近收到的內容。
- 「把剪貼簿傳到電腦」：讀手機目前的剪貼簿傳到所有登入同一帳號的電腦。
- 在其他 App 按「分享」選「傳到電腦（CopyC）」，文字會傳到電腦。

還沒有的（第 2 階段）：背景接收、通知欄快速按鈕、歷史紀錄與篩選、區域網路直傳、其他 5 種語言。

## 第一次設定（約 10 分鐘）

### 1. Google Cloud：新增「Android」用戶端
1. 打開 <https://console.cloud.google.com/>，切換到 **CopyC** 專案（和電腦版同一個，才讀得到同一份加密金鑰）。
2. **Google Auth Platform → 用戶端（Clients）→ 建立用戶端**。
3. 應用程式類型選 **Android**，填：
   - 名稱：`CopyC Android`
   - 套件名稱：`top.dreamsworld.copyc`
   - SHA-1 憑證指紋：`4A:94:DF:E0:9C:9F:12:8D:43:65:67:05:B5:88:0B:8A:18:02:BD:7E`（上傳金鑰的指紋）
4. 建立後記下 **用戶端 ID**。權限範圍（openid、email、drive.appdata）是整個專案共用的，不用再設定。

之後上架 Google Play 時，Play 會用自己的「應用程式簽署金鑰」重新簽章，到時要把 Play Console 顯示的那個 SHA-1 也加一個 Android 用戶端。

（先前建立的「電視和輸入受限裝置」用戶端，以及 GitHub Secrets 的 `COPYC_ANDROID_CLIENT_ID`、`COPYC_ANDROID_CLIENT_SECRET` 已經用不到，可以刪除。Google 的那種登入方式不允許 Drive 的 App 專用資料夾權限。）

### 2. GitHub Secrets
GitHub → `Axiang1211/CopyC` → **Settings → Secrets and variables → Actions → New repository secret**：

| 名稱 | 值 |
| --- | --- |
| `COPYC_ANDROID_KEYSTORE` | 金鑰檔裡那一長串（upload.jks 的 base64） |
| `COPYC_ANDROID_KEYSTORE_PASSWORD` | 金鑰檔裡的密碼 |

上傳金鑰（upload key）之後上架 Google Play 也會用到，請備份在密碼管理器。

### 3. Cloudflare：讓中繼伺服器認得 Android 用戶端
Cloudflare 主控台 → **Workers 和 Pages → copyc → 設定 → 變數和祕密**：
- 祕密 `GOOGLE_CLIENT_ID_ANDROID` 的值改成（或新增為）第 1 步 **Android 用戶端** 的 ID。

原本的 `GOOGLE_CLIENT_ID`（電腦版）不要動。

## 建置與安裝

1. GitHub → **Actions → Android (prototype) → Run workflow**（約 10～15 分鐘）。
2. 完成後在該次執行頁面下載 **copyc-android**，解壓縮得到：
   - `CopyC-android-arm64.apk`：裝在自己手機上測試。
   - `CopyC-android.aab`：之後上架 Google Play 用（封閉測試）。
3. 把 APK 傳到手機打開安裝（第一次會要求允許「安裝不明應用程式」）。

## 測試步驟

1. 電腦上的 CopyC 已用 Google 登入並開啟「即時同步」。
2. 手機打開 CopyC → **用 Google 登入** → 選擇和電腦版相同的 Google 帳號並同意。
3. 右上角顯示「已連線」後，在電腦上複製一段文字 → 手機跳出「收到來自 … 的內容」，到任何 App 長按就能貼上。
4. 在手機上複製文字 → 回到 CopyC 按 **把剪貼簿傳到電腦** → 電腦上就能 Ctrl+V。
5. 在瀏覽器或記事 App 選一段文字 → **分享 → 傳到電腦（CopyC）**。

如果有問題，把手機畫面截圖或錯誤訊息貼給我。
