package top.dreamsworld.copyc

import android.Manifest
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.core.os.LocaleListCompat

class MainActivity : TauriActivity() {
  companion object {
    /** 快速設定按鈕：打開 App 並把剪貼簿傳到電腦 */
    const val ACTION_SEND_CLIPBOARD = "top.dreamsworld.copyc.SEND_CLIPBOARD"
    /** Android 17 起，連區域網路裡的其他裝置要這個權限（較舊的系統沒有這個權限，也不需要） */
    private const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
  }

  // 從其他 App「分享 → CopyC」傳來、還沒交給畫面的文字
  @Volatile private var sharedText: String? = null
  // 從快速設定按鈕打開時要做的事
  @Volatile private var pendingAction: String? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    keepIntent(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    keepIntent(intent)
  }

  private fun keepIntent(intent: Intent?) {
    when (intent?.action) {
      Intent.ACTION_SEND -> if (intent.type?.startsWith("text/") == true) {
        intent.getStringExtra(Intent.EXTRA_TEXT)?.takeIf { it.isNotBlank() }?.let { sharedText = it }
      }
      ACTION_SEND_CLIPBOARD -> pendingAction = "sendClipboard"
    }
  }

  private fun granted(permission: String) =
    ContextCompat.checkSelfPermission(this, permission) == PackageManager.PERMISSION_GRANTED

  // 畫面（app.js）透過 window.CopyCAndroid 呼叫；網頁載入前就加好
  override fun onWebViewCreate(webView: WebView) {
    webView.addJavascriptInterface(object {
      /**
       * 讀剪貼簿的文字。從瀏覽器、Email 等 App 複製的通常是帶格式的文字（HTML），
       * 不是純文字，所以用 coerceToText 轉成文字（Android 只讓前景、有焦點的 App 讀）。
       */
      @JavascriptInterface
      fun readClipboard(): String? = try {
        val cm = getSystemService(ClipboardManager::class.java)
        val clip = cm?.primaryClip
        if (clip == null || clip.itemCount == 0) null
        else clip.getItemAt(0).coerceToText(this@MainActivity)?.toString()?.takeIf { it.isNotBlank() }
      } catch (e: Exception) {
        null
      }

      /** 剪貼簿內容被來源 App 標成敏感（例如密碼管理器，Android 13 起）。 */
      @JavascriptInterface
      fun clipboardSensitive(): Boolean = try {
        Build.VERSION.SDK_INT >= 33 &&
          getSystemService(ClipboardManager::class.java)?.primaryClipDescription?.extras
            ?.getBoolean(ClipDescription.EXTRA_IS_SENSITIVE) == true
      } catch (e: Exception) {
        false
      }

      @JavascriptInterface
      fun takeShared(): String? {
        val text = sharedText
        sharedText = null
        return text
      }

      @JavascriptInterface
      fun takeAction(): String? {
        val action = pendingAction
        pendingAction = null
        return action
      }

      /** 背景接收的通知需要的權限（Android 13 起） */
      @JavascriptInterface
      fun requestNotifications() {
        if (Build.VERSION.SDK_INT >= 33 && !granted(Manifest.permission.POST_NOTIFICATIONS)) {
          runOnUiThread { ActivityCompat.requestPermissions(this@MainActivity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1) }
        }
      }

      /** App 的語言（BCP 47，例如 zh-TW）；空字串表示跟隨系統。也可以在系統設定 → App → CopyC → 語言更改（Android 13 起）。 */
      @JavascriptInterface
      fun appLanguage(): String = AppCompatDelegate.getApplicationLocales().toLanguageTags()

      /** 改 App 的語言（空字串＝跟隨系統）。通知與快速設定按鈕的文字也會跟著換；畫面由 app.js 重新載入。 */
      @JavascriptInterface
      fun setLanguage(tag: String) {
        runOnUiThread { AppCompatDelegate.setApplicationLocales(LocaleListCompat.forLanguageTags(tag)) }
      }

      @JavascriptInterface
      fun hasLocalNetwork(): Boolean = Build.VERSION.SDK_INT < 37 || granted(LOCAL_NETWORK)

      /** 區域網路直傳需要的權限（Android 17 起） */
      @JavascriptInterface
      fun requestLocalNetwork() {
        if (Build.VERSION.SDK_INT >= 37 && !granted(LOCAL_NETWORK)) {
          runOnUiThread { ActivityCompat.requestPermissions(this@MainActivity, arrayOf(LOCAL_NETWORK), 2) }
        }
      }
    }, "CopyCAndroid")
  }
}
