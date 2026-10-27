//! 智慧內容識別：把剪貼簿快照轉成通知框要呈現的結構化資料。
//!
//! 文案（「你已成功複製……」）統一由前端組字，這裡只負責判斷型別與擷取必要欄位。
//! 敏感內容在這一層就丟棄明文，確保之後的事件拿不到原文；
//! 要不要寫進本機歷史（[`Entry`]）也在這裡依隱私規則決定，密碼預設不保存。

use crate::settings::{BlacklistAction, Settings, SmartTypes};
use regex::Regex;
use serde::Serialize;
use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::OnceLock,
};

pub const TEXT_PREVIEW_CHARS: usize = 25;
const URL_REST_CHARS: usize = 22;

#[derive(Debug, Clone, Default)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub format: String,
    /// 32×32 顯示用縮圖（以 64×64 輸出以支援高 DPI），data URL。
    pub thumb: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub icon: Option<String>,
}

/// 一次剪貼簿變更所讀到的全部資訊。
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// 剪貼簿上的格式數量；0 代表被清空，不應提示。
    pub format_count: usize,
    /// 密碼管理器設定的「勿記錄」標記（macOS ConcealedType、Windows ExcludeClipboardContentFromMonitorProcessing 等）。
    pub sensitive_marker: bool,
    pub files: Vec<FileEntry>,
    pub text: Option<String>,
    pub image: Option<ImageInfo>,
    pub source: Option<crate::platform::AppInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SensitiveReason {
    /// 來源應用設定了隱私標記
    Marker,
    /// 來源應用在黑名單內
    Blacklist,
    /// 內容符合高熵值密碼特徵
    Entropy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Payload {
    Text { preview: String, truncated: bool, chars: usize, lines: usize },
    Image { width: u32, height: u32, format: String, thumb: Option<String> },
    Files { first: String, count: usize, is_dir: bool, ext: String, icon: Option<String> },
    Color { value: String },
    Url { scheme: String, domain: String, rest: String },
    Sensitive { reason: SensitiveReason },
    /// 隱私打碼模式：僅顯示「✓ 你已成功複製」。
    Private,
    /// 對應類型的智慧預覽被關閉，或是無法辨識的格式。label 是類型 key（text / image / files / color / url / content），由前端翻譯。
    Generic { label: String },
}

/// 歷史紀錄的分類，方便查找。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Text,
    Link,
    Email,
    Phone,
    Color,
    Image,
    Files,
    /// 密碼與敏感內容，只有使用者開啟「保存密碼與敏感內容」時才會出現。
    Secret,
}

impl Category {
    /// 可以同步到其他裝置的分類：只有文字類。圖片、檔案與密碼永遠只留在這台電腦。
    pub fn syncable(self) -> bool {
        matches!(self, Category::Text | Category::Link | Category::Email | Category::Phone | Category::Color)
    }

    pub fn key(self) -> &'static str {
        match self {
            Category::Text => "text",
            Category::Link => "link",
            Category::Email => "email",
            Category::Phone => "phone",
            Category::Color => "color",
            Category::Image => "image",
            Category::Files => "files",
            Category::Secret => "secret",
        }
    }
}

/// 要寫進本機歷史的內容。圖片只保留尺寸與縮圖；檔案只記錄本機路徑。
#[derive(Debug, Clone)]
pub struct Entry {
    pub category: Category,
    /// 歷史列表顯示用（不受隱私打碼與智慧預覽開關影響）。
    pub payload: Payload,
    /// 文字類內容的全文，用於搜尋與重新複製。
    pub text: Option<String>,
    pub paths: Vec<PathBuf>,
}

/// 一次可顯示的複製事件。
#[derive(Debug, Clone)]
pub struct Clip {
    pub payload: Payload,
    /// 內容雜湊，用來判斷「連按」是否為同一份內容。
    pub hash: u64,
    pub source: Option<String>,
    /// 要寫進本機歷史的內容；None 代表這次不保存。
    pub entry: Option<Entry>,
    /// 可以即時同步到其他電腦的文字：黑名單、密碼與檔案都不會有。
    pub share: Option<String>,
    /// 這份內容是其他電腦即時同步過來的：那台電腦的名稱（剪貼簿管線填入）。
    pub via: Option<String>,
}

