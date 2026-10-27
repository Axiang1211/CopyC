//! 本機複製紀錄：SQLite 資料庫，存在使用者的 App 資料夾。
//!
//! 文字類內容保存全文，用於搜尋與重新複製；圖片只保存尺寸與縮圖；檔案只記錄本機路徑。
//! 同一份內容（雜湊相同）再次複製時只更新時間與來源，不會重複一筆。
//!
//! 登入雲端同步後，這台電腦的文字類紀錄（`origin IS NULL`）會依「天」加密備份到 Google Drive，
//! 其他裝置的紀錄下載後以 `origin`（裝置名稱）標記。刪除會留下墓碑，讓其他裝置也一起刪。

use crate::classify::{Category, Entry};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// 超過這個大小的文字只保存開頭，避免一次複製幾十 MB 拖慢資料庫；這類紀錄不能重新複製。
const MAX_TEXT_BYTES: usize = 1 << 20;
/// 列表只回傳開頭這麼多字元，完整內容在重新複製時才讀。
const LIST_TEXT_CHARS: u32 = 600;
/// 列表最多列出的檔案路徑數（payload 裡有總數）。
const LIST_PATHS: usize = 5;
/// 一天的毫秒數；雲端備份以 UTC 日為單位分檔。
pub const DAY_MS: u64 = 86_400_000;
/// 超過這個大小的文字不同步到雲端（只留在本機）。
pub const CLOUD_MAX_BYTES: usize = 256 * 1024;
const SYNCABLE: &str = "('text', 'link', 'email', 'phone', 'color')";

pub struct History {
    conn: Mutex<Connection>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: i64,
    /// Unix 毫秒
    pub at: u64,
    pub category: String,
    pub source: Option<String>,
    /// 從其他裝置同步來的紀錄：那台裝置的名稱
    pub origin: Option<String>,
    /// 顯示用的 Payload（與通知框相同結構）
    pub payload: serde_json::Value,
    /// 文字類內容的開頭；密碼類永遠是 None
    pub text: Option<String>,
    pub paths: Vec<String>,
    pub can_copy: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub items: Vec<Item>,
    /// 目前分類（含搜尋條件）的總筆數，用於「載入更多」
    pub total: u64,
    /// 各分類筆數（含搜尋條件）
    pub counts: BTreeMap<String, u64>,
}

/// 重新複製時寫回剪貼簿的內容。
#[derive(Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Files(Vec<PathBuf>),
}

