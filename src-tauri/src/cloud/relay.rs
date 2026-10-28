//! 即時同步（C）：連到官網 Worker 的中繼（Cloudflare Durable Object），同一個 Google 帳號的裝置
//! 在同一個房間。複製的文字加密後送出，伺服器轉發給房間裡的其他裝置，不保存、也解不開。
//! 已經在區域網路直接收到的裝置（見 [`super::lan`]）列在 `skip`，伺服器不會再轉給它們。

use super::{crypto::Key, Cloud, CloudError, Live};
use crate::{classify, state::AppState};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

pub const RELAY_URL: &str = "wss://www.copyc.app.dreamsworld.top/sync/ws";
/// 保持連線：Durable Object 會自動回 "pong"，不會因此被喚醒。
const PING: Duration = Duration::from_secs(30);

/// 線路上的訊息：只有密文，以及（送往中繼時）已經在區域網路收到的裝置 ID。
#[derive(Serialize, Deserialize)]
struct Wire {
    v: u32,
    data: String,
    #[serde(default, skip_serializing_if = "<[_]>::is_empty")]
    skip: Vec<String>,
}

/// 密文裡的內容。
#[derive(Serialize, Deserialize)]
pub(super) struct Clip {
    pub text: String,
    pub device: String,
    pub at: u64,
    /// 每次複製一個隨機編號：同一份內容可能從區域網路與中繼各收到一次，收過就略過。舊版沒有這個欄位（0）。
    #[serde(default)]
    pub id: u64,
}

/// 加密一次複製的內容，回傳 (編號, 密文)。區域網路與中繼送的是同一份密文。
pub fn seal(key: &Key, text: &str, device: &str) -> (u64, String) {
    let id = rand::random::<u64>() | 1;
    let clip = Clip { text: text.to_string(), device: device.to_string(), at: crate::state::now_ms(), id };
    (id, STANDARD.encode(key.seal(&serde_json::to_vec(&clip).unwrap_or_default())))
}

/// 包成線路訊息；`skip` 是不需要中繼再轉送的裝置 ID。
pub fn wire(data: &str, skip: &[String]) -> String {
    serde_json::to_string(&Wire { v: 1, data: data.to_string(), skip: skip.to_vec() }).unwrap_or_default()
}

pub(super) fn decode(key: &Key, msg: &str) -> Option<Clip> {
    let wire: Wire = serde_json::from_str(msg).ok()?;
    let plain = key.open(&STANDARD.decode(wire.data).ok()?)?;
    serde_json::from_slice(&plain).ok()
}

pub async fn run(app: AppHandle) {
    let cloud = app.state::<Cloud>();
    let mut config = cloud.config.subscribe();
    let mut backoff = Duration::from_secs(2);
    loop {
        let c = *config.borrow_and_update();
        if !(c.signed_in && c.live) {
            cloud.update(|rt| rt.live = Live::Off);
            if config.changed().await.is_err() {
                return;
            }
            continue;
        }
        cloud.update(|rt| rt.live = Live::Connecting);
        let wait = match connect(&app, &cloud, &mut config).await {
            Ok(()) => {
                backoff = Duration::from_secs(2);
                backoff
            }
            // 伺服器還沒啟用（或這個版本沒有設定 Google 登入）：很久之後再試
            Err(CloudError::Unavailable) => Duration::from_secs(600),
            Err(_) => {
                backoff = (backoff * 2).min(Duration::from_secs(60));
                backoff
            }
        };
        *cloud.relay_tx.lock() = None;
        if cloud.config.borrow().signed_in {
            cloud.update(|rt| rt.live = Live::Connecting);
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            r = config.changed() => if r.is_err() { return },
        }
    }
}

