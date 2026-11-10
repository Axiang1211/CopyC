package top.dreamsworld.copyc

import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.TileService

/**
 * 快速設定的「傳到電腦」按鈕：打開 CopyC 並把目前的剪貼簿傳到電腦
 * （Android 只允許前景的 App 讀剪貼簿，所以要先打開 App）。
 */
class SendTileService : TileService() {
  override fun onClick() {
    super.onClick()
    val intent = Intent(this, MainActivity::class.java)
      .setAction(MainActivity.ACTION_SEND_CLIPBOARD)
      .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    if (Build.VERSION.SDK_INT >= 34) {
      startActivityAndCollapse(
        PendingIntent.getActivity(this, 1, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
      )
    } else {
      @Suppress("DEPRECATION", "StartActivityAndCollapseDeprecated")
      startActivityAndCollapse(intent)
    }
  }
}
