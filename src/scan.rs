//! Looks at this computer and suggests what to save. Read-only.
use crate::out::{Msg, Out};
use crate::settings::{catalog, Item, Settings, KIT_FOLDER};
use crate::sync::measure;
use crate::sys;
use crate::tr;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize)]
pub struct Found {
    pub item: Item,
    pub full: String,
    pub bytes: u64,
    pub partial: bool,
    pub def: bool,
    pub count: usize,
    pub gone: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct InvInfo {
    pub id: String,
    pub available: bool,
    pub def: bool,
    pub note: String,
}
#[derive(Clone, Debug, Serialize, Default)]
pub struct ScanResult {
    pub items: Vec<Found>,
    pub inventory: Vec<InvInfo>,
    pub drives: Vec<sys::Drive>,
    pub projects_inside: usize,
    pub elevated: bool,
}

pub const EDITOR_CLIS: [&str; 5] = ["code", "code-insiders", "cursor", "codium", "windsurf"];

fn new_item(id: &str, group: &str, title: &str, path: &str, dest: String) -> Item {
    Item { id: id.into(), group: group.into(), title: title.into(), path: path.into(), dest, enabled: true, ..Default::default() }
}

fn under(path: &str, roots: &[String]) -> bool {
    let p = path.to_lowercase();
    roots.iter().any(|r| {
        let r = r.to_lowercase();
        p == r || p.starts_with(&(r + "\\"))
    })
}

/// Folders that look like backups or archives (English and Russian names) are not projects.
fn looks_like_backup(name: &str) -> bool {
    let n = name.to_lowercase();
    n == "backup" || n == "backups" || n == "archive" || n == "archives"
        || n.starts_with("бэкап") || n.starts_with("резерв") || n.starts_with("архив") || n.starts_with("копия")
}
fn looks_like_date(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 10 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[5..7].iter().all(u8::is_ascii_digit) && b[7] == b'-'
}

/// Breadth-first search for folders holding a ".git" folder.
fn find_repos(root: &str, max_depth: usize, deadline: Instant, skip_top: Option<&str>, dest_roots: &[String], found: &mut Vec<String>) {
    const SKIP: [&str; 18] = ["windows", "program files", "program files (x86)", "programdata", "system volume information", "appdata", "node_modules",
        "recovery", "perflogs", "msocache", "config.msi", "onedrivetemp", "_versions", "reinstallkit", "vendor", "site-packages", "steamapps", "steamlibrary"];
    let mut q: VecDeque<(String, usize)> = VecDeque::new();
    q.push_back((root.trim_end_matches('\\').to_string(), 0));
    while let Some((dir, depth)) = q.pop_front() {
        if Instant::now() > deadline {
            break;
        }
        let Ok(rd) = std::fs::read_dir(format!("{}\\", dir)) else { continue };
        let mut subs = vec![];
        let (mut is_repo, mut is_kit) = (false, false);
        for ent in rd.flatten() {
            let Ok(md) = ent.metadata() else { continue };
            if !md.is_dir() {
                continue;
            }
            let name = ent.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                is_repo = true;
            } else if name == "_versions" || name == "_kit" {
                is_kit = true; // a kit made by this program is a copy, not a project
            }
            if md.file_attributes() & 0x400 == 0 {
                subs.push(name);
            }
        }
        if is_kit {
            continue;
        }
        if is_repo && depth > 0 {
            found.push(dir);
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        for name in subs {
            let low = name.to_lowercase();
            if SKIP.contains(&low.as_str()) || name.starts_with('.') || name.starts_with('$') || looks_like_backup(&name) || looks_like_date(&name) {
                continue;
            }
            if depth == 0 && skip_top.map(|s| s.eq_ignore_ascii_case(&name)).unwrap_or(false) {
                continue;
            }
            let full = format!("{}\\{}", dir, name);
            if under(&full, dest_roots) {
                continue;
            }
            q.push_back((full, depth + 1));
        }
    }
}

pub fn scan(settings: Option<&Settings>, out: &Out) -> ScanResult {
    let mut res = ScanResult { elevated: sys::is_elevated(), ..Default::default() };

    // ---- Personal folders, at their real location ----
    out.status(tr!("scan.personal"));
    let mut personal_on: Vec<String> = vec![];
    for (id, known, def) in [("desktop", "Desktop", true), ("documents", "MyDocuments", true), ("pictures", "MyPictures", true),
                             ("videos", "MyVideos", false), ("music", "MyMusic", false), ("downloads", "Downloads", false)] {
        let Some(full) = sys::known_folder(known) else { continue };
        if !Path::new(&full).is_dir() {
            continue;
        }
        let (bytes, partial) = measure(Path::new(&full), 2500);
        if def {
            personal_on.push(full.trim_end_matches('\\').to_string());
        }
        let mut item = new_item(id, "personal", &tr!(&format!("item.{}", id)), &sys::tokenize(&full), format!("files\\personal\\{}", known));
        item.known = known.into();
        res.items.push(Found { item, full, bytes, partial, def, count: 0, gone: false });
    }

    // ---- Project folders: look for git repositories ----
    out.status(tr!("scan.projects"));
    let dest_roots: Vec<String> = settings.map(|s| s.dests().iter().map(|d| d.trim_end_matches('\\').to_string()).collect()).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(25);
    let drives = sys::drives();
    let mut repos: Vec<String> = vec![];
    for d in drives.iter().filter(|d| !d.removable) {
        find_repos(&d.root, 4, deadline, if d.system { Some("Users") } else { None }, &dest_roots, &mut repos);
    }
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    if !profile.is_empty() {
        find_repos(&profile, 4, deadline, None, &dest_roots, &mut repos);
    }
    repos.sort_by_key(|r| r.to_lowercase());
    repos.dedup_by_key(|r| r.to_lowercase());
    res.projects_inside = repos.iter().filter(|r| under(r, &personal_on)).count();
    repos.retain(|r| !under(r, &personal_on));
    // Several projects in one folder are offered as that one folder.
    let mut by_parent: HashMap<String, Vec<String>> = HashMap::new();
    for r in &repos {
        let parent = Path::new(r).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        by_parent.entry(parent).or_default().push(r.clone());
    }
    let mut cands: Vec<(String, usize)> = vec![];
    for (parent, list) in by_parent {
        let is_root = parent.trim_end_matches('\\').len() <= 2 || parent.eq_ignore_ascii_case(&profile);
        if list.len() >= 2 && !is_root {
            cands.push((parent, list.len()));
        } else {
            cands.extend(list.into_iter().map(|r| (r, 1)));
        }
    }
    cands.sort_by_key(|c| c.0.to_lowercase());
    let all: Vec<String> = cands.iter().map(|c| c.0.clone()).collect();
    let mut used: HashSet<String> = HashSet::new();
    for (path, count) in cands {
        let others: Vec<String> = all.iter().filter(|a| **a != path).cloned().collect();
        if under(&path, &others) {
            continue;
        }
        let leaf = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
        let mut dn = leaf.clone();
        let mut i = 2;
        while !used.insert(dn.to_lowercase()) {
            dn = format!("{} ({})", leaf, i);
            i += 1;
        }
        let (bytes, partial) = measure(Path::new(&path), 1000);
        let item = new_item(&format!("proj:{}", path), "projects", &leaf, &sys::tokenize(&path), format!("files\\projects\\{}", dn));
        res.items.push(Found { item, full: path, bytes, partial, def: true, count, gone: false });
    }

    // ---- Settings of known programs, from the catalog ----
    out.status(tr!("scan.apps"));
    for a in catalog() {
        let full = sys::expand(&a.path);
        let (bytes, partial);
        if !a.files.is_empty() {
            let have: Vec<u64> = a.files.iter().filter_map(|f| std::fs::metadata(Path::new(&full).join(f)).ok()).filter(|m| m.is_file()).map(|m| m.len()).collect();
            if have.is_empty() {
                continue;
            }
            bytes = have.iter().sum();
            partial = false;
        } else {
            if !Path::new(&full).is_dir() {
                continue;
            }
            (bytes, partial) = measure(Path::new(&full), 400);
        }
        let is_secret = a.group == "secrets";
        let dest = if is_secret { format!("files\\secrets\\{}", a.id) } else { format!("files\\apps\\{}", a.id) };
        let mut item = new_item(&a.id, &a.group, &a.name, &a.path, dest);
        item.exclude = a.exclude.clone();
        item.files = a.files.clone();
        item.secret = is_secret || a.secret;
        item.secret_files = a.secret_files.clone();
        res.items.push(Found { item, full, bytes, partial, def: a.default.unwrap_or(true), count: 0, gone: false });
    }

    // ---- Lists and system things ----
    out.status(tr!("scan.system"));
    let inv = |id: &str, available: bool, def: bool, note: String| InvInfo { id: id.into(), available, def, note };
    let winget = sys::has_tool("winget");
    res.inventory.push(inv("programs", true, true, if winget { tr!("scan.winget.yes") } else { tr!("scan.winget.no") }));
    let clis: Vec<&str> = EDITOR_CLIS.iter().copied().filter(|c| sys::has_tool(c)).collect();
    res.inventory.push(inv("extensions", !clis.is_empty(), !clis.is_empty(), clis.join(", ")));
    let pk: Vec<&str> = ["pip", "npm"].into_iter().filter(|c| sys::has_tool(c)).collect();
    res.inventory.push(inv("packages", !pk.is_empty(), !pk.is_empty(), pk.join(", ")));
    res.inventory.push(inv("tasks", true, true, tr!("scan.count", sys::user_tasks().len())));
    let startup = sys::known_folder("Startup").unwrap_or_default();
    let sc = std::fs::read_dir(&startup).map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase() != "desktop.ini").count()).unwrap_or(0);
    res.inventory.push(inv("startup", true, true, tr!("scan.count", sc)));
    let wsl = sys::wsl_distros();
    res.inventory.push(inv("wsl", !wsl.is_empty(), !wsl.is_empty(), wsl.join(", ")));
    let wifi = sys::run_ok("netsh", &["wlan", "show", "profiles"]);
    res.inventory.push(inv("wifi", wifi, false, tr!("scan.wifi.note")));
    res.inventory.push(inv("drivers", true, false, tr!("scan.drivers.note")));

    res.drives = drives;
    let _ = KIT_FOLDER;
    res
}

/// Runs the scan and sends the result to the window.
pub fn run(settings: Option<Settings>, out: Out) {
    let r = scan(settings.as_ref(), &out);
    out.send(Msg::Scan(Box::new(r)));
    out.send(Msg::Done(0));
}