pub enum Outcome {
    Show(Box<Clip>),
    /// 黑名單設定為「完全不提示」。
    Silent,
    /// 剪貼簿被清空或沒有內容，不應誤報。
    Ignore,
}

pub fn classify(snap: Snapshot, settings: &Settings) -> Outcome {
    if snap.format_count == 0 {
        return Outcome::Ignore;
    }
    let hash = content_hash(&snap);
    let source = snap.source.as_ref().map(|a| a.name.clone()).filter(|s| !s.is_empty());

    let blacklisted = snap.source.as_ref().is_some_and(|a| settings.is_blacklisted(a));
    if blacklisted && settings.blacklist_action == BlacklistAction::Silent {
        return Outcome::Silent;
    }

    let payload = if blacklisted {
        Payload::Sensitive { reason: SensitiveReason::Blacklist }
    } else if snap.sensitive_marker && settings.mask_sensitive {
        Payload::Sensitive { reason: SensitiveReason::Marker }
    } else {
        detect(&snap, settings)
    };

    // 黑名單 App 的內容一律不保存
    let entry = (settings.history_enabled && !blacklisted).then(|| history_entry(&snap, settings)).flatten();
    let payload = if settings.privacy_mode { Payload::Private } else { payload };
    let share = snap
        .text
        .as_deref()
        .filter(|_| !blacklisted && !snap.sensitive_marker && snap.files.is_empty())
        .filter(|t| !t.trim().is_empty() && !looks_like_secret(t.trim()))
        .map(str::to_string);
    Outcome::Show(Box::new(Clip { payload, hash, source, entry, share, via: None }))
}

/// 依隱私規則決定這次複製要不要寫進本機歷史。
/// 密碼管理器的標記或高熵值內容視為密碼：預設不保存，使用者開啟「保存密碼與敏感內容」才保存。
fn history_entry(snap: &Snapshot, settings: &Settings) -> Option<Entry> {
    let text = snap.text.as_deref().filter(|t| !t.trim().is_empty());
    let secret = snap.sensitive_marker || text.is_some_and(|t| looks_like_secret(t.trim()));
    if secret && !settings.history_secrets {
        return None;
    }

    if !snap.files.is_empty() {
        return Some(Entry {
            category: Category::Files,
            payload: detect(snap, &all_smart(settings)),
            text: None,
            paths: snap.files.iter().map(|f| f.path.clone()).collect(),
        });
    }
    if let Some(text) = text {
        let (category, payload) = if secret {
            let reason = if snap.sensitive_marker { SensitiveReason::Marker } else { SensitiveReason::Entropy };
            (Category::Secret, Payload::Sensitive { reason })
        } else {
            (text_category(text.trim()), detect(snap, &all_smart(settings)))
        };
        return Some(Entry { category, payload, text: Some(text.to_string()), paths: Vec::new() });
    }
    snap.image.as_ref().map(|_| Entry {
        category: Category::Image,
        payload: detect(snap, &all_smart(settings)),
        text: None,
        paths: Vec::new(),
    })
}

/// 和本機複製同一段純文字時算出的雜湊相同，用來比對其他裝置傳來的內容。
pub fn text_hash(text: &str) -> u64 {
    content_hash(&Snapshot { format_count: 1, text: Some(text.to_string()), ..Default::default() })
}

/// 其他裝置同步來的文字轉成歷史紀錄。看起來像密碼的內容一律不收（傳送端本來就不會送）。
pub fn remote_entry(text: &str) -> Option<Entry> {
    if text.trim().is_empty() || looks_like_secret(text.trim()) {
        return None;
    }
    let snap = Snapshot { format_count: 1, text: Some(text.to_string()), ..Default::default() };
    let settings = all_smart(&Settings::default());
    Some(Entry {
        category: text_category(text.trim()),
        payload: detect(&snap, &settings),
        text: Some(text.to_string()),
        paths: Vec::new(),
    })
}

