//! 使用者設定：以 JSON 存放於系統設定目錄（app_config_dir/settings.json）。

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

/// 通知框顯示位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    /// 模式 A：螢幕正上方居中滑出（類靈動島膠囊）
    Top,
    /// 模式 B：跟隨滑鼠游標右上方彈出微氣泡
    Cursor,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// 全螢幕獨佔（遊戲、影片、簡報）時的行為。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FullscreenAction {
    Pause,
    Dot,
    Normal,
}

/// 黑名單應用程式複製時的行為。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlacklistAction {
    Mask,
    Silent,
}

/// 介面語言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    ZhHant,
    ZhHans,
    En,
    Ja,
    Ko,
    Fr,
    Ms,
    /// 跟隨作業系統語系；未知語言不應導致整份設定重設。
    #[serde(other)]
    Auto,
}

/// 各內容類型是否啟用智慧預覽；關閉時以通用文案呈現。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartTypes {
    pub text: bool,
    pub image: bool,
    pub files: bool,
    pub color: bool,
    pub url: bool,
}

impl Default for SmartTypes {
    fn default() -> Self {
        Self { text: true, image: true, files: true, color: true, url: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub position: Position,
    /// 停留時間（毫秒），範圍 800–3000。
    pub duration_ms: u32,
    /// 通知框不透明度，範圍 0.6–1.0。
    pub opacity: f32,
    pub sound: bool,
    /// 會議分享螢幕用的隱私打碼模式：只顯示「✓ 你已成功複製」。
    pub privacy_mode: bool,
    /// 自動遮罩密碼與高熵值內容。
    pub mask_sensitive: bool,
    pub blacklist: Vec<String>,
    pub blacklist_action: BlacklistAction,
    pub fullscreen_action: FullscreenAction,
    /// 滑鼠懸停於通知框時展開最近 3 筆複製紀錄。
    pub hover_history: bool,
    /// 啟動時向 GitHub 查詢是否有新版本（Microsoft Store 版由商店更新，不檢查）。
    pub check_updates: bool,
    pub smart_types: SmartTypes,
    pub paused: bool,
    pub language: Language,
    /// 把複製紀錄保存在本機資料庫，可以搜尋、分類與重新複製。
    pub history_enabled: bool,
    /// 本機最多保留的筆數，超過時刪除最舊的。
    pub history_limit: u32,
    /// 也保存密碼與敏感內容。預設不保存，必須由使用者明確開啟。
    pub history_secrets: bool,
}

/// 「最多保留筆數」可選的值。
pub const HISTORY_LIMITS: [u32; 4] = [100, 500, 1000, 5000];

pub const DEFAULT_BLACKLIST: &[&str] = &[
    "1Password",
    "Bitwarden",
    "KeePass",
    "KeePassXC",
    "LastPass",
    "Dashlane",
    "Keychain Access",
];

impl Default for Settings {
    fn default() -> Self {
        Self {
            position: Position::Top,
            duration_ms: 1400,
            opacity: 0.96,
            sound: false,
            privacy_mode: false,
            mask_sensitive: true,
            blacklist: DEFAULT_BLACKLIST.iter().map(|s| s.to_string()).collect(),
            blacklist_action: BlacklistAction::Mask,
            fullscreen_action: FullscreenAction::Dot,
            hover_history: true,
            check_updates: true,
            smart_types: SmartTypes::default(),
            paused: false,
            language: Language::Auto,
            history_enabled: true,
            history_limit: 1000,
            history_secrets: false,
        }
    }
}

impl Settings {
    /// 把超出範圍的值夾回合法區間，避免手改設定檔造成異常。
    pub fn sanitized(mut self) -> Self {
        self.duration_ms = self.duration_ms.clamp(800, 3000);
        self.opacity = if self.opacity.is_finite() { self.opacity.clamp(0.6, 1.0) } else { 0.96 };
        self.blacklist = self
            .blacklist
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.blacklist.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        if !HISTORY_LIMITS.contains(&self.history_limit) {
            self.history_limit = 1000;
        }
        self
    }

    pub fn load(path: &PathBuf) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&self, path: &PathBuf) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        fs::rename(tmp, path)
    }

    /// 來源應用是否命中黑名單（不分大小寫，比對名稱或 Bundle ID / 執行檔名）。
    pub fn is_blacklisted(&self, app: &crate::platform::AppInfo) -> bool {
        let hay = [app.name.to_lowercase(), app.id.to_lowercase()];
        self.blacklist.iter().any(|entry| {
            let needle = entry.to_lowercase();
            hay.iter().any(|h| !h.is_empty() && (h == &needle || h.contains(&needle)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::AppInfo;

    #[test]
    fn sanitizes_out_of_range_values() {
        let s = Settings { duration_ms: 99_999, opacity: 0.1, ..Default::default() }.sanitized();
        assert_eq!(s.duration_ms, 3000);
        assert!((s.opacity - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn partial_json_falls_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"position":"cursor"}"#).unwrap();
        assert_eq!(s.position, Position::Cursor);
        assert_eq!(s.duration_ms, 1400);
        assert!(s.smart_types.url);
        assert_eq!(s.language, Language::Auto);
    }

    #[test]
    fn language_uses_kebab_case() {
        let s: Settings = serde_json::from_str(r#"{"language":"zh-hant"}"#).unwrap();
        assert_eq!(s.language, Language::ZhHant);
        let s: Settings = serde_json::from_str(r#"{"language":"en"}"#).unwrap();
        assert_eq!(s.language, Language::En);
        let s: Settings = serde_json::from_str(r#"{"language":"future-locale","sound":true,"durationMs":2000}"#).unwrap();
        assert_eq!(s.language, Language::Auto);
        assert!(s.sound);
        assert_eq!(s.duration_ms, 2000);
    }

    #[test]
    fn blacklist_matches_name_or_id() {
        let s = Settings::default();
        let app = AppInfo { name: "Bitwarden".into(), id: "com.bitwarden.desktop".into() };
        assert!(s.is_blacklisted(&app));
        let app = AppInfo { name: "Notes".into(), id: "com.apple.Notes".into() };
        assert!(!s.is_blacklisted(&app));
    }
}
