//! 純文字內容的雜湊：歷史備份用它比對「同一則紀錄」與同步刪除。
//! 必須和 classify::text_hash（電腦版剪貼簿管線）算出完全相同的值；Android 版以 #[path] 引用這個檔案。

use std::hash::{Hash, Hasher};

/// FNV-1a：不同電腦、不同版本算出來的值都一樣。
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// 等同於只含這段文字的剪貼簿快照（沒有機密標記、檔案或圖片）的內容雜湊。
pub fn text_hash(text: &str) -> u64 {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    false.hash(&mut h);
    Some(text.to_string()).hash(&mut h);
    h.finish()
}