/// 歷史列表用的設定：不受智慧預覽開關與自動遮罩影響（密碼已在 history_entry 另外判斷）。
fn all_smart(settings: &Settings) -> Settings {
    Settings { smart_types: SmartTypes::default(), mask_sensitive: false, ..settings.clone() }
}

fn text_category(s: &str) -> Category {
    if parse_url(s).is_some() {
        Category::Link
    } else if parse_color(s).is_some() {
        Category::Color
    } else if is_email(s) {
        Category::Email
    } else if is_phone(s) {
        Category::Phone
    } else {
        Category::Text
    }
}

fn is_email(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^(?:mailto:)?[\w.+-]+@[\w-]+(?:\.[\w-]+)+$").unwrap());
    s.len() <= 254 && re.is_match(s)
}

/// 電話號碼：可有 + 與常見分隔符，數字 7–15 位（E.164 上限）。2024-10-18 這類日期不算。
fn is_phone(s: &str) -> bool {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [phone, date] = RE.get_or_init(|| {
        [
            Regex::new(r"^(?:tel:)?\+?[\d\s().-]{7,24}$").unwrap(),
            Regex::new(r"^\d{4}[-/.]\d{1,2}[-/.]\d{1,2}$").unwrap(),
        ]
    });
    let digits = s.chars().filter(char::is_ascii_digit).count();
    phone.is_match(s) && !date.is_match(s) && (7..=15).contains(&digits)
}

fn detect(snap: &Snapshot, settings: &Settings) -> Payload {
    let smart = &settings.smart_types;

    if let Some(first) = snap.files.first() {
        if !smart.files {
            return generic("files");
        }
        let name = first
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| first.path.to_string_lossy().into_owned());
        let ext = first
            .path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        return Payload::Files {
            first: name,
            count: snap.files.len(),
            is_dir: first.path.is_dir(),
            ext,
            icon: first.icon.clone(),
        };
    }

    if let Some(text) = snap.text.as_deref().filter(|t| !t.trim().is_empty()) {
        let token = text.trim();
        if settings.mask_sensitive && looks_like_secret(token) {
            return Payload::Sensitive { reason: SensitiveReason::Entropy };
        }
        if let Some(value) = parse_color(token) {
            return if smart.color { Payload::Color { value } } else { generic("color") };
        }
        if let Some((scheme, domain, rest)) = parse_url(token) {
            return if smart.url { Payload::Url { scheme, domain, rest } } else { generic("url") };
        }
        if !smart.text {
            return generic("text");
        }
        let (preview, truncated) = text_preview(text);
        return Payload::Text {
            preview,
            truncated,
            chars: text.chars().count(),
            lines: text.lines().count().max(1),
        };
    }

    if let Some(img) = &snap.image {
        if !smart.image {
            return generic("image");
        }
        return Payload::Image {
            width: img.width,
            height: img.height,
            format: img.format.clone(),
            thumb: img.thumb.clone(),
        };
    }

    generic("content")
}

fn generic(label: &str) -> Payload {
    Payload::Generic { label: label.to_string() }
}

/// FNV-1a：不同電腦、不同版本算出來的值都一樣，跨裝置同步與刪除才對得起來。
/// （標準函式庫的 DefaultHasher 不保證跨版本一致。）
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

fn content_hash(snap: &Snapshot) -> u64 {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    snap.sensitive_marker.hash(&mut h);
    for f in &snap.files {
        f.path.hash(&mut h);
    }
    snap.text.hash(&mut h);
    if let Some(img) = &snap.image {
        (img.width, img.height, &img.thumb).hash(&mut h);
    }
    h.finish()
}

/// 換行、Tab 轉成空白，擷取前 25 個字元。
pub fn text_preview(text: &str) -> (String, bool) {
    let flat: String = text
        .trim()
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect();
    let mut chars = flat.chars();
    let preview: String = chars.by_ref().take(TEXT_PREVIEW_CHARS).collect();
    let truncated = chars.next().is_some();
    (preview.trim_end().to_string(), truncated)
}

