//! 藍牙低功耗近距離互傳（Android 版，手機當周邊裝置）。協定與電腦版共用（cloud::ble_proto）。
//!
//! 藍牙本身在 Kotlin（BleServer.kt）：廣播、GATT 服務、依序送出。這裡負責其他的事：
//! - 每 10 分鐘換一次廣播裡的帳號暗號；
//! - 把電腦寫來的小塊拼回訊息、解開、處理（打招呼、即時同步的文字、小檔案）；
//! - 把要送給電腦的訊息加密、切塊交給 Kotlin。
//!
//! 使用者在設定裡開啟「藍牙互傳」（並允許「附近的裝置」權限）才會廣播。

use crate::{
    android,
    cloud::{
        ble_proto::{self, Msg, Reassembly, CHUNK},
        crypto::Key,
        Cloud,
    },
    files,
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::Path,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc, oneshot};

// Kotlin 傳來的事件種類（與 BleServer.kt 一致）
const CONNECTED: i32 = 1;
const SUBSCRIBED: i32 = 2;
const DISCONNECTED: i32 = 3;
const DATA: i32 = 4;
const DRAINED: i32 = 5;
const MTU: i32 = 6;

/// 多久檢查一次開關與暗號。
const TICK: Duration = Duration::from_secs(5);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct Event {
    kind: i32,
    address: String,
    value: i32,
    data: Vec<u8>,
}

static EVENTS: OnceLock<mpsc::UnboundedSender<Event>> = OnceLock::new();

/// Kotlin 的藍牙回呼（BleServer.onBleEvent）。在藍牙執行緒上呼叫：只排進佇列，不在這裡做事。
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_top_dreamsworld_copyc_BleServer_onBleEvent(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    kind: jni::sys::jint,
    address: jni::objects::JString,
    value: jni::sys::jint,
    data: jni::objects::JByteArray,
) {
    let address: String = env.get_string(&address).map(Into::into).unwrap_or_default();
    let data = if data.is_null() { Vec::new() } else { env.convert_byte_array(&data).unwrap_or_default() };
    if let Some(tx) = EVENTS.get() {
        let _ = tx.send(Event { kind, address, value, data });
    }
}

/// 一台用藍牙連著的電腦。
struct Link {
    /// 打過招呼後才知道是哪台電腦
    id: Option<String>,
    name: String,
    mtu: usize,
    parts: Reassembly,
    /// 等「全部送完」的傳檔
    drained: Vec<oneshot::Sender<()>>,
}

#[derive(Default)]
pub struct Ble {
    links: Mutex<HashMap<String, Link>>,
}

impl Ble {
    /// 打過招呼的電腦：(裝置 ID, 名稱)。
    pub fn peers(&self) -> Vec<(String, String)> {
        self.links.lock().values().filter_map(|l| Some((l.id.clone()?, l.name.clone()))).collect()
    }

    fn send(&self, address: &str, key: &Key, msg: &Msg) {
        let mtu = self.links.lock().get(address).map_or(23, |l| l.mtu);
        let data = ble_proto::seal_msg(key, msg, now_ms());
        // 通知的內容上限是 MTU 減 3
        let chunk = CHUNK.min(mtu.saturating_sub(3)).max(ble_proto::HEADER + 1);
        for frame in ble_proto::frames(rand::random(), &data, chunk) {
            android::ble_notify(address, &frame);
        }
    }

    /// 把一則即時同步送給連著、但不在 `skip`（已經從區域網路收到）裡的電腦；回傳送給了哪些裝置。
    pub fn send_clip(&self, key: &Key, wire: &str, skip: &[String]) -> Vec<String> {
        let targets: Vec<(String, String)> = self
            .links
            .lock()
            .iter()
            .filter_map(|(addr, l)| l.id.clone().filter(|id| !skip.contains(id)).map(|id| (addr.clone(), id)))
            .collect();
        let msg = Msg::Clip { wire: wire.to_string() };
        for (addr, _) in &targets {
            self.send(addr, key, &msg);
        }
        targets.into_iter().map(|(_, id)| id).collect()
    }
}

/// 藍牙能傳的檔案大小。
pub fn small_enough(size: u64) -> bool {
    size <= ble_proto::MAX_FILE
}

