package top.dreamsworld.copyc

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothStatusCodes
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import androidx.core.content.ContextCompat
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * 藍牙低功耗近距離互傳（手機當周邊裝置）。協定在 Rust（src-tauri/src/cloud/ble_proto.rs），這裡只負責藍牙本身：
 * - 廣播：廠商資料放帳號暗號（Rust 每 10 分鐘換一次），掃描回應放 CopyC 的服務 UUID；
 * - GATT 服務：電腦寫入 RX、手機用 TX 通知送出；
 * - 收到的每一塊原封不動交給 Rust（[onBleEvent]），要送的每一塊由 Rust 交過來（[notify]），這裡排隊依序送出。
 * 不需要配對；內容都已經用帳號金鑰加密。
 */
@SuppressLint("MissingPermission") // 每個入口都先檢查過權限（hasPermission）
object BleServer {
  private val SERVICE: UUID = UUID.fromString("6f1c0001-5a3e-4c1b-9d2f-8b7a4e6c0b11")
  private val RX: UUID = UUID.fromString("6f1c0002-5a3e-4c1b-9d2f-8b7a4e6c0b11")
  private val TX: UUID = UUID.fromString("6f1c0003-5a3e-4c1b-9d2f-8b7a4e6c0b11")
  private val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
  private const val COMPANY = 0xFFFF

  // 傳給 Rust 的事件種類（與 mobile/src-tauri/src/ble.rs 一致）
  private const val CONNECTED = 1
  private const val SUBSCRIBED = 2
  private const val DISCONNECTED = 3
  private const val DATA = 4
  private const val DRAINED = 5
  private const val MTU = 6

  @JvmStatic external fun onBleEvent(kind: Int, address: String, value: Int, data: ByteArray?)

  private var server: BluetoothGattServer? = null
  private var tx: BluetoothGattCharacteristic? = null
  private var advertiser: BluetoothLeAdvertiser? = null
  private var advertising: AdvertiseCallback? = null
  private val devices = ConcurrentHashMap<String, BluetoothDevice>()
  private val mtus = ConcurrentHashMap<String, Int>()
  private val queues = ConcurrentHashMap<String, ArrayDeque<ByteArray>>()
  private val busy = ConcurrentHashMap<String, Boolean>()
  private val main = Handler(Looper.getMainLooper())

  fun permissions(): Array<String> =
    if (Build.VERSION.SDK_INT >= 31) arrayOf(Manifest.permission.BLUETOOTH_ADVERTISE, Manifest.permission.BLUETOOTH_CONNECT)
    else emptyArray()

  fun hasPermission(context: Context): Boolean =
    permissions().all { ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED }

  private fun safeEvent(kind: Int, address: String, value: Int, data: ByteArray?) {
    // Rust 的函式庫還沒載入時（理論上不會）不要讓藍牙執行緒當掉
    runCatching { onBleEvent(kind, address, value, data) }
  }

  /** 開始（或用新的暗號重新開始）廣播；沒有權限、沒有藍牙或藍牙關著時回傳 false。 */
  @Synchronized
  fun start(context: Context, token: ByteArray): Boolean {
    if (!hasPermission(context)) return false
    val manager = context.getSystemService(BluetoothManager::class.java) ?: return false
    val adapter = manager.adapter ?: return false
    if (!adapter.isEnabled) return false
    if (server == null) {
      val s = manager.openGattServer(context.applicationContext, callback) ?: return false
      val service = BluetoothGattService(SERVICE, BluetoothGattService.SERVICE_TYPE_PRIMARY)
      val rx = BluetoothGattCharacteristic(RX, BluetoothGattCharacteristic.PROPERTY_WRITE, BluetoothGattCharacteristic.PERMISSION_WRITE)
      val t = BluetoothGattCharacteristic(TX, BluetoothGattCharacteristic.PROPERTY_NOTIFY, 0)
      t.addDescriptor(BluetoothGattDescriptor(CCCD, BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE))
      service.addCharacteristic(rx)
      service.addCharacteristic(t)
      if (!s.addService(service)) {
        s.close()
        return false
      }
      server = s
      tx = t
    }
    val adv = adapter.bluetoothLeAdvertiser ?: return false
    advertising?.let { runCatching { adv.stopAdvertising(it) } }
    val settings = AdvertiseSettings.Builder()
      .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
      .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
      .setConnectable(true)
      .setTimeout(0)
      .build()
    // 廣播本身放暗號（電腦不用服務過濾就看得到），掃描回應放服務 UUID；都不放裝置名稱
    val data = AdvertiseData.Builder().setIncludeDeviceName(false).setIncludeTxPowerLevel(false).addManufacturerData(COMPANY, token).build()
    val scan = AdvertiseData.Builder().setIncludeDeviceName(false).addServiceUuid(ParcelUuid(SERVICE)).build()
    val cb = object : AdvertiseCallback() {}
    advertiser = adv
    advertising = cb
    return runCatching { adv.startAdvertising(settings, data, scan, cb) }.isSuccess
  }