fn color_regexes() -> &'static [Regex; 3] {
    static RE: OnceLock<[Regex; 3]> = OnceLock::new();
    RE.get_or_init(|| {
        [
            Regex::new(r"^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$").unwrap(),
            Regex::new(
                r"(?i)^rgba?\(\s*(\d{1,3})%?\s*[,\s]\s*(\d{1,3})%?\s*[,\s]\s*(\d{1,3})%?\s*(?:[,/]\s*(?:0|1|0?\.\d+|\d{1,3}%)\s*)?\)$",
            )
            .unwrap(),
            Regex::new(
                r"(?i)^hsla?\(\s*\d{1,3}(?:\.\d+)?(?:deg)?\s*[,\s]\s*\d{1,3}(?:\.\d+)?%\s*[,\s]\s*\d{1,3}(?:\.\d+)?%\s*(?:[,/]\s*(?:0|1|0?\.\d+|\d{1,3}%)\s*)?\)$",
            )
            .unwrap(),
        ]
    })
}

/// 識別 HEX / RGB / HSL 色碼；回傳值只會是通過正則的字串，可安全當作 CSS 顏色。
pub fn parse_color(s: &str) -> Option<String> {
    if s.len() > 64 {
        return None;
    }
    let [hex, rgb, hsl] = color_regexes();
    if hex.is_match(s) {
        return Some(s.to_uppercase());
    }
    if let Some(caps) = rgb.captures(s) {
        let ok = (1..=3).all(|i| caps[i].parse::<u16>().is_ok_and(|v| v <= 255));
        return ok.then(|| s.to_string());
    }
    hsl.is_match(s).then(|| s.to_string())
}

/// 識別網址並拆出「協定 / 網域 / 其餘路徑」，供前端高亮網域。
pub fn parse_url(s: &str) -> Option<(String, String, String)> {
    if s.len() > 4096 || s.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let (scheme, body) = if lower.starts_with("https://") || lower.starts_with("http://") {
        let idx = s.find("://")? + 3;
        (s[..idx].to_string(), &s[idx..])
    } else if lower.starts_with("www.") {
        (String::new(), s)
    } else {
        return None;
    };
    let host_end = body.find(['/', '?', '#']).unwrap_or(body.len());
    let host = &body[..host_end];
    let host = host.rsplit('@').next().unwrap_or(host);
    if host.is_empty() || !host.contains('.') && !host.starts_with("localhost") {
        return None;
    }
    let rest = &body[host_end..];
    let rest = if rest.chars().count() > URL_REST_CHARS {
        format!("{}…", rest.chars().take(URL_REST_CHARS).collect::<String>())
    } else {
        rest.to_string()
    };
    Some((scheme, host.to_string(), rest))
}

fn shannon_entropy(s: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    let mut total = 0usize;
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
        total += 1;
    }
    counts
        .values()
        .map(|&n| {
            let p = n as f64 / total as f64;
            -p * p.log2()
        })
        .sum()
}