/// 從藍牙傳一個小檔案給 `peer`（裝置 ID），等到全部送出為止。
pub async fn send_file(app: &AppHandle, peer: &str, path: &Path, name: &str) -> Result<(), &'static str> {
    let cloud = app.state::<Cloud>();
    let ble = app.state::<Ble>();
    let key = cloud.key.lock().clone().ok_or("offline")?;
    let bytes = tokio::fs::read(path).await.map_err(|_| "disk")?;
    let address = ble.links.lock().iter().find(|(_, l)| l.id.as_deref() == Some(peer)).map(|(a, _)| a.clone()).ok_or("offline")?;
    let (tx, rx) = oneshot::channel();
    if let Some(link) = ble.links.lock().get_mut(&address) {
        link.drained.push(tx);
    }
    ble.send(&address, &key, &ble_proto::file_msg(name, &bytes));
    // 藍牙大約每秒 20 KB 起跳：給足時間，斷線時 drained 會被丟掉而提早結束
    let wait = Duration::from_secs(30 + bytes.len() as u64 / 5_000);
    match tokio::time::timeout(wait, rx).await {
        Ok(Ok(())) => Ok(()),
        _ => Err("offline"),
    }
}

fn enabled(cloud: &Cloud) -> bool {
    cloud.signed_in() && cloud.prefs().ble
}

pub async fn run(app: AppHandle) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _ = EVENTS.set(tx);
    let cloud = app.state::<Cloud>();
    let ble = app.state::<Ble>();
    // 目前廣播用的暗號時段；None 表示沒在廣播
    let mut window: Option<u64> = None;
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let key = cloud.key.lock().clone();
                match key.filter(|_| enabled(&cloud)) {
                    Some(key) => {
                        let now = now_ms();
                        let w = now / ble_proto::WINDOW_MS;
                        if window != Some(w) {
                            window = android::ble_start(&ble_proto::token(&key, now)).then_some(w);
                        }
                    }
                    None if window.is_some() => {
                        android::ble_stop();
                        ble.links.lock().clear();
                        window = None;
                        files::ble_changed(&app);
                    }
                    None => {}
                }
            }
            Some(ev) = rx.recv() => handle(&app, &cloud, &ble, ev).await,
        }
    }
}

async fn handle(app: &AppHandle, cloud: &Cloud, ble: &Ble, ev: Event) {
    match ev.kind {
        CONNECTED => {
            ble.links.lock().insert(
                ev.address,
                Link { id: None, name: String::new(), mtu: 23, parts: Reassembly::default(), drained: Vec::new() },
            );
        }
        MTU => {
            if let Some(l) = ble.links.lock().get_mut(&ev.address) {
                l.mtu = ev.value.max(23) as usize;
            }
        }
        SUBSCRIBED => {
            if let Some(l) = ble.links.lock().get_mut(&ev.address) {
                l.mtu = ev.value.max(23) as usize;
            }
            // 電腦開始聽了：先自我介紹
            if let Some(key) = cloud.key.lock().clone() {
                let hello = Msg::Hello { id: cloud.device_id.clone(), name: cloud.device_name.clone() };
                ble.send(&ev.address, &key, &hello);
            }
        }
        DISCONNECTED => {
            let had = ble.links.lock().remove(&ev.address).is_some_and(|l| l.id.is_some());
            if had {
                files::ble_changed(app);
            }
        }
        DRAINED => {
            if let Some(l) = ble.links.lock().get_mut(&ev.address) {
                for done in l.drained.drain(..) {
                    let _ = done.send(());
                }
            }
        }
        DATA => {
            let sealed = ble.links.lock().get_mut(&ev.address).and_then(|l| l.parts.push(&ev.data));
            let (Some(sealed), Some(key)) = (sealed, cloud.key.lock().clone()) else { return };
            match ble_proto::open_msg(&key, &sealed, now_ms()) {
                Some(Msg::Hello { id, name }) if id != cloud.device_id => {
                    if let Some(l) = ble.links.lock().get_mut(&ev.address) {
                        l.id = Some(id);
                        l.name = name;
                    }
                    files::ble_changed(app);
                }
                Some(Msg::Clip { wire }) => {
                    cloud.receive(&key, &wire, Some(Duration::from_millis(ble_proto::MAX_AGE_MS)));
                }
                Some(Msg::File { name, data }) => {
                    let from = ble.links.lock().get(&ev.address).map(|l| l.name.clone()).unwrap_or_default();
                    if let Some(bytes) = ble_proto::file_bytes(&data) {
                        files::ble_received(app, &name, &bytes, &from).await;
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}
