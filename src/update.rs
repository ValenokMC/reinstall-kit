//! "Update the kit": copies everything selected into every available place.
//! Only new and changed files are copied; previous versions and deleted files go to
//! "_versions\<date>" and are removed after settings.keepVersionsDays days.
use crate::lang::fmt_size;
use crate::out::{Msg, Out};
use crate::scan::EDITOR_CLIS;
use crate::settings::{app_dir, item_title, Item, Settings, KIT_FOLDER};
use crate::sync::{sync_dir, Stats};
use crate::{sys, tr};
use chrono::Local;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn pad(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len > n {
        s.chars().take(n - 1).collect::<String>() + "…"
    } else {
        format!("{}{}", s, " ".repeat(n - len))
    }
}

pub fn journal(text: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(app_dir().join("journal.txt")) {
        let _ = writeln!(f, "{} {}", Local::now().format("%Y-%m-%d %H:%M"), text);
    }
}
pub fn last_journal_line(kind: &str) -> Option<String> {
    let text = fs::read_to_string(app_dir().join("journal.txt")).ok()?;
    text.lines().rev().find(|l| l.contains(kind)).map(|s| s.to_string())
}

pub fn log_file(prefix: &str, dry: bool) -> PathBuf {
    let dir = app_dir().join("logs");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{}_{}{}.log", prefix, Local::now().format("%Y-%m-%d_%H%M"), if dry { "_dry" } else { "" }))
}
pub fn append_log(log: &Path, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log) {
        for l in lines {
            let _ = writeln!(f, "{}", l);
        }
    }
}

fn write_text(path: &Path, text: &str) {
    if let Some(p) = path.parent() {
        let _ = fs::create_dir_all(p);
    }
    let _ = fs::write(path, text);
}