  @Synchronized
  fun stop() {
    advertising?.let { cb -> runCatching { advertiser?.stopAdvertising(cb) } }
    advertising = null
    devices.values.forEach { d -> runCatching { server?.cancelConnection(d) } }
    runCatching { server?.close() }
    server = null
    tx = null
    devices.clear()
    mtus.clear()
    queues.clear()
    busy.clear()
  }

  /** Rust 交來要送給 `address` 的一塊；依序送，一次一塊（送完 onNotificationSent 再送下一塊）。 */
  fun notify(address: String, frame: ByteArray) {
    val q = queues.getOrPut(address) { ArrayDeque() }
    synchronized(q) { q.addLast(frame) }
    pump(address)
  }

  private fun pump(address: String) {
    val q = queues[address] ?: return
    val frame: ByteArray
    synchronized(q) {
      if (busy[address] == true) return
      frame = q.removeFirstOrNull() ?: return
      busy[address] = true
    }
    val device = devices[address]
    val s = server
    val c = tx
    if (device == null || s == null || c == null) {
      synchronized(q) { q.clear() }
      busy[address] = false
      return
    }
    val ok = runCatching {
      if (Build.VERSION.SDK_INT >= 33) {
        s.notifyCharacteristicChanged(device, c, false, frame) == BluetoothStatusCodes.SUCCESS
      } else {
        @Suppress("DEPRECATION")
        c.value = frame
        @Suppress("DEPRECATION")
        s.notifyCharacteristicChanged(device, c, false)
      }
    }.getOrDefault(false)
    if (!ok) {
      // 藍牙堆疊忙碌：放回最前面，稍後再試
      synchronized(q) { q.addFirst(frame) }
      busy[address] = false
      main.postDelayed({ pump(address) }, 30)
    }
  }

  private val callback = object : BluetoothGattServerCallback() {
    override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
      val address = device.address
      if (newState == BluetoothProfile.STATE_CONNECTED) {
        devices[address] = device
        mtus[address] = 23
        safeEvent(CONNECTED, address, 0, null)
      } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
        devices.remove(address)
        mtus.remove(address)
        queues.remove(address)
        busy.remove(address)
        safeEvent(DISCONNECTED, address, 0, null)
      }
    }

    override fun onMtuChanged(device: BluetoothDevice, mtu: Int) {
      mtus[device.address] = mtu
      safeEvent(MTU, device.address, mtu, null)
    }

    override fun onDescriptorWriteRequest(
      device: BluetoothDevice, requestId: Int, descriptor: BluetoothGattDescriptor,
      preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray?,
    ) {
      if (responseNeeded) server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null)
      if (descriptor.uuid == CCCD && value != null && value.contentEquals(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)) {
        safeEvent(SUBSCRIBED, device.address, mtus[device.address] ?: 23, null)
      }
    }

    override fun onCharacteristicWriteRequest(
      device: BluetoothDevice, requestId: Int, characteristic: BluetoothGattCharacteristic,
      preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray?,
    ) {
      if (responseNeeded) server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null)
      if (characteristic.uuid == RX && value != null) safeEvent(DATA, device.address, 0, value)
    }

    override fun onNotificationSent(device: BluetoothDevice, status: Int) {
      val address = device.address
      busy[address] = false
      val empty = queues[address]?.let { q -> synchronized(q) { q.isEmpty() } } ?: true
      if (empty) safeEvent(DRAINED, address, 0, null) else pump(address)
    }
  }
}
