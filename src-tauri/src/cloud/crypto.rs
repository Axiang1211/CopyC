//! 端對端加密：XChaCha20-Poly1305。金鑰在第一台裝置產生，存在使用者自己 Google Drive 的
//! App 專用資料夾，同一個 Google 帳號的裝置都拿得到；Cloudflare 轉發的只有密文。

use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;

/// 密文開頭的版本標記，之後換演算法時用來分辨。
const MAGIC: &[u8; 4] = b"CPC1";
const NONCE: usize = 24;

#[derive(Clone, PartialEq, Eq)]
pub struct Key(pub [u8; 32]);

impl Key {
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        Key(k)
    }

    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }

    pub fn from_base64(s: &str) -> Option<Self> {
        STANDARD.decode(s.trim()).ok()?.try_into().ok().map(Key)
    }

    pub fn seal(&self, plain: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE];
        rand::thread_rng().fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let ct = cipher.encrypt(XNonce::from_slice(&nonce), plain).expect("encryption cannot fail");
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE + ct.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
    }

    /// 金鑰不對、內容被竄改或格式不符都回傳 None。
    pub fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
        let rest = data.strip_prefix(MAGIC)?;
        if rest.len() < NONCE {
            return None;
        }
        let (nonce, ct) = rest.split_at(NONCE);
        XChaCha20Poly1305::new((&self.0).into()).decrypt(XNonce::from_slice(nonce), ct).ok()
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(…)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper_detection() {
        let key = Key::generate();
        let sealed = key.seal("會議紀錄 hello".as_bytes());
        assert_eq!(key.open(&sealed).unwrap(), "會議紀錄 hello".as_bytes());
        // 同樣內容每次加密結果都不同（隨機 nonce）
        assert_ne!(key.seal(b"x"), key.seal(b"x"));
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(key.open(&bad).is_none());
        assert!(Key::generate().open(&sealed).is_none());
        assert!(key.open(b"CPC1short").is_none());
    }

    #[test]
    fn base64_round_trip() {
        let key = Key::generate();
        assert_eq!(Key::from_base64(&key.to_base64()), Some(key));
        assert!(Key::from_base64("bm90IDMyIGJ5dGVz").is_none());
    }
}