/// 高熵值密碼 / 金鑰特徵判斷。刻意保守，避免把一般檔名、程式識別字誤判為密碼。
pub fn looks_like_secret(s: &str) -> bool {
    let len = s.chars().count();
    if !(8..=128).contains(&len) || s.chars().any(char::is_whitespace) || !s.is_ascii() {
        return false;
    }
    if parse_url(s).is_some() || parse_color(s).is_some() {
        return false;
    }
    if s.contains('@') && s.contains('.') && !s.starts_with('@') {
        return false; // email
    }
    if s.starts_with('/') || s.starts_with('~') || s.contains(":\\") || s.chars().all(|c| c.is_ascii_digit()) {
        return false; // 路徑或純數字
    }

    let has_upper = s.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = s.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let has_special = s.chars().any(|c| !c.is_ascii_alphanumeric() && !"_-./\\:".contains(c));

    if has_upper && has_lower && has_digit && has_special {
        return true;
    }

    // 長金鑰（API Token、Base64）：字母數字類別頻繁交錯且熵值高。
    let class = |c: char| {
        if c.is_ascii_uppercase() {
            0
        } else if c.is_ascii_lowercase() {
            1
        } else {
            2
        }
    };
    let alnum: Vec<u8> = s.chars().filter(|c| c.is_ascii_alphanumeric()).map(class).collect();
    let transitions = alnum.windows(2).filter(|w| w[0] != w[1]).count();
    len >= 16
        && has_digit
        && (has_upper || has_lower)
        && shannon_entropy(s) >= 3.6
        && transitions as f64 / len as f64 >= 0.3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::AppInfo;

    fn text_snap(t: &str) -> Snapshot {
        Snapshot { format_count: 1, text: Some(t.into()), ..Default::default() }
    }

    fn show(snap: Snapshot, s: &Settings) -> Payload {
        match classify(snap, s) {
            Outcome::Show(c) => c.payload,
            _ => panic!("expected Show"),
        }
    }

    #[test]
    fn empty_clipboard_is_ignored() {
        assert!(matches!(classify(Snapshot::default(), &Settings::default()), Outcome::Ignore));
    }

    #[test]
    fn text_preview_flattens_and_truncates() {
        let text = "這是一段測試文字內容\n第二行的內容在這裡\n第三行會被截斷掉喔喔喔喔喔";
        match show(text_snap(text), &Settings::default()) {
            Payload::Text { preview, truncated, chars, lines } => {
                assert!(!preview.contains('\n'));
                assert_eq!(preview.chars().count(), 25);
                assert!(truncated);
                assert_eq!(chars, text.chars().count());
                assert_eq!(lines, 3);
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn short_text_is_not_truncated() {
        assert_eq!(text_preview("hello\nworld"), ("hello world".into(), false));
    }

    #[test]
    fn detects_colors() {
        assert_eq!(parse_color("#10b981").as_deref(), Some("#10B981"));
        assert!(parse_color("rgb(16, 185, 129)").is_some());
        assert!(parse_color("rgba(16 185 129 / 0.5)").is_some());
        assert!(parse_color("hsl(160, 84%, 39%)").is_some());
        assert!(parse_color("rgb(300, 0, 0)").is_none());
        assert!(parse_color("#12345").is_none());
        assert!(parse_color("10B981").is_none());
    }

    #[test]
    fn detects_urls_and_domain() {
        let (scheme, domain, rest) = parse_url("https://example.com/some/really/long/path?q=1").unwrap();
        assert_eq!(scheme, "https://");
        assert_eq!(domain, "example.com");
        assert!(rest.starts_with("/some"));
        assert!(parse_url("www.google.com").is_some());
        assert!(parse_url("hello world").is_none());
        assert!(parse_url("file.txt").is_none());
    }

    #[test]
    fn secret_heuristics() {
        assert!(looks_like_secret("Tr0ub4dor&3x!"));
        assert!(looks_like_secret("aZ3kP9qL2mX8vB7nW4tR"));
        assert!(!looks_like_secret("Project_Report_2024_Final.docx"));
        assert!(!looks_like_secret("getUserName"));
        assert!(!looks_like_secret("hello"));
        assert!(!looks_like_secret("someone@example.com"));
        assert!(!looks_like_secret("https://example.com/a1B2c3D4"));
        assert!(!looks_like_secret("20241018"));
    }

    #[test]
    fn sensitive_never_carries_plaintext() {
        let p = show(text_snap("Tr0ub4dor&3x!"), &Settings::default());
        assert_eq!(p, Payload::Sensitive { reason: SensitiveReason::Entropy });
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("Tr0ub"));
    }

    #[test]
    fn marker_and_blacklist() {
        let mut snap = text_snap("plain words");
        snap.sensitive_marker = true;
        assert_eq!(show(snap, &Settings::default()), Payload::Sensitive { reason: SensitiveReason::Marker });

        let mut snap = text_snap("plain words");
        snap.source = Some(AppInfo { name: "1Password".into(), id: "com.1password.1password".into() });
        assert_eq!(show(snap.clone(), &Settings::default()), Payload::Sensitive { reason: SensitiveReason::Blacklist });

        let silent = Settings { blacklist_action: BlacklistAction::Silent, ..Default::default() };
        assert!(matches!(classify(snap, &silent), Outcome::Silent));
    }

    #[test]
    fn privacy_mode_hides_everything() {
        let s = Settings { privacy_mode: true, ..Default::default() };
        assert_eq!(show(text_snap("secret meeting notes"), &s), Payload::Private);
    }

    #[test]
    fn files_take_priority_over_text() {
        let snap = Snapshot {
            format_count: 3,
            files: vec![
                FileEntry { path: "/tmp/要求.pdf".into(), icon: None },
                FileEntry { path: "/tmp/報告.docx".into(), icon: None },
            ],
            text: Some("要求.pdf".into()),
            ..Default::default()
        };
        match show(snap, &Settings::default()) {
            Payload::Files { first, count, ext, .. } => {
                assert_eq!(first, "要求.pdf");
                assert_eq!(count, 2);
                assert_eq!(ext, "pdf");
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn disabled_smart_type_falls_back_to_generic() {
        let mut s = Settings::default();
        s.smart_types.color = false;
        assert_eq!(show(text_snap("#FFFFFF"), &s), Payload::Generic { label: "color".into() });
    }

    fn entry(snap: Snapshot, s: &Settings) -> Option<Entry> {
        match classify(snap, s) {
            Outcome::Show(c) => c.entry,
            _ => panic!("expected Show"),
        }
    }

    #[test]
    fn history_categories() {
        let s = Settings::default();
        let cat = |t: &str| entry(text_snap(t), &s).unwrap().category;
        assert_eq!(cat("hello world"), Category::Text);
        assert_eq!(cat("https://example.com/a"), Category::Link);
        assert_eq!(cat("someone@example.com"), Category::Email);
        assert_eq!(cat("+60 12-345 6789"), Category::Phone);
        assert_eq!(cat("2024-10-18"), Category::Text);
        assert_eq!(cat("#10B981"), Category::Color);
    }

    #[test]
    fn history_keeps_full_text_even_when_preview_is_hidden() {
        let long = "第一行\n".repeat(50);
        let s = Settings { privacy_mode: true, ..Default::default() };
        let e = entry(text_snap(&long), &s).unwrap();
        assert_eq!(e.text.as_deref(), Some(long.as_str()));
        assert!(matches!(e.payload, Payload::Text { .. }));
    }

    #[test]
    fn passwords_are_not_saved_unless_allowed() {
        assert!(entry(text_snap("Tr0ub4dor&3x!"), &Settings::default()).is_none());
        let mut marked = text_snap("plain words");
        marked.sensitive_marker = true;
        assert!(entry(marked.clone(), &Settings::default()).is_none());

        let allow = Settings { history_secrets: true, ..Default::default() };
        let e = entry(text_snap("Tr0ub4dor&3x!"), &allow).unwrap();
        assert_eq!(e.category, Category::Secret);
        assert_eq!(e.payload, Payload::Sensitive { reason: SensitiveReason::Entropy });
        assert_eq!(entry(marked, &allow).unwrap().category, Category::Secret);
    }

    #[test]
    fn blacklisted_apps_and_disabled_history_save_nothing() {
        let mut snap = text_snap("plain words");
        snap.source = Some(AppInfo { name: "1Password".into(), id: String::new() });
        let allow = Settings { history_secrets: true, ..Default::default() };
        assert!(entry(snap, &allow).is_none());
        let off = Settings { history_enabled: false, ..Default::default() };
        assert!(entry(text_snap("hello"), &off).is_none());
    }

    #[test]
    fn files_are_saved_as_local_paths() {
        let snap = Snapshot {
            format_count: 1,
            files: vec![FileEntry { path: "/tmp/a.pdf".into(), icon: None }],
            ..Default::default()
        };
        let e = entry(snap, &Settings::default()).unwrap();
        assert_eq!(e.category, Category::Files);
        assert_eq!(e.paths, [PathBuf::from("/tmp/a.pdf")]);
        assert!(e.text.is_none());
    }

    #[test]
    fn same_content_same_hash() {
        let a = content_hash(&text_snap("abc"));
        let b = content_hash(&text_snap("abc"));
        let c = content_hash(&text_snap("abd"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