impl History {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::init(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                hash INTEGER NOT NULL UNIQUE,
                category TEXT NOT NULL,
                source TEXT,
                payload TEXT NOT NULL,
                content TEXT,
                truncated INTEGER NOT NULL DEFAULT 0,
                paths TEXT
            );
            CREATE INDEX IF NOT EXISTS entries_at ON entries(at DESC);
            CREATE TABLE IF NOT EXISTS tombstones (
                hash INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                day INTEGER,
                pushed INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        // 1.0.8 的資料庫沒有同步欄位，補上即可（舊紀錄都是這台電腦的、尚未備份）
        let has_origin = conn
            .prepare("SELECT 1 FROM pragma_table_info('entries') WHERE name = 'origin'")?
            .exists([])?;
        if !has_origin {
            conn.execute_batch(
                "ALTER TABLE entries ADD COLUMN origin TEXT;
                 ALTER TABLE entries ADD COLUMN synced INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 新增一筆；同一份內容已存在時只把它移到最上面。之後刪掉超過上限的舊紀錄。
    pub fn add(&self, entry: &Entry, source: Option<&str>, hash: u64, at: u64, limit: u32) -> rusqlite::Result<()> {
        let (content, truncated) = match &entry.text {
            Some(t) if t.len() > MAX_TEXT_BYTES => (Some(truncate_bytes(t, MAX_TEXT_BYTES).to_string()), true),
            Some(t) => (Some(t.clone()), false),
            None => (None, false),
        };
        let paths = (!entry.paths.is_empty()).then(|| {
            let list: Vec<String> = entry.paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            serde_json::to_string(&list).unwrap_or_default()
        });
        let payload = serde_json::to_string(&entry.payload).unwrap_or_default();
        let conn = self.conn.lock();
        // 在這台電腦重新複製：變成本機紀錄、重新排入備份；之前刪除留下的墓碑也作廢
        conn.execute(
            "INSERT INTO entries (at, hash, category, source, payload, content, truncated, paths, origin, synced)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, 0)
             ON CONFLICT(hash) DO UPDATE SET at = excluded.at, source = excluded.source, origin = NULL, synced = 0",
            params![at as i64, hash as i64, entry.category.key(), source, payload, content, truncated, paths],
        )?;
        conn.execute("DELETE FROM tombstones WHERE hash = ?1", params![hash as i64])?;
        trim(&conn, limit)
    }

    /// 其他裝置的紀錄。已經有同一份內容時只更新成較新的時間；被刪除過（有墓碑）的不再加回來。
    /// 回傳是否有變動。
    pub fn add_remote(&self, entry: &Entry, hash: u64, at: u64, origin: &str) -> rusqlite::Result<bool> {
        let payload = serde_json::to_string(&entry.payload).unwrap_or_default();
        let conn = self.conn.lock();
        let changed = conn.execute(
            "INSERT INTO entries (at, hash, category, source, payload, content, truncated, paths, origin, synced)
             SELECT ?1, ?2, ?3, NULL, ?4, ?5, 0, NULL, ?6, 1
             WHERE NOT EXISTS (SELECT 1 FROM tombstones WHERE hash = ?2)
             ON CONFLICT(hash) DO UPDATE SET at = excluded.at WHERE excluded.at > entries.at",
            params![at as i64, hash as i64, entry.category.key(), payload, entry.text, origin],
        )?;
        Ok(changed > 0)
    }

    /// 只保留最新的 limit 筆。
    pub fn trim(&self, limit: u32) -> rusqlite::Result<()> {
        trim(&self.conn.lock(), limit)
    }

    /// 依搜尋字與分類分頁查詢，最新的在前。密碼類內容不參與文字搜尋，也不回傳明文。
    pub fn query(&self, q: &str, category: Option<&str>, offset: u32, limit: u32) -> rusqlite::Result<Page> {
        let q = q.trim();
        let like = format!("%{}%", escape_like(q));
        let filter = "(?1 = '' OR (category != 'secret' AND content LIKE ?2 ESCAPE '\\')
                      OR paths LIKE ?2 ESCAPE '\\' OR source LIKE ?2 ESCAPE '\\' OR origin LIKE ?2 ESCAPE '\\')";
        let conn = self.conn.lock();

        let mut counts = BTreeMap::new();
        let mut stmt = conn.prepare(&format!("SELECT category, COUNT(*) FROM entries WHERE {filter} GROUP BY category"))?;
        let rows = stmt.query_map(params![q, like], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
        for row in rows {
            let (cat, n) = row?;
            counts.insert(cat, n);
        }
        let total = match category {
            Some(c) => counts.get(c).copied().unwrap_or(0),
            None => counts.values().sum(),
        };

        let mut stmt = conn.prepare(&format!(
            "SELECT id, at, category, source, payload,
                    CASE WHEN category = 'secret' THEN NULL ELSE substr(content, 1, {LIST_TEXT_CHARS}) END,
                    truncated, paths, origin
             FROM entries WHERE {filter} AND (?3 IS NULL OR category = ?3)
             ORDER BY at DESC LIMIT ?4 OFFSET ?5"
        ))?;
        let items = stmt
            .query_map(params![q, like, category, limit, offset], |r| {
                let category: String = r.get(2)?;
                let text: Option<String> = r.get(5)?;
                let truncated: bool = r.get(6)?;
                let paths: Vec<String> = r
                    .get::<_, Option<String>>(7)?
                    .and_then(|p| serde_json::from_str(&p).ok())
                    .unwrap_or_default();
                let payload: String = r.get(4)?;
                let can_copy = match category.as_str() {
                    "image" => false,
                    "files" => !paths.is_empty(),
                    _ => !truncated,
                };
                Ok(Item {
                    id: r.get(0)?,
                    at: r.get::<_, i64>(1)? as u64,
                    source: r.get(3)?,
                    origin: r.get(8)?,
                    payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
                    text,
                    paths: paths.into_iter().take(LIST_PATHS).collect(),
                    can_copy,
                    category,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Page { items, total, counts })
    }

    /// 重新複製用的完整內容。圖片與被截斷的超長文字回傳 None。
    pub fn content(&self, id: i64) -> rusqlite::Result<Option<Content>> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT category, content, truncated, paths FROM entries WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.and_then(|(category, content, truncated, paths)| match category.as_str() {
            "image" => None,
            "files" => {
                let list: Vec<PathBuf> = serde_json::from_str(&paths?).ok()?;
                Some(Content::Files(list))
            }
            _ if truncated => None,
            _ => content.map(Content::Text),
        }))
    }

    /// 刪除一筆。文字類紀錄會留下墓碑：其他裝置同步時一起刪除，這台電腦的雲端備份也會重寫那一天。
    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO tombstones (hash, at, day, pushed)
                 SELECT hash, ?2, CASE WHEN origin IS NULL THEN at / {DAY_MS} END, 0
                 FROM entries WHERE id = ?1 AND category IN {SYNCABLE}"
            ),
            params![id, crate::state::now_ms() as i64],
        )?;
        conn.execute("DELETE FROM entries WHERE id = ?1", params![id]).map(drop)
    }

    pub fn clear(&self) -> rusqlite::Result<()> {
        self.conn.lock().execute("DELETE FROM entries", []).map(drop)
    }

    /// 需要重寫雲端備份的日子：有尚未備份的本機紀錄，或那天有紀錄被刪除。
    pub fn dirty_days(&self) -> rusqlite::Result<Vec<u64>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT at / {DAY_MS} FROM entries WHERE origin IS NULL AND synced = 0 AND category IN {SYNCABLE}
             UNION SELECT day FROM tombstones WHERE pushed = 0 AND day IS NOT NULL"
        ))?;
        let days = stmt.query_map([], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(days.into_iter().map(|d| d as u64).collect())
    }

    /// 某一天（UTC）這台電腦要備份的文字紀錄：(id, 時間, 全文)。超長或被截斷的不備份。
    pub fn day_texts(&self, day: u64) -> rusqlite::Result<Vec<(i64, u64, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, at, content FROM entries
             WHERE origin IS NULL AND at / {DAY_MS} = ?1 AND category IN {SYNCABLE}
               AND truncated = 0 AND content IS NOT NULL AND length(CAST(content AS BLOB)) <= {CLOUD_MAX_BYTES}
             ORDER BY at"
        ))?;
        let rows = stmt
            .query_map(params![day as i64], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?)))?
            .collect();
        rows
    }

    /// 那一天的備份已上傳：標記那幾筆，並把那天的墓碑標成已處理。
    pub fn mark_day_synced(&self, day: u64, ids: &[i64]) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for id in ids {
            tx.execute("UPDATE entries SET synced = 1 WHERE id = ?1", params![id])?;
        }
        // 那天沒有可備份內容的本機紀錄（例如超長文字）也不用再處理
        tx.execute(
            &format!("UPDATE entries SET synced = 1 WHERE origin IS NULL AND at / {DAY_MS} = ?1"),
            params![day as i64],
        )?;
        tx.execute("UPDATE tombstones SET day = NULL WHERE day = ?1", params![day as i64])?;
        tx.commit()
    }

    /// 是否有還沒發布的墓碑。
    pub fn has_unpushed_tombstones(&self) -> rusqlite::Result<bool> {
        self.conn.lock().prepare("SELECT 1 FROM tombstones WHERE pushed = 0")?.exists([])
    }

    /// 最近（since 之後）的墓碑雜湊，發布給其他裝置。順便清掉更舊的墓碑。
    pub fn tombstones_since(&self, since: u64) -> rusqlite::Result<Vec<u64>> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM tombstones WHERE at < ?1 AND day IS NULL", params![since as i64])?;
        let mut stmt = conn.prepare("SELECT hash FROM tombstones WHERE at >= ?1")?;
        let rows = stmt.query_map(params![since as i64], |r| r.get::<_, i64>(0).map(|h| h as u64))?.collect();
        rows
    }

    pub fn mark_tombstones_pushed(&self) -> rusqlite::Result<()> {
        self.conn.lock().execute("UPDATE tombstones SET pushed = 1", []).map(drop)
    }

    /// 套用其他裝置的刪除。這台電腦自己的紀錄也要重寫雲端備份，所以留下尚未處理的墓碑。
    pub fn apply_tombstones(&self, hashes: &[u64]) -> rusqlite::Result<bool> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let now = crate::state::now_ms() as i64;
        let mut changed = false;
        for &hash in hashes {
            let hash = hash as i64;
            let row: Option<(i64, Option<String>)> = tx
                .query_row("SELECT at, origin FROM entries WHERE hash = ?1", params![hash], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            match row {
                Some((at, None)) => {
                    tx.execute(
                        "INSERT OR REPLACE INTO tombstones (hash, at, day, pushed) VALUES (?1, ?2, ?3, 0)",
                        params![hash, now, at / DAY_MS as i64],
                    )?;
                }
                _ => {
                    tx.execute(
                        "INSERT OR IGNORE INTO tombstones (hash, at, day, pushed) VALUES (?1, ?2, NULL, 1)",
                        params![hash, now],
                    )?;
                }
            }
            changed |= tx.execute("DELETE FROM entries WHERE hash = ?1", params![hash])? > 0;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// 加密金鑰換了：所有本機紀錄與墓碑都要用新金鑰重新上傳。
    pub fn mark_all_unsynced(&self) -> rusqlite::Result<()> {
        self.conn.lock().execute_batch("UPDATE entries SET synced = 0 WHERE origin IS NULL; UPDATE tombstones SET pushed = 0;")
    }

    /// 登出時刪掉其他裝置同步來的紀錄（例如共用電腦上登出後不該留下別台的內容）。
    pub fn delete_remote(&self) -> rusqlite::Result<()> {
        self.conn.lock().execute("DELETE FROM entries WHERE origin IS NOT NULL", []).map(drop)
    }

    /// 關閉「保存密碼與敏感內容」時，把已保存的密碼類紀錄一併刪掉。
    pub fn delete_category(&self, category: Category) -> rusqlite::Result<()> {
        self.conn.lock().execute("DELETE FROM entries WHERE category = ?1", params![category.key()]).map(drop)
    }
}

fn trim(conn: &Connection, limit: u32) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM entries WHERE id IN (SELECT id FROM entries ORDER BY at DESC LIMIT -1 OFFSET ?1)",
        params![limit],
    )
    .map(drop)
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn truncate_bytes(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::{Payload, SensitiveReason};

    fn text(t: &str, category: Category) -> Entry {
        Entry {
            category,
            payload: Payload::Text { preview: t.into(), truncated: false, chars: t.len(), lines: 1 },
            text: Some(t.into()),
            paths: Vec::new(),
        }
    }

    #[test]
    fn same_content_moves_to_top_instead_of_duplicating() {
        let h = History::in_memory();
        h.add(&text("a", Category::Text), Some("Notes"), 1, 10, 100).unwrap();
        h.add(&text("b", Category::Text), None, 2, 20, 100).unwrap();
        h.add(&text("a", Category::Text), Some("Mail"), 1, 30, 100).unwrap();
        let page = h.query("", None, 0, 10).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].text.as_deref(), Some("a"));
        assert_eq!(page.items[0].source.as_deref(), Some("Mail"));
    }

    #[test]
    fn keeps_only_the_newest_up_to_the_limit() {
        let h = History::in_memory();
        for i in 0..5u64 {
            h.add(&text(&format!("t{i}"), Category::Text), None, i, i, 3).unwrap();
        }
        let page = h.query("", None, 0, 10).unwrap();
        let texts: Vec<_> = page.items.iter().map(|i| i.text.clone().unwrap()).collect();
        assert_eq!(texts, ["t4", "t3", "t2"]);
    }

    #[test]
    fn search_and_category_filters() {
        let h = History::in_memory();
        h.add(&text("Meeting at 3pm", Category::Text), Some("Notes"), 1, 1, 100).unwrap();
        h.add(&text("https://example.com", Category::Link), None, 2, 2, 100).unwrap();
        h.add(&text("50%_off", Category::Text), None, 3, 3, 100).unwrap();
        let page = h.query("meeting", None, 0, 10).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.counts.get("text"), Some(&1));
        assert_eq!(h.query("", Some("link"), 0, 10).unwrap().items.len(), 1);
        // LIKE 的萬用字元要被跳脫
        assert_eq!(h.query("%_", None, 0, 10).unwrap().total, 1);
        assert_eq!(h.query("notes", None, 0, 10).unwrap().total, 1);
    }

    #[test]
    fn secrets_are_never_searchable_or_listed_in_plaintext() {
        let h = History::in_memory();
        let secret = Entry {
            category: Category::Secret,
            payload: Payload::Sensitive { reason: SensitiveReason::Entropy },
            text: Some("Tr0ub4dor&3x!".into()),
            paths: Vec::new(),
        };
        h.add(&secret, None, 9, 1, 100).unwrap();
        assert_eq!(h.query("Tr0ub", None, 0, 10).unwrap().total, 0);
        let page = h.query("", None, 0, 10).unwrap();
        assert!(page.items[0].text.is_none());
        assert!(page.items[0].can_copy);
        let id = page.items[0].id;
        assert_eq!(h.content(id).unwrap(), Some(Content::Text("Tr0ub4dor&3x!".into())));
        h.delete_category(Category::Secret).unwrap();
        assert_eq!(h.query("", None, 0, 10).unwrap().total, 0);
    }

    #[test]
    fn files_keep_local_paths_and_images_cannot_be_copied_back() {
        let h = History::in_memory();
        let files = Entry {
            category: Category::Files,
            payload: Payload::Generic { label: "files".into() },
            text: None,
            paths: vec!["/tmp/a.pdf".into(), "/tmp/b.pdf".into()],
        };
        let image = Entry {
            category: Category::Image,
            payload: Payload::Image { width: 2, height: 2, format: "PNG".into(), thumb: None },
            text: None,
            paths: Vec::new(),
        };
        h.add(&files, None, 1, 1, 100).unwrap();
        h.add(&image, None, 2, 2, 100).unwrap();
        let page = h.query("a.pdf", None, 0, 10).unwrap();
        assert_eq!(page.items[0].paths, ["/tmp/a.pdf", "/tmp/b.pdf"]);
        assert_eq!(
            h.content(page.items[0].id).unwrap(),
            Some(Content::Files(vec!["/tmp/a.pdf".into(), "/tmp/b.pdf".into()]))
        );
        let img = h.query("", Some("image"), 0, 10).unwrap();
        assert!(!img.items[0].can_copy);
        assert_eq!(h.content(img.items[0].id).unwrap(), None);
    }

    #[test]
    fn huge_text_is_truncated_and_not_copyable() {
        let h = History::in_memory();
        let big = "字".repeat(MAX_TEXT_BYTES); // 3 bytes each
        h.add(&text(&big, Category::Text), None, 1, 1, 100).unwrap();
        let page = h.query("", None, 0, 10).unwrap();
        assert!(!page.items[0].can_copy);
        assert_eq!(page.items[0].text.as_ref().unwrap().chars().count(), LIST_TEXT_CHARS as usize);
        assert_eq!(h.content(page.items[0].id).unwrap(), None);
    }
}
