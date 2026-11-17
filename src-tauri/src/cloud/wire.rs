//! 即時同步的訊息格式：中繼與區域網路送的是同一份密文。
//! 桌面版與 Android 版共用這個檔案（Android 版以 #[path] 引用），兩邊格式才會一致。

use super::crypto::Key;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

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
pub struct Clip {
    pub text: String,
    pub device: String,
    pub at: u64,
    /// 每次複製一個隨機編號：同一份內容可能從區域網路與中繼各收到一次，收過就略過。舊版沒有這個欄位（0）。
    #[serde(default)]
    pub id: u64,
    /// 經由 Google Drive 傳的檔案已經上傳好（文字是空的；舊版遇到空文字會直接略過）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileNote>,
}

/// 「有檔案在 Drive 等你下載」的通知。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileNote {
    /// 說明檔（x1-<編號>.m）在 Drive 上的 ID
    pub m: String,
    /// 收件裝置 ID
    pub to: Vec<String>,
}

/// 加密一次複製的內容，回傳 (編號, 密文)。區域網路與中繼送的是同一份密文。
pub fn seal(key: &Key, text: &str, device: &str) -> (u64, String) {
    let id = rand::random::<u64>() | 1;
    let clip = Clip { text: text.to_string(), device: device.to_string(), at: now_ms(), id, file: None };
    (id, STANDARD.encode(key.seal(&serde_json::to_vec(&clip).unwrap_or_default())))
}

/// 加密一則「檔案已經上傳到 Drive」的通知。
pub fn seal_note(key: &Key, device: &str, note: FileNote) -> String {
    let id = rand::random::<u64>() | 1;
    let clip = Clip { text: String::new(), device: device.to_string(), at: now_ms(), id, file: Some(note) };
    STANDARD.encode(key.seal(&serde_json::to_vec(&clip).unwrap_or_default()))
}

/// 包成線路訊息；`skip` 是不需要中繼再轉送的裝置 ID。
pub fn wire(data: &str, skip: &[String]) -> String {
    serde_json::to_string(&Wire { v: 1, data: data.to_string(), skip: skip.to_vec() }).unwrap_or_default()
}

pub fn decode(key: &Key, msg: &str) -> Option<Clip> {
    let wire: Wire = serde_json::from_str(msg).ok()?;
    let plain = key.open(&STANDARD.decode(wire.data).ok()?)?;
    serde_json::from_slice(&plain).ok()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
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
    fn file_notes_have_empty_text_for_old_clients() {
        let key = Key::generate();
        let note = FileNote { m: "abc".into(), to: vec!["0123456789abcdef".into()] };
        let msg = wire(&seal_note(&key, "PC", note.clone()), &[]);
        let clip = decode(&key, &msg).unwrap();
        assert_eq!(clip.text, "");
        assert_eq!(clip.file, Some(note));
        // 舊版的格式（沒有 file 欄位）照樣解得開
        let (_, data) = seal(&key, "hi", "PC");
        assert!(decode(&key, &wire(&data, &[])).unwrap().file.is_none());
    }

    #[test]
    fn old_clients_without_id_still_decode() {
        let key = Key::generate();
        let old = serde_json::json!({ "text": "hi", "device": "OLD", "at": 1 }).to_string();
        let msg = wire(&STANDARD.encode(key.seal(old.as_bytes())), &[]);
        assert_eq!(decode(&key, &msg).unwrap().id, 0);
    }
}
