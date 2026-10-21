//! 七種介面語言。設定為「跟隨系統」時依作業系統語系決定。
//! Rust 端只負責系統匣選單、視窗標題與預覽範例；其餘文案在前端 shared/i18n.js。

use crate::settings::Language;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Lang {
    #[serde(rename = "zh-Hant")]
    Zh,
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-Hans")]
    ZhHans,
    #[serde(rename = "ja")]
    Ja,
    #[serde(rename = "ko")]
    Ko,
    #[serde(rename = "fr")]
    Fr,
    #[serde(rename = "ms")]
    Ms,
}

/// BCP-47 / POSIX 語系；明確 script 優先於地區，不支援的語系回退英文。
pub fn from_locale(locale: &str) -> Lang {
    let normalized = locale.to_ascii_lowercase().replace('_', "-");
    let base = normalized.split(['.', '@']).next().unwrap_or("");
    let parts: Vec<_> = base.split('-').collect();
    match parts[0] {
        "zh" if parts.contains(&"hant") => Lang::Zh,
        "zh" if parts.contains(&"hans") => Lang::ZhHans,
        "zh" if parts.iter().any(|p| ["cn", "sg", "my"].contains(p)) => Lang::ZhHans,
        "zh" => Lang::Zh,
        "ja" => Lang::Ja,
        "ko" => Lang::Ko,
        "fr" => Lang::Fr,
        "ms" => Lang::Ms,
        _ => Lang::En,
    }
}

pub fn resolve(pref: Language) -> Lang {
    match pref {
        Language::ZhHant => Lang::Zh,
        Language::En => Lang::En,
        Language::ZhHans => Lang::ZhHans,
        Language::Ja => Lang::Ja,
        Language::Ko => Lang::Ko,
        Language::Fr => Lang::Fr,
        Language::Ms => Lang::Ms,
        Language::Auto => sys_locale::get_locale().as_deref().map(from_locale).unwrap_or(Lang::En),
    }
}

/// Rust 端用到的固定字串。
pub struct Strings {
    pub watching: &'static str,
    pub pause: &'static str,
    pub position: &'static str,
    pub positions: [&'static str; 6],
    pub duration: &'static str,
    pub seconds: &'static str,
    pub opacity: &'static str,
    pub sound: &'static str,
    pub privacy: &'static str,
    pub language: &'static str,
    pub lang_auto: &'static str,
    pub try_it: &'static str,
    pub settings: &'static str,
    pub quit: &'static str,
    pub tooltip: &'static str,
    pub settings_title: &'static str,
    pub sample_text: &'static str,
    pub sample_files: &'static str,
    pub sample_file: &'static str,
}

const ZH: Strings = Strings {
    watching: "正在守護你的剪貼簿",
    pause: "暫停提示",
    position: "顯示位置",
    positions: ["螢幕正上方膠囊", "游標旁微氣泡", "左上角", "右上角", "左下角", "右下角"],
    duration: "停留秒數",
    seconds: "秒",
    opacity: "透明度",
    sound: "提示音效",
    privacy: "隱私打碼模式（分享螢幕用）",
    language: "語言 Language",
    lang_auto: "跟隨系統",
    try_it: "試一下提示",
    settings: "偏好設定…",
    quit: "結束",
    tooltip: "複製即時回饋",
    settings_title: "CopyC 偏好設定",
    sample_text: "這是一段測試文字內容，讓你確認 CopyC 正常",
    sample_files: "專案進度報告_v2.docx",
    sample_file: "要求.pdf",
};

const EN: Strings = Strings {
    watching: "is looking after your clipboard",
    pause: "Pause notifications",
    position: "Position",
    positions: ["Top-center pill", "Bubble by the cursor", "Top left", "Top right", "Bottom left", "Bottom right"],
    duration: "Duration",
    seconds: "s",
    opacity: "Opacity",
    sound: "Sound",
    privacy: "Privacy mode (for screen sharing)",
    language: "Language 語言",
    lang_auto: "System default",
    try_it: "Try a notification",
    settings: "Settings…",
    quit: "Quit",
    tooltip: "instant copy feedback",
    settings_title: "CopyC Settings",
    sample_text: "This is a test sentence to check that CopyC works",
    sample_files: "Project report v2.docx",
    sample_file: "Requirements.pdf",
};

include!("i18n_extra.rs");

pub fn strings(lang: Lang) -> &'static Strings {
    match lang {
        Lang::Zh => &ZH,
        Lang::En => &EN,
        Lang::ZhHans => &ZH_HANS,
        Lang::Ja => &JA,
        Lang::Ko => &KO,
        Lang::Fr => &FR,
        Lang::Ms => &MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_locales_map_to_traditional_chinese() {
        for l in ["zh-TW", "zh-Hant-HK", "zh_HK", "zh-Hant-CN"] {
            assert_eq!(from_locale(l), Lang::Zh, "{l}");
        }
    }

    #[test]
    fn other_locales_fall_back_to_english() {
        for l in ["en-US", "de-DE", "zhx", ""] {
            assert_eq!(from_locale(l), Lang::En, "{l}");
        }
    }

    #[test]
    fn explicit_choice_wins_over_system() {
        assert_eq!(resolve(Language::En), Lang::En);
        assert_eq!(resolve(Language::ZhHant), Lang::Zh);
    }

    #[test]
    fn serializes_as_bcp47_tags() {
        assert_eq!(serde_json::to_string(&Lang::Zh).unwrap(), "\"zh-Hant\"");
        assert_eq!(serde_json::to_string(&Lang::En).unwrap(), "\"en\"");
    }

    #[test]
    fn added_locales_resolve_serialize_and_preserve_settings() {
        for (pref, lang, tag, system) in [
            (Language::ZhHans, Lang::ZhHans, "zh-Hans", "ZH_CN.UTF-8"),
            (Language::Ja, Lang::Ja, "ja", "ja_JP"),
            (Language::Ko, Lang::Ko, "ko", "ko-KR"),
            (Language::Fr, Lang::Fr, "fr", "fr-CA"),
            (Language::Ms, Lang::Ms, "ms", "ms-MY"),
        ] {
            assert_eq!(resolve(pref), lang);
            assert_eq!(from_locale(system), lang);
            assert_eq!(serde_json::to_string(&lang).unwrap(), format!("\"{tag}\""));
            let settings = crate::settings::Settings { language: pref, sound: true, ..Default::default() };
            let json = serde_json::to_string(&settings).unwrap();
            let restored: crate::settings::Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(restored.language, pref);
            assert!(restored.sound);
            assert!(!strings(lang).quit.is_empty());
        }
        assert_eq!(from_locale("zh-Hans-TW"), Lang::ZhHans);
        assert_eq!(from_locale("zh-SG"), Lang::ZhHans);
        assert_eq!(from_locale("zh-MO"), Lang::Zh);
    }
}
