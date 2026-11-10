package top.dreamsworld.copyc

import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * 背景接收（使用者在 App 裡開啟才會啟動）：常駐通知讓系統不要結束 CopyC，
 * 即時同步的連線（在 Rust 那邊）就能在 App 不在前景時繼續收到電腦傳來的內容。
 */
class SyncService : Service() {
  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    CopyCBridge.ensureChannels(this)
    val notification = CopyCBridge.syncNotification(this)
    if (Build.VERSION.SDK_INT >= 34) {
      startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
    } else {
      startForeground(1, notification)
    }
    // 系統結束 App 後不要自己重啟服務：同步程式在 App 裡，下次打開 App 時會再啟動
    return START_NOT_STICKY
  }
}
