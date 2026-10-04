//! Locale identity, negotiation, and system detection.
//!
//! `en` is the source of truth: every message exists in English first, and any
//! locale (or individual message) without a translation falls back to it.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A user-interface language the catalog knows about.
///
/// A locale being recognized does not mean it is translated; see
/// [`Locale::coverage`] and [`Locale::available`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Locale {
    #[default]
    En,
    Es,
    Fr,
    De,
    PtBr,
    ZhHans,
    Ja,
    Ru,
    Hi,
}

/// Direction in which a locale's text flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection {
    LeftToRight,
    RightToLeft,
}

/// The CLDR plural categories that integer counts can select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PluralCategory {
    One,
    Few,
    Many,
    Other,
}

impl PluralCategory {
    /// The suffix used by plural keys in a locale table, for example `other`.
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::One => "one",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
        }
    }
}

impl Locale {
    /// Every recognized locale, source language first.
    pub const ALL: &'static [Locale] = &[
        Locale::En,
        Locale::Es,
        Locale::Fr,
        Locale::De,
        Locale::PtBr,
        Locale::ZhHans,
        Locale::Ja,
        Locale::Ru,
        Locale::Hi,
    ];

    /// The language every message is authored in and every lookup falls back to.
    pub const SOURCE: Locale = Locale::En;

    /// BCP 47 tag, as persisted in preferences.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Es => "es",
            Self::Fr => "fr",
            Self::De => "de",
            Self::PtBr => "pt-BR",
            Self::ZhHans => "zh-Hans",
            Self::Ja => "ja",
            Self::Ru => "ru",
            Self::Hi => "hi",
        }
    }

    /// The language's own name, suitable for a language picker.
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Es => "Español",
            Self::Fr => "Français",
            Self::De => "Deutsch",
            Self::PtBr => "Português (Brasil)",
            Self::ZhHans => "简体中文",
            Self::Ja => "日本語",
            Self::Ru => "Русский",
            Self::Hi => "हिन्दी",
        }
    }

    pub const fn english_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Es => "Spanish",
            Self::Fr => "French",
            Self::De => "German",
            Self::PtBr => "Portuguese (Brazil)",
            Self::ZhHans => "Chinese (Simplified)",
            Self::Ja => "Japanese",
            Self::Ru => "Russian",
            Self::Hi => "Hindi",
        }
    }

    pub const fn is_source(self) -> bool {
        matches!(self, Self::En)
    }

    /// No shipped locale is right-to-left yet; adapters should still consult
    /// this instead of assuming left-to-right.
    pub const fn direction(self) -> TextDirection {
        TextDirection::LeftToRight
    }

    /// True when the script uses double-width glyphs in a terminal, so layout
    /// cannot count `char`s as columns.
    pub const fn uses_wide_characters(self) -> bool {
        matches!(self, Self::ZhHans | Self::Ja)
    }

    /// The non-`other` plural categories integer counts select in this locale.
    pub const fn plural_categories(self) -> &'static [PluralCategory] {
        match self {
            Self::En | Self::Es | Self::Fr | Self::De | Self::PtBr | Self::Hi => {
                &[PluralCategory::One]
            }
            Self::Ru => &[
                PluralCategory::One,
                PluralCategory::Few,
                PluralCategory::Many,
            ],
            Self::ZhHans | Self::Ja => &[],
        }
    }

    /// CLDR plural category for an integer count.
    ///
    /// Spanish, French, and Portuguese also have a `many` category for exact
    /// multiples of one million; it is not modeled and falls back to `other`.
    pub fn plural_category(self, count: u64) -> PluralCategory {
        match self {
            Self::En | Self::Es | Self::De => {
                if count == 1 {
                    PluralCategory::One
                } else {
                    PluralCategory::Other
                }
            }
            Self::Fr | Self::PtBr | Self::Hi => {
                if count <= 1 {
                    PluralCategory::One
                } else {
                    PluralCategory::Other
                }
            }
            Self::Ru => {
                let tens = count % 100;
                let units = count % 10;
                if units == 1 && tens != 11 {
                    PluralCategory::One
                } else if (2..=4).contains(&units) && !(12..=14).contains(&tens) {
                    PluralCategory::Few
                } else {
                    PluralCategory::Many
                }
            }
            Self::ZhHans | Self::Ja => PluralCategory::Other,
        }
    }

    /// Parses a POSIX locale name (`pt_BR.UTF-8`), a BCP 47 tag (`zh-Hans-CN`),
    /// or a bare language (`fr`). Returns `None` for unsupported languages and
    /// for the `C`/`POSIX` locales, which express no language preference.
    ///
    /// Regional variants map to the nearest shipped locale: any `pt` becomes
    /// `pt-BR`, and `zh` becomes `zh-Hans` unless the script or region says
    /// Traditional (`Hant`, `TW`, `HK`, `MO`).
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        let value = value.split(['.', '@']).next().unwrap_or(value);
        let mut parts = value.split(['-', '_']).filter(|part| !part.is_empty());
        let language = parts.next()?.to_ascii_lowercase();
        let rest = parts
            .map(|part| part.to_ascii_lowercase())
            .collect::<Vec<_>>();
        match language.as_str() {
            "en" => Some(Self::En),
            "es" => Some(Self::Es),
            "fr" => Some(Self::Fr),
            "de" => Some(Self::De),
            "pt" => Some(Self::PtBr),
            "ja" => Some(Self::Ja),
            "ru" => Some(Self::Ru),
            "hi" => Some(Self::Hi),
            "zh" => {
                let traditional = rest
                    .iter()
                    .any(|part| matches!(part.as_str(), "hant" | "tw" | "hk" | "mo"));
                (!traditional).then_some(Self::ZhHans)
            }
            _ => None,
        }
    }

    /// The first supported locale in an ordered preference list.
    pub fn negotiate<I, S>(requested: I) -> Option<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        requested
            .into_iter()
            .find_map(|candidate| Self::parse(candidate.as_ref()))
    }

    /// The locale to render with: an explicit override, else the system locale,
    /// else English.
    pub fn resolve(explicit: Option<Self>) -> Self {
        explicit
            .or_else(Self::detect_system)
            .unwrap_or(Self::SOURCE)
    }

    /// Detects the user's language from the process environment (and, on
    /// macOS when no variable is set, the `AppleLanguages` global default).
    ///
    /// On Windows only the POSIX-style variables are consulted because the OS
    /// language setting cannot be read without FFI; the explicit
    /// `Preferences::language` override covers that gap.
    pub fn detect_system() -> Option<Self> {
        Self::detect_with(|name| std::env::var(name).ok()).or_else(|| {
            if cfg!(target_os = "macos") {
                macos_apple_languages().and_then(Self::negotiate)
            } else {
                None
            }
        })
    }

    /// Environment-based detection with an injectable variable source.
    ///
    /// Precedence follows gettext: the effective locale is the first non-empty
    /// of `LC_ALL`, `LC_MESSAGES`, `LANG`; when it names `C`/`POSIX` no
    /// preference is expressed. Otherwise the colon-separated `LANGUAGE` list
    /// is tried before the effective locale itself.
    pub fn detect_with(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let value = |name: &str| get(name).filter(|value| !value.trim().is_empty());
        let effective = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(value)?;
        let name = effective.trim();
        if name.eq_ignore_ascii_case("c")
            || name.eq_ignore_ascii_case("posix")
            || name.starts_with("C.")
        {
            return None;
        }
        let language_list = value("LANGUAGE").unwrap_or_default();
        language_list
            .split(':')
            .find_map(Self::parse)
            .or_else(|| Self::parse(name))
    }
}

