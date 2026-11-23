//! 藍牙低功耗近距離互傳（電腦端，當中央裝置）。協定見 ble_proto.rs。
//!
//! 沒有 Wi-Fi、也不在同一個網路時，附近登入同一個帳號的手機還是能和這台電腦互傳：
//! - 一直掃描附近的 CopyC 手機（認廣播裡的帳號暗號，不需要配對），找到就連上、互相打招呼；
//! - 這台電腦複製的文字照常同步，另外也從藍牙送給連著的手機；
//! - 傳檔案時，小檔案（[`MAX_FILE`] 以內）在對方不在同一個網路時改走藍牙；大檔案還是走 Wi-Fi 或 Google Drive。
//!
//! 只在已登入、開著「藍牙互傳」、而且帳號裡有 Android 手機時才會打開藍牙（macOS 第一次會問藍牙權限）。

use super::{
    ble_proto::{self, Msg, Reassembly, CHUNK, COMPANY, MAX_FILE, RX, SERVICE, TX},
    crypto::Key,
    file_proto as fp, relay, transfer, Cloud,
};
use btleplug::{
    api::{Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType},
    platform::{Adapter, Manager, Peripheral, PeripheralId},
};
use futures_util::StreamExt;
use parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager as _};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// 沒有藍牙介面卡、或藍牙關著時，隔這麼久再看一次。
const RETRY: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 單塊寫入的逾時（有回應的寫入，對方收到才回）。
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// 要送出的一則訊息（已加密）；送完回報結果，進度回報已送出的位元組數。
struct Outgoing {
    data: Vec<u8>,
    done: Option<oneshot::Sender<bool>>,
    progress: Option<Box<dyn Fn(u64) + Send>>,
}

/// 連著的手機。
struct Link {
    name: String,
    tx: mpsc::UnboundedSender<Outgoing>,
}

#[derive(Default)]
pub struct Ble {
    links: Mutex<HashMap<String, Link>>,
    /// 正在連線（或已連上）的藍牙裝置，避免同一支手機連兩次
    busy: Mutex<HashSet<PeripheralId>>,
}

impl Ble {
    /// 連著的手機：(裝置 ID, 名稱)。
    pub fn peers(&self) -> Vec<(String, String)> {
        self.links.lock().iter().map(|(id, l)| (id.clone(), l.name.clone())).collect()
    }

    /// 把一則即時同步送給連著、但不在 `skip`（已經從區域網路收到）裡的手機。
    pub fn send_clip(&self, key: &Key, wire: &str, skip: &[String]) {
        let data = ble_proto::seal_msg(key, &Msg::Clip { wire: wire.to_string() }, now_ms());
        for (id, link) in self.links.lock().iter() {
            if !skip.contains(id) {
                let _ = link.tx.send(Outgoing { data: data.clone(), done: None, progress: None });
            }
        }
    }
}

/// 藍牙能傳的檔案大小。
pub fn small_enough(size: u64) -> bool {
    size <= MAX_FILE
}

/// 從藍牙傳一個小檔案給 `peer`。回傳錯誤碼（和其他傳送方式相同：offline、disk…）。
pub async fn send_file(app: &AppHandle, peer: &str, path: &Path, name: &str, id: u64) -> Result<(), &'static str> {
    let cloud = app.state::<Cloud>();
    let key = cloud.key.lock().clone().ok_or("offline")?;
    let bytes = tokio::fs::read(path).await.map_err(|_| "disk")?;
    let data = ble_proto::seal_msg(&key, &ble_proto::file_msg(name, &bytes), now_ms());
    let (done_tx, done_rx) = oneshot::channel();
    let total = data.len().max(1) as u64;
    let size = bytes.len() as u64;
    let a = app.clone();
    let progress = Box::new(move |sent: u64| {
        a.state::<Cloud>().transfers.progress(&a, id, size * sent.min(total) / total);
    });
    {
        let links = cloud.ble.links.lock();
        let link = links.get(peer).ok_or("offline")?;
        link.tx.send(Outgoing { data, done: Some(done_tx), progress: Some(progress) }).map_err(|_| "offline")?;
    }
    match done_rx.await {
        Ok(true) => Ok(()),
        _ => Err("offline"),
    }
}