/// 連線一次，直到斷線或設定改變。正常斷線回傳 Ok（馬上重連）。
async fn connect(
    app: &AppHandle,
    cloud: &Cloud,
    config: &mut tokio::sync::watch::Receiver<super::Config>,
) -> Result<(), CloudError> {
    let key = cloud.key().await?;
    let id_token = cloud.id_token().await?;
    // 帶上這台裝置的 ID，中繼才知道 `skip` 指的是哪條連線
    let url = format!("{RELAY_URL}?d={}", cloud.device_id);
    let mut req = url.into_client_request().map_err(|_| CloudError::Network)?;
    let auth = format!("Bearer {id_token}").parse().map_err(|_| CloudError::Network)?;
    req.headers_mut().insert("Authorization", auth);
    let (ws, _) = match tokio_tungstenite::connect_async(req).await {
        Ok(ok) => ok,
        Err(tungstenite::Error::Http(resp)) => {
            return Err(match resp.status().as_u16() {
                401 => {
                    cloud.drop_id_token().await;
                    CloudError::Network
                }
                404 | 503 => CloudError::Unavailable,
                429 => CloudError::RateLimited,
                _ => CloudError::Network,
            })
        }
        Err(_) => return Err(CloudError::Network),
    };
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    *cloud.relay_tx.lock() = Some(tx);
    cloud.update(|rt| {
        rt.live = Live::On;
        if rt.error == Some(CloudError::Unavailable) {
            rt.error = None;
        }
    });
    let mut ping = tokio::time::interval(PING);
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) if text != "pong" => {
                    if !receive(app, cloud, &key, &text, None) {
                        // 解不開：可能另一台裝置換了金鑰，重新讀取後再連
                        *cloud.key.lock() = None;
                        return Ok(());
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ok(()),
                _ => {}
            },
            out = rx.recv() => match out {
                Some(out) => if sink.send(Message::Text(out)).await.is_err() { return Ok(()) },
                None => return Ok(()),
            },
            _ = ping.tick() => if sink.send(Message::Text("ping".into())).await.is_err() { return Ok(()) },
            r = config.changed() => {
                let c = *config.borrow();
                if r.is_err() || !(c.signed_in && c.live) {
                    let _ = sink.send(Message::Close(None)).await;
                    return Ok(());
                }
            }
        }
    }
}

/// 收到其他裝置的內容（經中繼或區域網路）。回傳 false 表示解不開。
/// `max_age`：區域網路收到的內容太舊就不採用，避免有人錄下封包之後重播。
pub(super) fn receive(app: &AppHandle, cloud: &Cloud, key: &Key, msg: &str, max_age: Option<Duration>) -> bool {
    let Some(clip) = decode(key, msg) else { return false };
    if max_age.is_some_and(|age| crate::state::now_ms().abs_diff(clip.at) > age.as_millis() as u64) {
        return true;
    }
    if !cloud.first_time(clip.id) {
        return true;
    }
    let Some(entry) = classify::remote_entry(&clip.text) else { return true };
    let hash = classify::text_hash(&clip.text);
    let state = app.state::<AppState>();
    if state.settings.lock().cloud_apply {
        // 放進剪貼簿；剪貼簿管線看到這份內容時會標成「從某台裝置同步」並寫進歷史紀錄
        cloud.expect_incoming(hash, clip.device);
        let text = clip.text;
        std::thread::spawn(move || {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(text);
            }
        });
    } else if let Some(store) = state.store.get() {
        if store.add_remote(&entry, hash, clip.at, &clip.device).unwrap_or(false) {
            let _ = app.emit("history:changed", ());
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_hides_content() {
        let key = Key::generate();
        let (id, data) = seal(&key, "hello 世界", "MY-PC");
        let msg = wire(&data, &[]);
        assert!(!msg.contains("hello") && !msg.contains("MY-PC") && !msg.contains("skip"));
        let clip = decode(&key, &msg).unwrap();
        assert_eq!((clip.text.as_str(), clip.device.as_str(), clip.id), ("hello 世界", "MY-PC", id));
        assert_ne!(id, 0);
        // 送往中繼時帶上不用再轉送的裝置，密文不變
        let relayed = wire(&data, &["0123456789abcdef".into()]);
        assert!(relayed.contains(r#""skip":["0123456789abcdef"]"#));
        assert_eq!(decode(&key, &relayed).unwrap().id, id);
        assert!(decode(&Key::generate(), &msg).is_none());
        assert!(decode(&key, "not json").is_none());
    }

    #[test]
    fn old_clients_without_id_still_decode() {
        let key = Key::generate();
        let old = serde_json::json!({ "text": "hi", "device": "OLD", "at": 1 }).to_string();
        let msg = wire(&STANDARD.encode(key.seal(old.as_bytes())), &[]);
        assert_eq!(decode(&key, &msg).unwrap().id, 0);
    }
}
