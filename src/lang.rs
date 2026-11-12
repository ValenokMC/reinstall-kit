//! Texts. Both languages are compiled into the program from assets/lang/*.json.
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::RwLock;

static TABLE: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

const RU: &str = include_str!("../assets/lang/ru.json");
const EN: &str = include_str!("../assets/lang/en.json");

/// `choice` is "auto", "ru" or "en". "auto" means Russian when either the Windows display
/// language or the regional format is Russian, otherwise English.
pub fn init(choice: &str) {
    let lang = match choice {
        "ru" | "en" => choice,
        _ => {
            if crate::sys::system_is_russian() {
                "ru"
            } else {
                "en"
            }
        }
    };
    let text = if lang == "ru" { RU } else { EN };
    let map: HashMap<String, String> = serde_json::from_str(text).unwrap_or_default();
    *TABLE.write().unwrap() = Some(map);
}

pub fn has(key: &str) -> bool {
    TABLE.read().unwrap().as_ref().map(|m| m.contains_key(key)).unwrap_or(false)
}

pub fn t(key: &str) -> String {
    TABLE.read().unwrap().as_ref().and_then(|m| m.get(key).cloned()).unwrap_or_else(|| key.to_string())
}

/// Replaces {0}, {1}, ... with the arguments.
pub fn tf(key: &str, args: &[&dyn Display]) -> String {
    let mut s = t(key);
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{}}}", i), &a.to_string());
    }
    s
}

#[macro_export]
macro_rules! tr {
    ($key:expr) => { $crate::lang::t($key) };
    ($key:expr, $($arg:expr),+ $(,)?) => { $crate::lang::tf($key, &[$(&$arg as &dyn std::fmt::Display),+]) };
}

pub fn fmt_size(b: u64) -> String {
    let b = b as f64;
    if b >= 1073741824.0 {
        format!("{:.1} {}", b / 1073741824.0, t("u.gb")).replace('.', &t("u.dec"))
    } else if b >= 1048576.0 {
        format!("{:.0} {}", b / 1048576.0, t("u.mb"))
    } else if b > 0.0 && b < 1024.0 {
        format!("1 {}", t("u.kb"))
    } else {
        format!("{:.0} {}", b / 1024.0, t("u.kb"))
    }
}
