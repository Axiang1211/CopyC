package top.dreamsworld.copyc

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.LocaleList
import android.os.Looper
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

/**
 * 給 Rust 呼叫的 Android 功能（見 src/android.rs）。用 Application context，
 * 所以只剩背景服務、沒有畫面時也能寫入剪貼簿與顯示通知。
 */
object CopyCBridge {
  private const val CHANNEL_SYNC = "sync"
  private const val CHANNEL_RECEIVED = "received"
  private const val NOTIFY_RECEIVED = 2
  private val main = Handler(Looper.getMainLooper())
  private var multicastLock: WifiManager.MulticastLock? = null

  @JvmStatic
  fun setClipboard(context: Context, text: String) {
    main.post {
      context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(ClipData.newPlainText("CopyC", text))
    }
  }

  /** App 不在前景時收到電腦傳來的內容：通知一下（鎖定畫面上不顯示內容）。 */
  @JvmStatic
  fun notifyReceived(context: Context, device: String, preview: String) {
    if (!canNotify(context)) return
    ensureChannels(context)
    val title = text(context).getString(R.string.received_title, device)
    val public = NotificationCompat.Builder(context, CHANNEL_RECEIVED)
      .setSmallIcon(R.drawable.ic_notification)
      .setContentTitle(title)
      .build()
    val notification = NotificationCompat.Builder(context, CHANNEL_RECEIVED)
      .setSmallIcon(R.drawable.ic_notification)
      .setContentTitle(title)
      .setContentText(preview)
      .setContentIntent(openApp(context))
      .setAutoCancel(true)
      .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
      .setPublicVersion(public)
      .build()
    context.getSystemService(NotificationManager::class.java)?.notify(NOTIFY_RECEIVED, notification)
  }

  /** 背景接收：用前景服務讓 App 在背景時保持連線。 */
  @JvmStatic
  fun setBackground(context: Context, on: Boolean) {
    val intent = Intent(context, SyncService::class.java)
    try {
      if (on) ContextCompat.startForegroundService(context, intent) else context.stopService(intent)
    } catch (e: Exception) {
      // Android 12 起只有 App 在前景時才能啟動前景服務；下次打開 App 時會再啟動
    }
  }

  /** 區域網路直傳：手機預設會丟掉廣播封包，要拿著 MulticastLock 才收得到其他電腦的信標。 */
  @JvmStatic
  fun setMulticast(context: Context, on: Boolean) {
    if (on) {
      if (multicastLock == null) {
        val wifi = context.applicationContext.getSystemService(WifiManager::class.java) ?: return
        multicastLock = wifi.createMulticastLock("copyc-lan").apply {
          setReferenceCounted(false)
          acquire()
        }
      }
    } else {
      multicastLock?.release()
      multicastLock = null
    }
  }

  @JvmStatic
  fun deviceName(): String = Build.MODEL ?: "Android"

  fun syncNotification(context: Context) = NotificationCompat.Builder(context, CHANNEL_SYNC)
    .setSmallIcon(R.drawable.ic_notification)
    .setContentTitle(text(context).getString(R.string.sync_title))
    .setContentText(text(context).getString(R.string.sync_text))
    .setContentIntent(openApp(context))
    .setOngoing(true)
    .setPriority(NotificationCompat.PRIORITY_LOW)
    .build()
    .also { ensureChannels(context) }

  fun ensureChannels(context: Context) {
    val nm = context.getSystemService(NotificationManager::class.java) ?: return
    nm.createNotificationChannel(
      NotificationChannel(CHANNEL_SYNC, text(context).getString(R.string.sync_channel), NotificationManager.IMPORTANCE_LOW)
    )
    nm.createNotificationChannel(
      NotificationChannel(CHANNEL_RECEIVED, text(context).getString(R.string.received_channel), NotificationManager.IMPORTANCE_DEFAULT)
    )
  }

  /**
   * 通知文字用 App 裡選的語言。Android 13 起系統會自己套用；較舊的系統只套用在畫面上，
   * 所以通知要自己換成那個語言。
   */
  private fun text(context: Context): Context {
    val locales = AppCompatDelegate.getApplicationLocales()
    if (Build.VERSION.SDK_INT >= 33 || locales.isEmpty) return context
    val config = Configuration(context.resources.configuration)
    config.setLocales(LocaleList.forLanguageTags(locales.toLanguageTags()))
    return context.createConfigurationContext(config)
  }

  private fun canNotify(context: Context) =
    Build.VERSION.SDK_INT < 33 ||
      ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

  private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
    context,
    0,
    Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
  )
}
