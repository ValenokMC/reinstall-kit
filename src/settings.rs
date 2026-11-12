//! settings.json (next to the program) and the catalog of known programs.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const KIT_FOLDER: &str = "ReinstallKit";
pub const GROUPS: [&str; 11] = ["personal", "projects", "custom", "editors", "ai", "make", "dev", "gaming", "other", "browsers", "secrets"];
pub const INV_IDS: [&str; 8] = ["programs", "extensions", "packages", "tasks", "startup", "wsl", "wifi", "drivers"];
pub const JUNK: [&str; 9] = ["node_modules/**", ".venv/**", "venv/**", "__pycache__/**", ".pytest_cache/**", ".mypy_cache/**", "*.pyc", "Thumbs.db", "desktop.ini"];

fn yes() -> bool {
    true
}
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Dest {
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Schedule {
    pub enabled: bool,
    pub time: String,
}
impl Default for Schedule {
    fn default() -> Self {
        Schedule { enabled: true, time: "13:00".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Inventory {
    pub programs: bool,
    pub extensions: bool,
    pub packages: bool,
    pub tasks: bool,
    pub startup: bool,
    pub wsl: bool,
    pub wifi: bool,
    pub drivers: bool,
}
impl Inventory {
    pub fn get(&self, id: &str) -> bool {
        match id {
            "programs" => self.programs,
            "extensions" => self.extensions,
            "packages" => self.packages,
            "tasks" => self.tasks,
            "startup" => self.startup,
            "wsl" => self.wsl,
            "wifi" => self.wifi,
            "drivers" => self.drivers,
            _ => false,
        }
    }
    pub fn set(&mut self, id: &str, v: bool) {
        match id {
            "programs" => self.programs = v,
            "extensions" => self.extensions = v,
            "packages" => self.packages = v,
            "tasks" => self.tasks = v,
            "startup" => self.startup = v,
            "wsl" => self.wsl = v,
            "wifi" => self.wifi = v,
            "drivers" => self.drivers = v,
            _ => {}
        }
    }
}

/// One thing to save: a folder (or a few files from a folder).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub group: String,
    pub title: String,
    /// Where it is on the computer; may use %USERPROFILE%, %APPDATA%, %LOCALAPPDATA%, %DOCUMENTS%.
    pub path: String,
    /// For personal folders: which one (Desktop, MyDocuments, ...), so restore finds its current place.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub known: String,
    /// Folder inside the kit.
    pub dest: String,
    /// Save only these files from the folder.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    pub exclude: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub secret: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub secret_files: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    #[serde(rename = "_help")]
    pub help: String,
    pub version: u32,
    pub language: String,
    pub destinations: Vec<Dest>,
    pub secrets_only_encrypted: bool,
    pub keep_versions_days: u32,
    pub schedule: Schedule,
    pub notify: bool,
    pub exclude_always: Vec<String>,
    pub inventory: Inventory,
    pub items: Vec<Item>,
}
impl Default for Settings {
    fn default() -> Self {
        Settings {
            help: "Reinstall Kit settings. You can edit this file by hand; the Set up button in the program rewrites it. \
                   items: what to save (path = where it is on the computer, dest = folder inside the kit, exclude = patterns to skip, \
                   secret = goes only to an encrypted drive when secretsOnlyEncrypted is true). See README for details."
                .into(),
            version: 1,
            language: "auto".into(),
            destinations: vec![],
            secrets_only_encrypted: true,
            keep_versions_days: 30,
            schedule: Schedule::default(),
            notify: true,
            exclude_always: JUNK.iter().map(|s| s.to_string()).collect(),
            inventory: Inventory::default(),
            items: vec![],
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Option<Settings> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut s: Settings = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
        for it in &mut s.items {
            // "enabled" missing in a hand-written entry means enabled
            if it.id.is_empty() {
                it.id = it.dest.clone();
            }
        }
        Some(s)
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, text.replace('\n', "\r\n") + "\r\n")
    }
    pub fn dests(&self) -> Vec<String> {
        self.destinations.iter().map(|d| d.path.trim().to_string()).filter(|p| !p.is_empty()).collect()
    }
    pub fn active_items(&self) -> Vec<&Item> {
        self.items.iter().filter(|i| i.enabled).collect()
    }
    pub fn keep_days(&self) -> i64 {
        self.keep_versions_days.max(1) as i64
    }
}

pub fn app_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}
pub fn settings_path() -> PathBuf {
    app_dir().join("settings.json")
}

/// One entry of the catalog of known programs.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogApp {
    pub id: String,
    pub name: String,
    pub group: String,
    pub path: String,
    pub exclude: Vec<String>,
    pub files: Vec<String>,
    pub secret: bool,
    pub secret_files: Vec<String>,
    pub default: Option<bool>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct CatalogFile {
    apps: Vec<CatalogApp>,
}

/// The built-in catalog, extended (or overridden by id) with an optional catalog.json next to the program.
pub fn catalog() -> Vec<CatalogApp> {
    let mut apps = serde_json::from_str::<CatalogFile>(include_str!("../assets/catalog.json")).map(|c| c.apps).unwrap_or_default();
    if let Ok(text) = std::fs::read_to_string(app_dir().join("catalog.json")) {
        if let Ok(extra) = serde_json::from_str::<CatalogFile>(text.trim_start_matches('\u{feff}')) {
            for a in extra.apps {
                if let Some(old) = apps.iter_mut().find(|x| x.id == a.id) {
                    *old = a;
                } else {
                    apps.push(a);
                }
            }
        }
    }
    apps
}

pub fn item_title(m: &Item) -> String {
    let key = format!("item.{}", m.id);
    if m.group == "personal" && crate::lang::has(&key) {
        return crate::lang::t(&key);
    }
    if m.title.is_empty() {
        m.id.clone()
    } else {
        m.title.clone()
    }
}
