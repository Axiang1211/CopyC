package top.dreamsworld.copyc

import android.content.Intent
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  // 從其他 App「分享 → CopyC」傳來、還沒交給畫面的文字
  @Volatile private var sharedText: String? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    keepShared(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    keepShared(intent)
  }

  private fun keepShared(intent: Intent?) {
    if (intent?.action == Intent.ACTION_SEND && intent.type?.startsWith("text/") == true) {
      intent.getStringExtra(Intent.EXTRA_TEXT)?.takeIf { it.isNotBlank() }?.let { sharedText = it }
    }
  }

  // 畫面（app.js）用 window.CopyCAndroid.takeShared() 取走分享的文字；網頁載入前就加好
  override fun onWebViewCreate(webView: WebView) {
    webView.addJavascriptInterface(object {
      @JavascriptInterface
      fun takeShared(): String? {
        val text = sharedText
        sharedText = null
        return text
      }
    }, "CopyCAndroid")
  }
}
