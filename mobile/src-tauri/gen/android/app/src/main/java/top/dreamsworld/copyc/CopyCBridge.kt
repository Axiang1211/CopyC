package top.dreamsworld.copyc

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import android.os.Handler
import android.os.LocaleList
import android.os.Looper
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import java.io.File

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
        // 沒有 Wi-Fi 的裝置沒有 WifiManager：只是收不到同一個網路的信標，其他功能照常
        val wifi = context.applicationContext.getSystemService(WifiManager::class.java) ?: return
        multicastLock = runCatching {
          wifi.createMulticastLock("copyc-lan").apply {
            setReferenceCounted(false)
            acquire()
          }
        }.getOrNull()
      }
    } else {
      multicastLock?.release()
      multicastLock = null
    }
  }

  /**
   * 把收到的檔案（App 暫存資料夾裡）存到「下載/CopyC」，圖片存到「圖片/CopyC」，回傳 content:// URI。
   * Android 9 以前沒有 MediaStore 的下載集合：存到 App 自己的「下載」資料夾（不需要儲存權限）。
   */
  @JvmStatic
  fun saveReceived(context: Context, path: String, name: String): String? = try {
    val src = File(path)
    val mime = mimeOf(name)
    if (Build.VERSION.SDK_INT >= 29) {
      val image = mime.startsWith("image/")
      val collection = if (image) MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        else MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
      val folder = if (image) Environment.DIRECTORY_PICTURES else Environment.DIRECTORY_DOWNLOADS
      val values = ContentValues().apply {
        put(MediaStore.MediaColumns.DISPLAY_NAME, name)
        put(MediaStore.MediaColumns.MIME_TYPE, mime)
        put(MediaStore.MediaColumns.RELATIVE_PATH, "$folder/CopyC")
        put(MediaStore.MediaColumns.IS_PENDING, 1)
      }
      val resolver = context.contentResolver
      val uri = resolver.insert(collection, values)
      if (uri == null) null else {
        resolver.openOutputStream(uri)?.use { out -> src.inputStream().use { it.copyTo(out, 1 shl 20) } }
        values.clear()
        values.put(MediaStore.MediaColumns.IS_PENDING, 0)
        resolver.update(uri, values, null, null)
        uri.toString()
      }
    } else {
      val dir = File(context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS), "CopyC").apply { mkdirs() }
      val dest = uniqueFile(dir, name)
      src.copyTo(dest)
      FileProvider.getUriForFile(context, context.packageName + ".fileprovider", dest).toString()
    }
  } catch (e: Exception) {
    null
  }

  /** 收到檔案：通知一下，點一下用其他 App 打開。 */
  @JvmStatic
  fun notifyFile(context: Context, device: String, name: String, uri: String) {
    if (!canNotify(context)) return
    ensureChannels(context)
    val open = Intent(Intent.ACTION_VIEW)
      .setDataAndType(Uri.parse(uri), mimeOf(name))
      .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
    val pending = PendingIntent.getActivity(
      context, uri.hashCode(), Intent.createChooser(open, name).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val notification = NotificationCompat.Builder(context, CHANNEL_RECEIVED)
      .setSmallIcon(R.drawable.ic_notification)
      .setContentTitle(text(context).getString(R.string.file_title, device))
      .setContentText(name)
      .setContentIntent(pending)
      .setAutoCancel(true)
      .build()
    context.getSystemService(NotificationManager::class.java)?.notify(uri.hashCode(), notification)
  }

  /** 有檔案經由 Google Drive 傳來：通知一下，點一下打開 App 的「檔案」下載（24 小時內）。 */
  @JvmStatic
  fun notifyIncoming(context: Context, device: String, name: String) {
    if (!canNotify(context)) return
    ensureChannels(context)
    val intent = Intent(context, MainActivity::class.java)
      .setAction(MainActivity.ACTION_OPEN_FILES)
      .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    val pending = PendingIntent.getActivity(context, 3, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    val notification = NotificationCompat.Builder(context, CHANNEL_RECEIVED)
      .setSmallIcon(R.drawable.ic_notification)
      .setContentTitle(text(context).getString(R.string.file_title, device))
      .setContentText(text(context).getString(R.string.file_incoming, name))
      .setContentIntent(pending)
      .setAutoCancel(true)
      .build()
    context.getSystemService(NotificationManager::class.java)?.notify(name.hashCode() xor device.hashCode(), notification)
  }

  fun mimeOf(name: String): String =
    MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase())
      ?: "application/octet-stream"

  /** 資料夾裡不和現有檔案撞名的檔案：報告.pdf → 報告 (1).pdf */
  fun uniqueFile(dir: File, name: String): File {
    val first = File(dir, name)
    if (!first.exists()) return first
    val stem = name.substringBeforeLast('.', name)
    val ext = if (name.contains('.')) "." + name.substringAfterLast('.') else ""
    var n = 1
    while (true) {
      val f = File(dir, "$stem ($n)$ext")
      if (!f.exists()) return f
      n++
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
