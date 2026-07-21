use std::{collections::HashMap, sync::OnceLock};

use crate::domain::{LanguagePreference, QualityState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Ko,
    Ja,
    ZhCn,
    ZhTw,
}

static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
static KO: OnceLock<HashMap<String, String>> = OnceLock::new();
static JA: OnceLock<HashMap<String, String>> = OnceLock::new();
static ZH_CN: OnceLock<HashMap<String, String>> = OnceLock::new();
static ZH_TW: OnceLock<HashMap<String, String>> = OnceLock::new();

pub fn active_language(preference: LanguagePreference) -> Language {
    let system_locale = sys_locale::get_locale();
    resolve_language(preference, system_locale.as_deref())
}

pub fn resolve_language(preference: LanguagePreference, system_locale: Option<&str>) -> Language {
    match preference {
        LanguagePreference::En => Language::En,
        LanguagePreference::Ko => Language::Ko,
        LanguagePreference::Ja => Language::Ja,
        LanguagePreference::ZhCn => Language::ZhCn,
        LanguagePreference::ZhTw => Language::ZhTw,
        LanguagePreference::Auto => system_locale
            .and_then(resolve_system_locale)
            .unwrap_or(Language::En),
    }
}

pub fn resolve_system_locale(locale: &str) -> Option<Language> {
    let normalized = locale.trim().replace('_', "-").to_lowercase();
    let mut parts = normalized.split('-');
    match parts.next()? {
        "en" => Some(Language::En),
        "ko" => Some(Language::Ko),
        "ja" => Some(Language::Ja),
        "zh" => {
            let parts = parts.collect::<Vec<_>>();
            if parts
                .iter()
                .any(|part| matches!(*part, "hant" | "tw" | "hk" | "mo"))
            {
                Some(Language::ZhTw)
            } else {
                Some(Language::ZhCn)
            }
        }
        _ => None,
    }
}

pub fn text(language: Language, key: &str) -> String {
    catalog(language)
        .get(key)
        .or_else(|| catalog(Language::En).get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

pub fn message(language: Language, key: &str, values: &[(&str, &str)]) -> String {
    let mut result = text(language, key);
    for (name, value) in values {
        result = result.replace(&format!("{{{name}}}"), value);
    }
    result
}

pub fn state_label(language: Language, state: QualityState) -> String {
    let key = match state {
        QualityState::Stable => "state.stable",
        QualityState::Unstable => "state.unstable",
        QualityState::Disconnected => "state.disconnected",
        QualityState::Paused => "state.paused",
        QualityState::Unobserved => "state.unobserved",
        QualityState::Error => "state.error",
        QualityState::WarmingUp => "state.warmingUp",
    };
    text(language, key)
}

pub fn target_count(language: Language, count: usize) -> String {
    let key = if count == 1 {
        "tray.targetCount.one"
    } else {
        "tray.targetCount.other"
    };
    message(language, key, &[("count", &count.to_string())])
}

fn catalog(language: Language) -> &'static HashMap<String, String> {
    match language {
        Language::En => EN.get_or_init(|| parse(include_str!("../../src/locales/en.json"))),
        Language::Ko => KO.get_or_init(|| parse(include_str!("../../src/locales/ko.json"))),
        Language::Ja => JA.get_or_init(|| parse(include_str!("../../src/locales/ja.json"))),
        Language::ZhCn => ZH_CN.get_or_init(|| parse(include_str!("../../src/locales/zh-CN.json"))),
        Language::ZhTw => ZH_TW.get_or_init(|| parse(include_str!("../../src/locales/zh-TW.json"))),
    }
}

fn parse(json: &str) -> HashMap<String, String> {
    serde_json::from_str(json).expect("embedded translation catalog must be valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_supported_system_locales_and_english_fallback() {
        assert_eq!(resolve_system_locale("ko_KR"), Some(Language::Ko));
        assert_eq!(resolve_system_locale("ja-JP"), Some(Language::Ja));
        assert_eq!(resolve_system_locale("zh-Hans-CN"), Some(Language::ZhCn));
        assert_eq!(resolve_system_locale("zh-HK"), Some(Language::ZhTw));
        assert_eq!(resolve_system_locale("fr-FR"), None);
        assert_eq!(
            resolve_language(LanguagePreference::Auto, Some("fr-FR")),
            Language::En
        );
    }

    #[test]
    fn embedded_catalogs_have_the_same_keys() {
        let mut expected = catalog(Language::En).keys().collect::<Vec<_>>();
        expected.sort();
        for language in [Language::Ko, Language::Ja, Language::ZhCn, Language::ZhTw] {
            let mut actual = catalog(language).keys().collect::<Vec<_>>();
            actual.sort();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn formats_native_messages() {
        assert_eq!(target_count(Language::En, 1), "1 target");
        assert_eq!(target_count(Language::Ko, 2), "대상 2개");
        assert_eq!(
            message(Language::Ja, "notification.recovered", &[("name", "DNS")]),
            "DNS の接続が復旧しました"
        );
    }
}
