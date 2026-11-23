//! 藍牙低功耗（BLE）近距離互傳的協定（電腦版與 Android 版共用；Android 版以 #[path] 引用）。
//!
//! 沒有 Wi-Fi、也沒有網路時，手機和電腦還是能互傳文字與小檔案（例如截圖）：
//! - **手機**當周邊裝置：廣播 [`SERVICE`]，掃描回應裡的廠商資料放一個 8 位元組的「帳號暗號」[`token`]。
//!   暗號由帳號金鑰與時間算出，每 10 分鐘換一次：同一個帳號的電腦認得出來，其他人無法得知是誰、也追蹤不了。
//! - **電腦**當中央裝置：掃描、比對暗號、連上去、訂閱 [`TX`]（手機送來的），寫入 [`RX`]（送給手機的）。
//! - 每則訊息都先用帳號金鑰加密（[`seal_msg`]），再切成小塊（[`frames`]）送出，收到後重組（[`Reassembly`]）再解開。
//!   不需要藍牙配對；解不開、太舊（重播）的訊息一律丟掉。
//!
//! 藍牙很慢（大約每秒 20～100 KB），所以只傳文字與 [`MAX_FILE`] 以內的小檔案；大圖片與影片走 Wi-Fi 或 Google Drive。

use super::crypto::Key;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// CopyC 的 GATT 服務。
pub const SERVICE: &str = "6f1c0001-5a3e-4c1b-9d2f-8b7a4e6c0b11";
/// 電腦 → 手機（電腦寫入）。
pub const RX: &str = "6f1c0002-5a3e-4c1b-9d2f-8b7a4e6c0b11";
/// 手機 → 電腦（手機通知）。
pub const TX: &str = "6f1c0003-5a3e-4c1b-9d2f-8b7a4e6c0b11";
/// 掃描回應裡廠商資料的公司代碼（0xFFFF：未向藍牙聯盟註冊、測試用）。
pub const COMPANY: u16 = 0xFFFF;
pub const TOKEN_LEN: usize = 8;
/// 暗號每這麼久換一次。
pub const WINDOW_MS: u64 = 10 * 60 * 1000;
/// 每一塊的大小上限（含標頭）：macOS 常見的 MTU 是 185，扣掉 3 位元組的 ATT 標頭。
pub const CHUNK: usize = 180;
/// 每一塊的標頭：訊息編號（2）＋序號（2）＋是否最後一塊（1）。
pub const HEADER: usize = 5;
/// 走藍牙的檔案大小上限，超過就走 Wi-Fi 或 Google Drive。
pub const MAX_FILE: u64 = 1_000_000;
/// 重組中的訊息上限（加密與 base64 會讓檔案變大約三分之一）。
pub const MAX_MSG: usize = 1_600_000;
/// 訊息時間和現在差太多就不採用（防重播）。
pub const MAX_AGE_MS: u64 = 10 * 60 * 1000;
/// 重組到一半、這麼久沒有下一塊就丟掉。
const STALE: Duration = Duration::from_secs(60);

/// 這個時間點的帳號暗號。
pub fn token(key: &Key, at_ms: u64) -> [u8; TOKEN_LEN] {
    let window = at_ms / WINDOW_MS;
    let mut h = Sha256::new();
    h.update(b"copyc-ble-1");
    h.update(key.0);
    h.update(window.to_le_bytes());
    let digest = h.finalize();
    let mut out = [0u8; TOKEN_LEN];
    out.copy_from_slice(&digest[..TOKEN_LEN]);
    out
}

/// 掃描到的暗號是不是同一個帳號（容許前後一個時段，兩台裝置的時鐘不必完全一致）。
pub fn token_matches(key: &Key, got: &[u8], now_ms: u64) -> bool {
    got.len() == TOKEN_LEN
        && [now_ms.saturating_sub(WINDOW_MS), now_ms, now_ms + WINDOW_MS].iter().any(|t| token(key, *t) == got)
}

/// 藍牙上的訊息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "camelCase")]
pub enum Msg {
    /// 連上後互相介紹：裝置 ID 與名稱。
    Hello { id: String, name: String },
    /// 一則即時同步（和中繼、區域網路完全相同的線路訊息，見 wire.rs）。
    Clip { wire: String },
    /// 一個小檔案（base64）。
    File { name: String, data: String },
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    t: u64,
    #[serde(flatten)]
    m: Msg,
}

pub fn file_msg(name: &str, bytes: &[u8]) -> Msg {
    Msg::File { name: name.to_string(), data: STANDARD.encode(bytes) }
}

/// 檔案訊息裡的內容。
pub fn file_bytes(data: &str) -> Option<Vec<u8>> {
    STANDARD.decode(data).ok()
}

pub fn seal_msg(key: &Key, msg: &Msg, now_ms: u64) -> Vec<u8> {
    let env = Envelope { t: now_ms, m: msg.clone() };
    key.seal(&serde_json::to_vec(&env).unwrap_or_default())
}

