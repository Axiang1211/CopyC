# 遠端資料（不用發新版本就能推給所有用戶）

`remote.json` 是 App 每次啟動、以及之後每 6 小時會下載的資料。改這個檔案並推上 GitHub，
**Publish remote data** 這個 Actions 會用私鑰簽章，更新 `website/config/v1.json`，官網部署後就生效。
官網版與 Microsoft Store 版都會收到，不用送審。

App 只接受用 `COPYC_REMOTE_SIGNING_KEY` 簽過的檔案，網站被竄改或連線被攔截都不會被採用。
這裡只能放**資料**（公告、名單、文字、開關），不能放程式碼；要改程式請發新版本（會自動更新）。

## 可以放什麼

```jsonc
{
  // 偏好設定最上方的公告。用戶按「知道了」後不再顯示同一個 id
  "notices": [
    {
      "id": "relay-maintenance-2026-11",          // 每則公告不同；改內容時換一個 id 才會再顯示給按過「知道了」的人
      "level": "warn",                            // "info"（綠色）或 "warn"（琥珀色）
      "title": { "zh-Hant": "即時同步維護中", "en": "Live sync maintenance" },
      "body": { "zh-Hant": "今晚 11 點到 12 點暫停，歷史備份不受影響。", "en": "Paused tonight 11 pm–12 am. History backup is not affected." },
      "url": "https://www.copyc.app.dreamsworld.top/",  // 選填，只接受 https；會出現「了解更多」
      "until": 1793900000000,                     // 選填：到期時間（毫秒）
      "platforms": ["windows"],                   // 選填："windows"、"macos"；不填 = 全部
      "channels": ["web"],                        // 選填："web"（官網版）、"store"（商店版）；不填 = 全部
      "minVersion": "1.1.1",                      // 選填：只給這個版本以上
      "maxVersion": "1.1.9"                       // 選填：只給這個版本以下
    }
  ],

  // 額外當作密碼管理器的 App（遮蔽內容、不寫進歷史），比對 App 名稱或執行檔名，不分大小寫
  "blacklist": ["Proton Pass", "Enpass"],

  // 修正介面文字或翻譯：語言 → 文字鍵 → 新文字。文字鍵見 src/shared/i18n.js 與 src/shared/locales/
  // 只能取代現有的文字，{0} 這類位置要跟原文一樣，否則會被忽略
  "strings": {
    "en": { "sync.lanDesc": "Computers on the same network get your copy directly." }
  },

  // 緊急開關：false 會暫時關掉所有用戶的這項功能（例如中繼伺服器出問題）；恢復時改回 true
  "features": { "live": true, "lan": true, "backup": true }
}
```

公告的語言代碼：`zh-Hant`、`zh-Hans`、`en`、`ja`、`ko`、`fr`、`ms`；沒有用戶目前的語言時顯示 `en`，再沒有就顯示第一個。

注意：`remote.json` 本身是 JSON，不能寫上面範例裡的 `//` 註解。

## 生效時間

- 推上 GitHub 後約 1～2 分鐘官網更新。
- App 開啟時會下載一次，之後每 6 小時一次；離線時沿用上次下載的內容。
