//! User-facing language: Spanish or English, taken from the OS locale.
//!
//! `OPEN_AMPP_LANG=es|en` overrides the detection (useful for tests and screenshots).

use std::sync::OnceLock;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Es,
    En,
}

static LANG: OnceLock<Lang> = OnceLock::new();

pub fn lang() -> Lang {
    *LANG.get_or_init(detect)
}

fn detect() -> Lang {
    let locale = std::env::var("OPEN_AMPP_LANG").ok().or_else(sys_locale::get_locale).unwrap_or_default();
    if locale.to_lowercase().starts_with("es") { Lang::Es } else { Lang::En }
}

/// Formats a message in the user's language: `tr!("Hola {name}", "Hello {name}")`.
/// Both literals accept the usual `format!` syntax, including inline captures.
#[macro_export]
macro_rules! tr {
    ($es:literal, $en:literal $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Es => format!($es),
            $crate::i18n::Lang::En => format!($en),
        }
    };
    ($es:literal, $en:literal, $($arg:tt)+) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Es => format!($es, $($arg)+),
            $crate::i18n::Lang::En => format!($en, $($arg)+),
        }
    };
}