fn enabled(cloud: &Cloud) -> bool {
    let config = *cloud.config.borrow();
    // 帳號裡有手機才需要藍牙（電腦之間有網路就夠了），也避免沒有手機的人被問藍牙權限
    config.signed_in && config.ble && cloud.transfers.remote.devices().iter().any(|d| d.platform == "android")
}

pub async fn run(app: AppHandle) {
    let cloud = app.state::<Cloud>();
    let mut config = cloud.config.subscribe();
    loop {
        if enabled(&cloud) {
            if let Err(e) = serve(&app, &cloud).await {
                eprintln!("copyc: 藍牙互傳暫停：{e}");
            }
            clear(&app, &cloud);
            tokio::select! {
                _ = tokio::time::sleep(RETRY) => {}
                _ = config.changed() => {}
            }
        } else {
            clear(&app, &cloud);
            tokio::select! {
                _ = tokio::time::sleep(RETRY) => {}
                _ = config.changed() => {}
            }
        }
    }
}

fn clear(app: &AppHandle, cloud: &Cloud) {
    let had = !cloud.ble.links.lock().is_empty();
    cloud.ble.links.lock().clear();
    cloud.ble.busy.lock().clear();
    if had {
        let _ = app.emit("transfer:changed", ());
    }
}

async fn adapter() -> Result<Adapter, String> {
    let manager = Manager::new().await.map_err(|e| e.to_string())?;
    manager.adapters().await.map_err(|e| e.to_string())?.into_iter().next().ok_or_else(|| "沒有藍牙".to_string())
}

/// 一直掃描，看到同帳號的手機就連上。設定關掉或登出時回傳。
async fn serve(app: &AppHandle, cloud: &Cloud) -> Result<(), String> {
    let key = cloud.key().await.map_err(|e| format!("{e:?}"))?;
    let central = adapter().await?;
    let mut events = central.events().await.map_err(|e| e.to_string())?;
    // 不用服務過濾：手機把服務 UUID 放在掃描回應、暗號放在廣播本身，過濾會把廣播濾掉
    central.start_scan(ScanFilter::default()).await.map_err(|e| e.to_string())?;
    let mut tick = tokio::time::interval(RETRY);
    loop {
        let event = tokio::select! {
            e = events.next() => e,
            _ = tick.tick() => {
                if !enabled(cloud) {
                    let _ = central.stop_scan().await;
                    return Ok(());
                }
                continue;
            }
        };
        let Some(event) = event else { return Err("藍牙事件中斷".into()) };
        let (id, data) = match event {
            CentralEvent::ManufacturerDataAdvertisement { id, manufacturer_data } => (id, manufacturer_data.get(&COMPANY).cloned()),
            CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => {
                let props = match central.peripheral(&id).await {
                    Ok(p) => p.properties().await.ok().flatten(),
                    Err(_) => None,
                };
                (id, props.and_then(|p| p.manufacturer_data.get(&COMPANY).cloned()))
            }
            CentralEvent::DeviceDisconnected(id) => {
                cloud.ble.busy.lock().remove(&id);
                continue;
            }
            _ => continue,
        };
        let Some(data) = data else { continue };
        if !ble_proto::token_matches(&key, &data, now_ms()) || !cloud.ble.busy.lock().insert(id.clone()) {
            continue;
        }
        let Ok(peripheral) = central.peripheral(&id).await else {
            cloud.ble.busy.lock().remove(&id);
            continue;
        };
        let (app, key) = (app.clone(), key.clone());
        tauri::async_runtime::spawn(async move {
            if let Err(e) = link(&app, &key, &peripheral).await {
                eprintln!("copyc: 藍牙連線結束：{e}");
            }
            let _ = peripheral.disconnect().await;
            app.state::<Cloud>().ble.busy.lock().remove(&id);
        });
    }
}

fn find(p: &Peripheral, uuid: &str) -> Option<Characteristic> {
    let uuid = Uuid::from_str(uuid).ok()?;
    let service = Uuid::from_str(SERVICE).ok()?;
    p.characteristics().into_iter().find(|c| c.uuid == uuid && c.service_uuid == service)
}