/// `/usr/bin/defaults read -g AppleLanguages`, parsed. Fixed argv; no shell involved.
fn macos_apple_languages() -> Option<Vec<String>> {
    let output = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let languages = parse_apple_languages(&String::from_utf8_lossy(&output.stdout));
    (!languages.is_empty()).then_some(languages)
}

/// Parses the property-list array `defaults` prints, for example
/// `(\n    "fr-FR",\n    en\n)`.
pub(crate) fn parse_apple_languages(output: &str) -> Vec<String> {
    output
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(|item| item.trim().trim_matches('"').trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

impl fmt::Display for Locale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.tag())
    }
}

impl FromStr for Locale {
    type Err = UnsupportedLocale;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| UnsupportedLocale(value.to_owned()))
    }
}

/// Returned when a language tag does not map to any recognized locale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedLocale(String);

impl fmt::Display for UnsupportedLocale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported language `{}`", self.0)
    }
}

impl std::error::Error for UnsupportedLocale {}

impl Serialize for Locale {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.tag())
    }
}

impl<'de> Deserialize<'de> for Locale {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let tag = String::deserialize(deserializer)?;
        tag.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn parses_posix_and_bcp47_names() {
        assert_eq!(Locale::parse("en_US.UTF-8"), Some(Locale::En));
        assert_eq!(Locale::parse("pt_BR.UTF-8"), Some(Locale::PtBr));
        assert_eq!(Locale::parse("pt-PT"), Some(Locale::PtBr));
        assert_eq!(Locale::parse("zh_CN.UTF-8"), Some(Locale::ZhHans));
        assert_eq!(Locale::parse("zh-Hans-CN"), Some(Locale::ZhHans));
        assert_eq!(Locale::parse("zh"), Some(Locale::ZhHans));
        assert_eq!(Locale::parse("zh_TW"), None);
        assert_eq!(Locale::parse("zh-Hant"), None);
        assert_eq!(Locale::parse("de_DE@euro"), Some(Locale::De));
        assert_eq!(Locale::parse("FR"), Some(Locale::Fr));
        assert_eq!(Locale::parse("C"), None);
        assert_eq!(Locale::parse("POSIX"), None);
        assert_eq!(Locale::parse(""), None);
        assert_eq!(Locale::parse("tlh"), None);
    }