fn csv(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Table of installed programs from the registry (includes what winget does not know about).
fn installed_programs_csv() -> String {
    use winreg::enums::*;
    let mut rows: Vec<(String, String, String, String)> = vec![];
    for (hive, path) in [
        (HKEY_LOCAL_MACHINE, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
    ] {
        let Ok(key) = winreg::RegKey::predef(hive).open_subkey(path) else { continue };
        for name in key.enum_keys().flatten() {
            let Ok(sub) = key.open_subkey(&name) else { continue };
            let get = |v: &str| sub.get_value::<String, _>(v).unwrap_or_default();
            let display = get("DisplayName");
            if display.is_empty() || sub.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 {
                continue;
            }
            rows.push((display, get("DisplayVersion"), get("Publisher"), get("InstallLocation")));
        }
    }
    rows.sort_by_key(|r| r.0.to_lowercase());
    rows.dedup_by_key(|r| r.0.to_lowercase());
    let mut s = String::from("\u{feff}\"DisplayName\",\"DisplayVersion\",\"Publisher\",\"InstallLocation\"\r\n");
    for r in rows {
        s += &format!("{},{},{},{}\r\n", csv(&r.0), csv(&r.1), csv(&r.2), csv(&r.3));
    }
    s
}

fn safe_name(s: &str) -> String {
    s.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect()
}

fn utf16_bytes(s: &str) -> Vec<u8> {
    let mut v = vec![0xff, 0xfe];
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v
}

fn age_days(p: &Path) -> Option<f64> {
    let m = fs::metadata(p).ok()?.modified().ok()?;
    Some(m.elapsed().ok()?.as_secs_f64() / 86400.0)
}

/// Lists and system things, written into <kit>\inventory.
fn save_inventory(s: &Settings, items: &[&Item], kit: &Path, enc: bool, out: &Out) {
    let dir = kit.join("inventory");
    let _ = fs::create_dir_all(&dir);
    let inv = &s.inventory;
    let mut done: Vec<String> = vec![];
    if inv.programs {
        if sys::has_tool("winget") {
            let f = dir.join("winget-export.json");
            sys::run("winget", &["export", "-o", &f.to_string_lossy(), "--accept-source-agreements", "--disable-interactivity"]);
        }
        write_text(&dir.join("installed-programs.csv"), &installed_programs_csv());
        done.push(tr!("inv.programs"));
    }
    if inv.extensions {
        for cli in EDITOR_CLIS {
            if let Some((0, text)) = sys::run(cli, &["--list-extensions"]) {
                if !text.trim().is_empty() {
                    write_text(&dir.join(format!("extensions-{}.txt", cli)), &text);
                }
            }
        }
        done.push(tr!("inv.extensions"));
    }
    if inv.packages {
        if let Some((0, text)) = sys::run("pip", &["list", "--format=freeze", "--disable-pip-version-check"]) {
            write_text(&dir.join("pip-global.txt"), &text);
        }
        if let Some((_, text)) = sys::run("npm", &["ls", "-g", "--depth=0", "--json"]) {
            if text.trim_start().starts_with('{') {
                write_text(&dir.join("npm-global.json"), &text);
            }
        }
        done.push(tr!("inv.packages"));
    }
    if inv.tasks {
        let td = dir.join("tasks");
        let _ = fs::remove_dir_all(&td);
        let _ = fs::create_dir_all(&td);
        for (name, xml) in sys::user_tasks() {
            let _ = fs::write(td.join(format!("{}.xml", safe_name(&name))), utf16_bytes(&xml));
        }
        done.push(tr!("inv.tasks"));
    }
    if inv.startup {
        let sd = dir.join("startup");
        let _ = fs::remove_dir_all(&sd);
        let _ = fs::create_dir_all(&sd);
        if let Some(st) = sys::known_folder("Startup") {
            if let Ok(rd) = fs::read_dir(st) {
                for e in rd.flatten() {
                    let name = e.file_name();
                    if e.path().is_file() && name.to_string_lossy().to_lowercase() != "desktop.ini" {
                        let _ = fs::copy(e.path(), sd.join(name));
                    }
                }
            }
        }
        let reg = dir.join("startup-registry.reg");
        sys::run("reg", &["export", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", &reg.to_string_lossy(), "/y"]);
        done.push(tr!("inv.startup"));
    }
    // How the system was laid out - for reference; the kit never changes these things itself.
    let mut layout = String::from("Drives:\r\n");
    for d in sys::drives() {
        layout += &format!("  {} {:<14} {:>8.0} GB, free {:>8.0} GB{}\r\n", d.root, d.label, d.total as f64 / 1073741824.0, d.free as f64 / 1073741824.0, if d.enc { ", BitLocker" } else { "" });
    }
    layout += "Profile folders:\r\n";
    for k in ["Desktop", "MyDocuments", "MyPictures", "MyVideos", "MyMusic", "Downloads"] {
        layout += &format!("  {:<12} = {}\r\n", k, sys::known_folder(k).unwrap_or_default());
    }
    layout += &format!("WSL: {}\r\n", sys::wsl_distros().join(", "));
    layout += &format!("Computer: {}\r\n", std::env::var("COMPUTERNAME").unwrap_or_default());
    write_text(&dir.join("system-layout.txt"), &layout);

    // State of git repositories in project folders: branch, last commit, uncommitted changes.
    if sys::has_tool("git") {
        let mut rows = String::from("\u{feff}\"Repo\",\"Branch\",\"Commit\",\"Dirty\",\"Remote\"\r\n");
        let mut any = false;
        for m in items.iter().filter(|m| m.group == "projects") {
            let p = sys::item_path(m);
            let mut list = vec![];
            if Path::new(&p).join(".git").exists() {
                list.push(p.clone());
            }
            if let Ok(rd) = fs::read_dir(&p) {
                for e in rd.flatten() {
                    if e.path().join(".git").exists() {
                        list.push(e.path().to_string_lossy().into_owned());
                    }
                }
            }
            for r in list {
                let g = |args: &[&str]| sys::run("git", &[&["-C", r.as_str()], args].concat()).map(|(_, o)| o.trim().to_string()).unwrap_or_default();
                let dirty = g(&["status", "--porcelain"]).lines().count();
                rows += &format!("{},{},{},{},{}\r\n", csv(&r), csv(&g(&["rev-parse", "--abbrev-ref", "HEAD"])), csv(&g(&["log", "-1", "--format=%h %ci %s"])), dirty, csv(&g(&["remote", "get-url", "origin"])));
                any = true;
            }
        }
        if any {
            write_text(&dir.join("git-repos-state.csv"), &rows);
        }
    }
    if inv.wsl {
        // Home folders of each Linux distribution, once a week.
        for distro in sys::wsl_distros() {
            let safe: String = distro.chars().map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
            let tgz = kit.join("wsl").join(format!("{}-home.tgz", safe));
            if age_days(&tgz).map(|a| a < 7.0).unwrap_or(false) {
                continue;
            }
            let tmp = PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join("ReinstallKit");
            let _ = fs::create_dir_all(&tmp);
            let _ = fs::create_dir_all(kit.join("wsl"));
            let t = tmp.join(format!("{}-home.tgz", safe));
            let ts = t.to_string_lossy().into_owned();
            let lin = format!("/mnt/{}{}", ts[..1].to_lowercase(), ts[2..].replace('\\', "/"));
            let ok = sys::run_ok("wsl", &["-d", &distro, "-u", "root", "--", "tar", "-czf", &lin, "-C", "/home", "."]);
            if ok && fs::metadata(&t).map(|m| m.len() > 0).unwrap_or(false) && (fs::rename(&t, &tgz).is_ok() || fs::copy(&t, &tgz).is_ok()) {
                out.line(tr!("upd.wsl.ok", distro));
            } else {
                out.line(tr!("upd.wsl.fail", distro));
            }
            let _ = fs::remove_file(&t);
        }
    }
    if inv.wifi {
        if s.secrets_only_encrypted && !enc {
            out.line(tr!("upd.wifi.enc"));
        } else {
            let wd = dir.join("wifi");
            let _ = fs::remove_dir_all(&wd);
            let _ = fs::create_dir_all(&wd);
            let folder = format!("folder={}", wd.to_string_lossy());
            if sys::is_elevated() {
                sys::run("netsh", &["wlan", "export", "profile", "key=clear", &folder]);
                done.push(tr!("inv.wifi"));
            } else {
                sys::run("netsh", &["wlan", "export", "profile", &folder]);
                out.line(tr!("upd.wifi.nopass"));
            }
        }
    }
    if inv.drivers {
        let dd = kit.join("drivers");
        if age_days(&dd.join("_exported.txt")).map(|a| a < 30.0).unwrap_or(false) {
            // exported recently
        } else if sys::is_elevated() {
            let _ = fs::create_dir_all(&dd);
            sys::run("pnputil", &["/export-driver", "*", &dd.to_string_lossy()]);
            write_text(&dd.join("_exported.txt"), &Local::now().format("%Y-%m-%d").to_string());
            done.push(tr!("inv.drivers"));
        } else {
            out.line(tr!("upd.drivers.admin"));
        }
    }
    if !done.is_empty() {
        out.line(tr!("upd.inventory", done.join(", ")));
    }
}

/// The kit carries its own copy of the program, so it can restore on a clean Windows with nothing installed.
fn copy_program_into(kit: &Path) {
    let dir = kit.join("_kit");
    let _ = fs::create_dir_all(&dir);
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::copy(&exe, dir.join("ReinstallKit.exe"));
    }
    for f in ["settings.json", "catalog.json"] {
        let p = app_dir().join(f);
        if p.exists() {
            let _ = fs::copy(&p, dir.join(f));
        }
    }
    write_text(&kit.join("Open Reinstall Kit.cmd"), "@echo off\r\nstart \"\" \"%~dp0_kit\\ReinstallKit.exe\"\r\n");
}

/// Old versions and old logs are removed, so nothing piles up.
fn prune(kit: &Path, keep_days: i64) {
    let limit = (Local::now().date_naive() - chrono::Duration::days(keep_days)).format("%Y-%m-%d").to_string();
    if let Ok(rd) = fs::read_dir(kit.join("_versions")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_date = name.len() == 10 && name.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
            if is_date && name < limit {
                let _ = fs::remove_dir_all(crate::sync::long(&e.path()));
            }
        }
    }
    if let Ok(rd) = fs::read_dir(app_dir().join("logs")) {
        for e in rd.flatten() {
            if age_days(&e.path()).map(|a| a > 60.0).unwrap_or(false) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Exit codes: 0 done, 1 done with errors, 2 nothing to do or not set up, 3 skipped (automatic run).
pub fn run(dry: bool, auto: bool, out: Out) -> i32 {
    let code = run_inner(dry, auto, &out);
    out.send(Msg::Done(code));
    code
}

fn run_inner(dry: bool, auto: bool, out: &Out) -> i32 {
    let Some(s) = Settings::load(&crate::settings::settings_path()) else {
        if !auto {
            out.line(tr!("upd.notconfigured"));
        }
        return if auto { 3 } else { 2 };
    };
    let items = s.active_items();
    let dests = s.dests();
    if dests.is_empty() || items.is_empty() {
        if !auto {
            out.line(tr!("upd.notconfigured"));
        }
        return if auto { 3 } else { 2 };
    }
    let Some(_lock) = sys::single_instance() else {
        if !auto {
            out.line(tr!("upd.running"));
        }
        return if auto { 3 } else { 2 };
    };

    // Which places are available right now.
    let mut targets: Vec<PathBuf> = vec![];
    for d in &dests {
        if !sys::drive_there(d) {
            if !auto {
                out.line(tr!("upd.dest.missing", d));
            }
            continue;
        }
        let kit = Path::new(d).join(KIT_FOLDER);
        if auto && age_days(&kit.join("_updated.txt")).map(|a| a * 24.0 < 20.0).unwrap_or(false) {
            continue; // updated less than 20 hours ago
        }
        targets.push(kit);
    }
    if targets.is_empty() {
        if !auto {
            out.line(tr!("upd.nowhere"));
        }
        return if auto { 3 } else { 2 };
    }

    let stamp = Local::now().format("%Y-%m-%d").to_string();
    let log = log_file("update", dry);
    let steps = targets.len() * (items.len() + 2);
    let mut step = 0;
    let mut any_err = 0u64;
    let mut toast: Vec<String> = vec![];

    for kit in &targets {
        let started = std::time::Instant::now();
        let kit_s = kit.to_string_lossy().into_owned();
        let place = kit.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let enc = sys::is_encrypted(&kit_s);
        if !dry {
            let _ = fs::create_dir_all(kit);
        }
        out.line("");
        out.line(format!("{}{}{}", tr!("upd.kit", kit_s), if enc { format!("  {}", tr!("upd.kit.enc")) } else { String::new() }, if dry { format!("   {}", tr!("upd.dry")) } else { String::new() }));
        let mut total = Stats::default();
        let mut rows: Vec<String> = vec![];

        for m in &items {
            let title = item_title(m);
            step += 1;
            out.progress(step, steps, &title);
            let name = pad(&title, 35);
            if m.secret && s.secrets_only_encrypted && !enc {
                let r = format!("{} {}", name, tr!("upd.row.enc"));
                out.line(r.clone());
                rows.push(r);
                continue;
            }
            let src = sys::item_path(m);
            if src.is_empty() || !Path::new(&src).is_dir() {
                let r = format!("{} {}", name, tr!("upd.row.missing"));
                out.line(r.clone());
                rows.push(r);
                continue;
            }
            let mut ex = m.exclude.clone();
            if ["personal", "projects", "custom"].contains(&m.group.as_str()) {
                ex.extend(s.exclude_always.iter().cloned());
            }
            if s.secrets_only_encrypted && !enc {
                ex.extend(m.secret_files.iter().cloned());
            }
            let mut errs: Vec<String> = vec![];
            let st = sync_dir(Path::new(&src), &kit.join(&m.dest), &kit.join("_versions").join(&stamp).join(&m.dest), &ex, &m.files, dry, &mut errs);
            append_log(&log, &errs);
            total.copied += st.copied;
            total.bytes += st.bytes;
            total.versioned += st.versioned;
            total.errors += st.errors;
            let r = format!("{} {}", name, tr!("upd.row", format!("{:>6}", st.copied), format!("{:>9}", fmt_size(st.bytes)), format!("{:>5}", st.versioned), st.errors));
            out.line(r.clone());
            rows.push(r);
        }

        step += 1;
        out.progress(step, steps, &tr!("upd.step.inventory"));
        if !dry {
            save_inventory(&s, &items, kit, enc, out);
        }
        step += 1;
        out.progress(step, steps, &tr!("upd.step.finish"));
        if !dry {
            copy_program_into(kit);
            prune(kit, s.keep_days());
        }

        let free = sys::root_of(&kit_s).and_then(|r| sys::free_space(&r)).map(|f| f.0).unwrap_or(0);
        let mins = (started.elapsed().as_secs_f64() / 60.0).round() as u64;
        let mut sum = tr!("upd.sum", place, total.copied, fmt_size(total.bytes), total.versioned, total.errors, mins, fmt_size(free));
        if dry {
            sum = format!("{} {}", sum, tr!("upd.dry"));
        }
        out.line(format!("{} {}", tr!("upd.total"), sum));
        if total.errors > 0 {
            out.line(tr!("upd.errhint", log.display()));
        }
        journal(&format!("update: {}", sum));
        any_err += total.errors;
        toast.push(if total.errors > 0 { tr!("toast.line.err", place, total.errors) } else { tr!("toast.line.ok", place, total.copied) });
        if !dry {
            let text = format!("{}\r\n{}\r\n\r\n{}\r\n", Local::now().format("%Y-%m-%d %H:%M"), sum, rows.join("\r\n"));
            write_text(&kit.join("_updated.txt"), &text);
        }
    }

    if auto && !dry && s.notify {
        out.send(Msg::Toast(if any_err > 0 { tr!("toast.err") } else { tr!("toast.ok") }, toast.join("; ")));
    }
    if any_err > 0 {
        1
    } else {
        0
    }
}