/// 解開一則訊息；解不開或太舊（重播）回傳 None。
pub fn open_msg(key: &Key, data: &[u8], now_ms: u64) -> Option<Msg> {
    let env: Envelope = serde_json::from_slice(&key.open(data)?).ok()?;
    (env.t.abs_diff(now_ms) <= MAX_AGE_MS).then_some(env.m)
}

/// 把一則（加密好的）訊息切成小塊。`chunk` 是每一塊的大小上限（含標頭）。
pub fn frames(id: u16, msg: &[u8], chunk: usize) -> Vec<Vec<u8>> {
    let body = chunk.max(HEADER + 1) - HEADER;
    let parts: Vec<&[u8]> = if msg.is_empty() { vec![&[][..]] } else { msg.chunks(body).collect() };
    let n = parts.len();
    parts
        .into_iter()
        .enumerate()
        .map(|(i, part)| {
            let mut f = Vec::with_capacity(HEADER + part.len());
            f.extend_from_slice(&id.to_le_bytes());
            f.extend_from_slice(&(i as u16).to_le_bytes());
            f.push(u8::from(i + 1 == n));
            f.extend_from_slice(part);
            f
        })
        .collect()
}

/// 把收到的小塊依序拼回完整訊息。藍牙同一條連線上的資料是依序送達的，順序不對就整則丟掉。
#[derive(Default)]
pub struct Reassembly {
    open: HashMap<u16, (u16, Vec<u8>, Instant)>,
}

impl Reassembly {
    /// 收到一塊；拼完一整則時回傳它。
    pub fn push(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < HEADER {
            return None;
        }
        self.open.retain(|_, (_, _, at)| at.elapsed() < STALE);
        let id = u16::from_le_bytes([frame[0], frame[1]]);
        let seq = u16::from_le_bytes([frame[2], frame[3]]);
        let last = frame[4] == 1;
        let body = &frame[HEADER..];
        if seq == 0 {
            self.open.insert(id, (0, Vec::new(), Instant::now()));
        }
        let (next, buf, at) = self.open.get_mut(&id)?;
        if *next != seq || buf.len() + body.len() > MAX_MSG {
            self.open.remove(&id);
            return None;
        }
        buf.extend_from_slice(body);
        *next = next.wrapping_add(1);
        *at = Instant::now();
        if last {
            return self.open.remove(&id).map(|(_, buf, _)| buf);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_rotates_and_matches_neighbouring_windows() {
        let key = Key::generate();
        let now = 1_800_000_000_000;
        let t = token(&key, now);
        assert!(token_matches(&key, &t, now));
        assert!(token_matches(&key, &t, now + WINDOW_MS)); // 對方時鐘快一點
        assert!(!token_matches(&key, &t, now + 5 * WINDOW_MS));
        assert_ne!(token(&key, now), token(&key, now + 2 * WINDOW_MS));
        assert!(!token_matches(&Key::generate(), &t, now)); // 別的帳號認不出來
        assert!(!token_matches(&key, &t[..4], now));
    }

    #[test]
    fn messages_round_trip_through_frames() {
        let key = Key::generate();
        let now = 1_800_000_000_000;
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let sealed = seal_msg(&key, &file_msg("截圖.png", &data), now);
        let mut r = Reassembly::default();
        let mut got = None;
        let fs = frames(7, &sealed, CHUNK);
        assert!(fs.iter().all(|f| f.len() <= CHUNK));
        for f in &fs {
            if let Some(m) = r.push(f) {
                got = Some(m);
            }
        }
        let msg = open_msg(&key, &got.unwrap(), now + 1000).unwrap();
        let Msg::File { name, data: b64 } = msg else { panic!() };
        assert_eq!(name, "截圖.png");
        assert_eq!(file_bytes(&b64).unwrap(), data);
    }

    #[test]
    fn stale_wrong_key_and_out_of_order_are_rejected() {
        let key = Key::generate();
        let now = 1_800_000_000_000;
        let sealed = seal_msg(&key, &Msg::Hello { id: "a".into(), name: "PC".into() }, now);
        assert!(open_msg(&key, &sealed, now + MAX_AGE_MS + 1).is_none());
        assert!(open_msg(&Key::generate(), &sealed, now).is_none());
        let fs = frames(1, &[1u8; 400], 100);
        let mut r = Reassembly::default();
        assert!(r.push(&fs[0]).is_none());
        assert!(r.push(&fs[2]).is_none()); // 跳過一塊：整則丟掉
        assert!(r.push(&fs[3]).is_none());
        // 一塊就結束的訊息
        let one = frames(2, b"hi", CHUNK);
        assert_eq!(one.len(), 1);
        assert_eq!(Reassembly::default().push(&one[0]).unwrap(), b"hi");
    }
}