/// 和一支手機的連線：打招呼、收手機送來的、把要送的寫過去，直到斷線。
async fn link(app: &AppHandle, key: &Key, p: &Peripheral) -> Result<(), String> {
    let cloud = app.state::<Cloud>();
    tokio::time::timeout(CONNECT_TIMEOUT, p.connect()).await.map_err(|_| "連線逾時")?.map_err(|e| e.to_string())?;
    p.discover_services().await.map_err(|e| e.to_string())?;
    let (rx_char, tx_char) = (find(p, RX).ok_or("不是 CopyC")?, find(p, TX).ok_or("不是 CopyC")?);
    p.subscribe(&tx_char).await.map_err(|e| e.to_string())?;
    let mut notes = p.notifications().await.map_err(|e| e.to_string())?;

    // 寫入：一則一則、一塊一塊依序寫（有回應的寫入，自然做到流量控制）
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outgoing>();
    let hello = Msg::Hello { id: cloud.device_id.clone(), name: cloud.device_name.clone() };
    let _ = out_tx.send(Outgoing { data: ble_proto::seal_msg(key, &hello, now_ms()), done: None, progress: None });
    let writer = {
        let p = p.clone();
        async move {
            let mut next_id: u16 = rand::random();
            while let Some(out) = out_rx.recv().await {
                next_id = next_id.wrapping_add(1);
                let mut ok = true;
                let mut sent = 0u64;
                for frame in ble_proto::frames(next_id, &out.data, CHUNK) {
                    let w = p.write(&rx_char, &frame, WriteType::WithResponse);
                    if !matches!(tokio::time::timeout(WRITE_TIMEOUT, w).await, Ok(Ok(()))) {
                        ok = false;
                        break;
                    }
                    sent += (frame.len() - ble_proto::HEADER) as u64;
                    if let Some(progress) = &out.progress {
                        progress(sent);
                    }
                }
                if let Some(done) = out.done {
                    let _ = done.send(ok);
                }
                if !ok {
                    return;
                }
            }
        }
    };

    // 讀取：手機的通知，拼回完整訊息後解開
    let mut peer_id: Option<String> = None;
    let reader = async {
        let mut parts = Reassembly::default();
        while let Some(note) = notes.next().await {
            if note.uuid != tx_char.uuid {
                continue;
            }
            let Some(sealed) = parts.push(&note.value) else { continue };
            match ble_proto::open_msg(key, &sealed, now_ms()) {
                Some(Msg::Hello { id, name }) if id != cloud.device_id => {
                    cloud.ble.links.lock().insert(id.clone(), Link { name, tx: out_tx.clone() });
                    peer_id = Some(id);
                    let _ = app.emit("transfer:changed", ());
                }
                Some(Msg::Clip { wire }) => {
                    relay::receive(app, &cloud, key, &wire, Some(Duration::from_millis(ble_proto::MAX_AGE_MS)));
                }
                Some(Msg::File { name, data }) => {
                    let from = peer_id.as_ref().and_then(|id| cloud.ble.links.lock().get(id).map(|l| l.name.clone()));
                    if let Some(bytes) = ble_proto::file_bytes(&data) {
                        receive_file(app, &name, &bytes, &from.unwrap_or_default()).await;
                    }
                }
                _ => {}
            }
        }
    };
    tokio::select! {
        _ = writer => {}
        _ = reader => {}
    }
    if let Some(id) = peer_id {
        cloud.ble.links.lock().remove(&id);
        let _ = app.emit("transfer:changed", ());
    }
    Ok(())
}

/// 手機從藍牙傳來一個小檔案：存進收件資料夾、列在傳送紀錄裡，並放進剪貼簿。
async fn receive_file(app: &AppHandle, name: &str, bytes: &[u8], from: &str) {
    let cloud = app.state::<Cloud>();
    let dest = transfer::dest_dir(app);
    let _ = tokio::fs::create_dir_all(&dest).await;
    let Some(file_name) = Path::new(name).file_name().map(|n| n.to_owned()) else { return };
    let path = fp::unique_path(&dest, Path::new(&file_name));
    let size = bytes.len() as u64;
    let item = transfer::Item::new(transfer::Direction::In, transfer::Via::Ble, name.to_string(), size, from.to_string(), transfer::TransferState::Active);
    let (id, _) = cloud.transfers.add(item, None);
    if tokio::fs::write(&path, bytes).await.is_err() {
        cloud.transfers.fail(app, id, "disk");
        return;
    }
    let shown = path.to_string_lossy().into_owned();
    cloud.transfers.set(app, id, |it| {
        it.state = transfer::TransferState::Done;
        it.done = size;
        it.path = Some(shown);
    });
    transfer::put_on_clipboard(app, vec![path], from);
}
