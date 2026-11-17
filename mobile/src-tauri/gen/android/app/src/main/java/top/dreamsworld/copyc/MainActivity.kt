package top.dreamsworld.copyc

import android.Manifest
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.OpenableColumns
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.core.os.LocaleListCompat
import java.io.File
import java.util.concurrent.Executors
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : TauriActivity() {
  companion object {
    /** 快速設定按鈕：打開 App 並把剪貼簿傳到電腦 */
    const val ACTION_SEND_CLIPBOARD = "top.dreamsworld.copyc.SEND_CLIPBOARD"
    /** 通知「有檔案經由 Google Drive 傳來」：打開 App 的「檔案」 */
    const val ACTION_OPEN_FILES = "top.dreamsworld.copyc.OPEN_FILES"
    private const val PICK_FILES = 7
    /** Android 17 起，連區域網路裡的其他裝置要這個權限（較舊的系統沒有這個權限，也不需要） */
    private const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
  }

  // 從其他 App「分享 → CopyC」傳來、還沒交給畫面的文字
  @Volatile private var sharedText: String? = null
  // 從快速設定按鈕打開時要做的事
  @Volatile private var pendingAction: String? = null
  // 從分享選單或「選擇檔案」來的檔案：複製到 App 的暫存資料夾後交給畫面（JSON：[{path, name, size}]）
  @Volatile private var sharedFiles: String? = null
  @Volatile private var preparing = false
  private var webView: WebView? = null
  private val io = Executors.newSingleThreadExecutor()

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
      Intent.ACTION_SEND -> {
        val streams = streamsOf(intent)
        if (streams.isNotEmpty()) {
          prepareFiles(streams)
        } else if (intent.type?.startsWith("text/") == true) {
          intent.getStringExtra(Intent.EXTRA_TEXT)?.takeIf { it.isNotBlank() }?.let { sharedText = it }
        }
      }
      Intent.ACTION_SEND_MULTIPLE -> prepareFiles(streamsOf(intent))
      ACTION_SEND_CLIPBOARD -> pendingAction = "sendClipboard"
      ACTION_OPEN_FILES -> pendingAction = "openFiles"
    }
  }

  /** 分享選單帶來的檔案（一個或多個）。 */
  @Suppress("DEPRECATION")
  private fun streamsOf(intent: Intent): List<Uri> {
    val out = mutableListOf<Uri>()
    if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
      intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)?.let { out.addAll(it) }
    } else {
      intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)?.let { out.add(it) }
    }
    if (out.isEmpty()) {
      intent.clipData?.let { clip -> for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let { out.add(it) } }
    }
    return out
  }

  private fun displayName(uri: Uri): String? = try {
    contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
      if (c.moveToFirst()) c.getString(0) else null
    }
  } catch (e: Exception) {
    null
  } ?: uri.lastPathSegment?.substringAfterLast('/')

  /**
   * 把檔案複製到 App 的暫存資料夾（傳送時要能重讀、接著傳），完成後通知畫面。
   * 傳完 Rust 端會刪掉複本。
   */
  private fun prepareFiles(uris: List<Uri>) {
    if (uris.isEmpty()) return
    preparing = true
    io.execute {
      val out = JSONArray()
      val dir = File(cacheDir, "outbox/" + System.currentTimeMillis()).apply { mkdirs() }
      for (uri in uris) {
        try {
          val name = (displayName(uri) ?: "file").replace('/', '_').ifBlank { "file" }
          val dest = CopyCBridge.uniqueFile(dir, name)
          contentResolver.openInputStream(uri)?.use { input -> dest.outputStream().use { input.copyTo(it, 1 shl 20) } }
          if (dest.exists()) out.put(JSONObject().put("path", dest.absolutePath).put("name", dest.name).put("size", dest.length()))
        } catch (e: Exception) {
          // 讀不到的檔案略過
        }
      }
      sharedFiles = out.toString()
      preparing = false
      runOnUiThread { webView?.evaluateJavascript("window.copycFilesReady && window.copycFilesReady()", null) }
    }
  }

  @Deprecated("Deprecated in Java")
  @Suppress("DEPRECATION")
  override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
    super.onActivityResult(requestCode, resultCode, data)
    if (requestCode != PICK_FILES || resultCode != RESULT_OK || data == null) return
    val uris = mutableListOf<Uri>()
    val clip = data.clipData
    if (clip != null) {
      for (i in 0 until clip.itemCount) uris.add(clip.getItemAt(i).uri)
    } else {
      data.data?.let { uris.add(it) }
    }
    prepareFiles(uris)
  }

  private fun granted(permission: String) =
    ContextCompat.checkSelfPermission(this, permission) == PackageManager.PERMISSION_GRANTED

  // 畫面（app.js）透過 window.CopyCAndroid 呼叫；網頁載入前就加好
  override fun onWebViewCreate(webView: WebView) {
    this.webView = webView
    webView.addJavascriptInterface(object {
      /** 準備好、還沒交給畫面的檔案（JSON），沒有就是 null。 */
      @JavascriptInterface
      fun takeSharedFiles(): String? {
        val files = sharedFiles
        sharedFiles = null
        return files
      }

      /** 正在把分享來的檔案複製到暫存資料夾。 */
      @JavascriptInterface
      fun isPreparing(): Boolean = preparing

      /** 「選擇檔案」：打開系統的檔案選擇畫面（可以多選）。 */
      @Suppress("DEPRECATION")
      @JavascriptInterface
      fun pickFiles() {
        runOnUiThread {
          val intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType("*/*")
            .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
          try {
            startActivityForResult(intent, PICK_FILES)
          } catch (e: Exception) {
            // 沒有檔案選擇器
          }
        }
      }

      /** 打開收到的檔案（content:// URI）。 */
      @JavascriptInterface
      fun openUri(uri: String) {
        runOnUiThread {
          try {
            val view = Intent(Intent.ACTION_VIEW)
              .setDataAndType(Uri.parse(uri), contentResolver.getType(Uri.parse(uri)) ?: "*/*")
              .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            startActivity(Intent.createChooser(view, null))
          } catch (e: Exception) {
            // 沒有可以打開的 App
          }
        }
      }

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