    #[test]
    fn tags_round_trip_through_parse_and_serde() {
        for locale in Locale::ALL {
            assert_eq!(Locale::parse(locale.tag()), Some(*locale));
            let json = serde_json::to_string(locale).expect("serialize");
            assert_eq!(
                serde_json::from_str::<Locale>(&json).expect("parse"),
                *locale
            );
        }
        assert!(serde_json::from_str::<Locale>("\"tlh\"").is_err());
    }

    #[test]
    fn negotiation_takes_the_first_supported_preference() {
        assert_eq!(
            Locale::negotiate(["ko-KR", "fr-CA", "en"]),
            Some(Locale::Fr)
        );
        assert_eq!(Locale::negotiate(["ko", "sw"]), None);
        assert_eq!(Locale::resolve(Some(Locale::Ja)), Locale::Ja);
    }

    #[test]
    fn detection_follows_gettext_precedence() {
        assert_eq!(
            Locale::detect_with(env(&[("LANG", "de_DE.UTF-8")])),
            Some(Locale::De)
        );
        assert_eq!(
            Locale::detect_with(env(&[("LANG", "de_DE.UTF-8"), ("LC_ALL", "ja_JP.UTF-8")])),
            Some(Locale::Ja)
        );
        assert_eq!(
            Locale::detect_with(env(&[("LANG", "en_US.UTF-8"), ("LC_MESSAGES", "es_ES")])),
            Some(Locale::Es)
        );
        // LANGUAGE refines a real locale, skipping unsupported entries.
        assert_eq!(
            Locale::detect_with(env(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "ko:ru:en")])),
            Some(Locale::Ru)
        );
        // The C locale disables LANGUAGE and expresses no preference.
        assert_eq!(
            Locale::detect_with(env(&[("LANG", "C"), ("LANGUAGE", "fr")])),
            None
        );
        assert_eq!(Locale::detect_with(env(&[("LANG", "C.UTF-8")])), None);
        assert_eq!(Locale::detect_with(env(&[("LANG", "")])), None);
        assert_eq!(Locale::detect_with(env(&[])), None);
        // Unsupported system language: no preference, so callers use English.
        assert_eq!(Locale::detect_with(env(&[("LANG", "ko_KR.UTF-8")])), None);
    }

    #[test]
    fn apple_languages_output_is_parsed() {
        let output = "(\n    \"fr-FR\",\n    en,\n    \"zh-Hans-CN\"\n)\n";
        let languages = parse_apple_languages(output);
        assert_eq!(languages, ["fr-FR", "en", "zh-Hans-CN"]);
        assert_eq!(Locale::negotiate(&languages), Some(Locale::Fr));
        assert!(parse_apple_languages("()").is_empty());
    }

    #[test]
    fn plural_rules_match_cldr_for_integers() {
        use PluralCategory::*;
        assert_eq!(Locale::En.plural_category(1), One);
        assert_eq!(Locale::En.plural_category(0), Other);
        assert_eq!(Locale::En.plural_category(2), Other);
        assert_eq!(Locale::Fr.plural_category(0), One);
        assert_eq!(Locale::Fr.plural_category(1), One);
        assert_eq!(Locale::Fr.plural_category(2), Other);
        assert_eq!(Locale::Ja.plural_category(1), Other);
        let ru = |n| Locale::Ru.plural_category(n);
        assert_eq!([ru(1), ru(21), ru(101)], [One, One, One]);
        assert_eq!([ru(2), ru(3), ru(4), ru(22), ru(104)], [Few; 5]);
        assert_eq!([ru(0), ru(5), ru(11), ru(12), ru(14), ru(25)], [Many; 6]);
    }

    #[test]
    fn plural_category_sets_match_what_the_rules_can_produce() {
        for locale in Locale::ALL {
            for count in 0..=200 {
                let category = locale.plural_category(count);
                assert!(
                    category == PluralCategory::Other
                        || locale.plural_categories().contains(&category),
                    "{locale} count {count} produced undeclared {category:?}"
                );
            }
        }
    }
}
