//! "Restore after reinstalling": brings folders, settings, programs and schedules back from a kit.
//! Files are only ever added or replaced, never deleted. System-level things (moving profile folders
//! to another drive, page file, built-in apps) are deliberately not touched.
use crate::out::{Msg, Out};
use crate::settings::{item_title, Item, Settings};
use crate::sync::copy_dir;
use crate::update::{append_log, journal, log_file};
use crate::{sys, tr};
use chrono::{DateTime, Local};
use std::fs;
use std::path::Path;

pub const STEPS: [&str; 10] = ["folders", "apps", "secrets", "programs", "extensions", "packages", "tasks", "startup", "wifi", "drivers"];

fn step_of(m: &Item) -> &'static str {
    match m.group.as_str() {
        "secrets" => "secrets",
        "personal" | "projects" | "custom" => "folders",
        _ => "apps",
    }
}

fn read_utf16_or_utf8(p: &Path) -> String {
    let b = fs::read(p).unwrap_or_default();
    if b.len() >= 2 && b[0] == 0xff && b[1] == 0xfe {
        let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(&b).trim_start_matches('\u{feff}').to_string()
    }
}

/// Removes every <UserId>...</UserId>: the task is then registered for whoever restores it
/// (on the old Windows the user had a different internal id).
fn strip_user_id(xml: &str) -> String {
    let mut s = xml.to_string();
    while let (Some(a), Some(b)) = (s.find("<UserId>"), s.find("</UserId>")) {
        if b < a {
            break;
        }
        s.replace_range(a..b + "</UserId>".len(), "");
    }
    s
}

pub fn run(kit: String, dry: bool, steps: Vec<String>, out: Out) -> i32 {
    let code = run_inner(&kit, dry, &steps, &out);
    out.send(Msg::Done(code));
    code
}

fn run_inner(kit_s: &str, dry: bool, steps: &[String], out: &Out) -> i32 {
    let kit = Path::new(kit_s);
    let settings = Settings::load(&kit.join("_kit").join("settings.json"));
    let (Some(s), true) = (settings, kit.join("files").is_dir()) else {
        out.line(tr!("rst.nokit", kit_s));
        return 2;
    };
    let on = |name: &str| steps.iter().any(|x| x == name);
    let inv = kit.join("inventory");
    let log = log_file("restore", dry);
    let mut errors = 0u64;
    let items = s.active_items();
    let total = items.len() + 7;
    let mut step = 0;
    let head = |t: String| {
        out.line("");
        out.line(t);
    };
    let skip = |t: String| out.line(format!("  {}", tr!("rst.skip", t)));
    let say = |t: String| out.line(format!("  {}", t));

    out.line(format!("{}{}", tr!("rst.kit", kit_s), if dry { format!("   {}", tr!("rst.dry")) } else { String::new() }));
    if let Ok(w) = fs::metadata(kit.join("_updated.txt")).and_then(|m| m.modified()) {
        out.line(tr!("rst.kit.date", DateTime::<Local>::from(w).format("%d.%m.%Y %H:%M")));
    }

    // ---- Folders, program settings, secrets ----
    for name in ["folders", "apps", "secrets"] {
        let list: Vec<&&Item> = items.iter().filter(|m| step_of(m) == name).collect();
        if list.is_empty() {
            continue;
        }
        head(tr!(&format!("rst.h.{}", name)));
        if !on(name) {
            skip(tr!("rst.notselected"));
            step += list.len();
            continue;
        }
        for m in list {
            let title = item_title(m);
            step += 1;
            out.progress(step, total, &title);
            let src = kit.join(&m.dest);
            if !src.is_dir() {
                say(tr!("rst.item.absent", title));
                continue;
            }
            let mut dst = sys::item_path(m);
            if !sys::drive_there(&dst) {
                // The drive this folder lived on does not exist here: put it inside the user profile instead.
                let leaf = Path::new(&m.dest).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                dst = format!(r"{}\Restored\{}", std::env::var("USERPROFILE").unwrap_or_default(), leaf);
                say(tr!("rst.item.moved", title, dst));
            }
            let mut errs = vec![];
            // Personal and project folders: never overwrite a file that is newer on this computer.
            let st = copy_dir(&src, Path::new(&dst), name == "folders", dry, &mut errs);
            append_log(&log, &errs);
            errors += st.errors;
            let mut msg = if dry { tr!("rst.item.would", title, dst, st.copied) } else { tr!("rst.item.done", title, dst, st.copied) };
            if st.errors > 0 {
                msg = format!("{}  {}", msg, tr!("rst.item.err", st.errors));
            }
            say(msg);
        }
    }

    // ---- Programs ----
    head(tr!("rst.h.programs"));
    step += 1;
    out.progress(step, total, &tr!("rst.h.programs"));
    let wg = inv.join("winget-export.json");
    if !on("programs") {
        skip(tr!("rst.notselected"));
    } else if !wg.exists() {
        say(tr!("rst.programs.none"));
    } else {
        let ids: Vec<String> = fs::read_to_string(&wg)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')).ok())
            .map(|v| {
                v["Sources"].as_array().into_iter().flatten()
                    .flat_map(|s| s["Packages"].as_array().cloned().unwrap_or_default())
                    .filter_map(|p| p["PackageIdentifier"].as_str().map(|x| x.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if !sys::has_tool("winget") {
            say(tr!("rst.programs.nowinget"));
        } else if dry {
            say(tr!("rst.programs.would", ids.len()));
            out.line(format!("    {}", ids.join(", ")));
        } else {
            say(tr!("rst.programs.start", ids.len()));
            sys::run("winget", &["import", "-i", &wg.to_string_lossy(), "--accept-package-agreements", "--accept-source-agreements", "--ignore-unavailable", "--disable-interactivity"]);
            say(tr!("rst.programs.done"));
        }
        let csv = inv.join("installed-programs.csv");
        if csv.exists() {
            say(tr!("rst.programs.csv", csv.display()));
        }
    }

    // ---- Editor extensions ----
    head(tr!("rst.h.extensions"));
    step += 1;
    out.progress(step, total, &tr!("rst.h.extensions"));
    if !on("extensions") {
        skip(tr!("rst.notselected"));
    } else {
        let mut any = false;
        for e in fs::read_dir(&inv).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(cli) = name.strip_prefix("extensions-").and_then(|n| n.strip_suffix(".txt")) else { continue };
            any = true;
            let ext: Vec<String> = fs::read_to_string(e.path()).unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            if !sys::has_tool(cli) {
                say(tr!("rst.ext.nocli", cli, ext.len()));
            } else if dry {
                say(tr!("rst.ext.would", cli, ext.len()));
            } else {
                let bad = ext.iter().filter(|x| !sys::run_ok(cli, &["--install-extension", x.as_str(), "--force"])).count();
                say(tr!("rst.ext.done", cli, ext.len() - bad, bad));
            }
        }
        if !any {
            say(tr!("rst.none"));
        }
    }

    // ---- Global packages ----
    head(tr!("rst.h.packages"));
    step += 1;
    out.progress(step, total, &tr!("rst.h.packages"));
    if !on("packages") {
        skip(tr!("rst.notselected"));
    } else {
        let mut any = false;
        if let Ok(text) = fs::read_to_string(inv.join("pip-global.txt")) {
            any = true;
            let names: Vec<String> = text.lines().map(|l| l.split(['=', '@', ' ']).next().unwrap_or("").trim().to_string())
                .filter(|n| !n.is_empty() && !["pip", "setuptools", "wheel"].contains(&n.as_str())).collect();
            if !sys::has_tool("pip") {
                say(tr!("rst.pkg.notool", "pip", names.len()));
            } else if dry {
                say(tr!("rst.pkg.would", "pip", names.len()));
            } else {
                let mut args = vec!["install", "--disable-pip-version-check", "-q"];
                args.extend(names.iter().map(|n| n.as_str()));
                let code = sys::run("pip", &args).map(|r| r.0).unwrap_or(-1);
                say(tr!("rst.pkg.done", "pip", names.len(), code));
            }
        }
        if let Ok(text) = fs::read_to_string(inv.join("npm-global.json")) {
            any = true;
            let names: Vec<String> = serde_json::from_str::<serde_json::Value>(&text).ok()
                .and_then(|v| v["dependencies"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()))
                .unwrap_or_default().into_iter().filter(|n| n != "npm" && n != "corepack").collect();
            if !sys::has_tool("npm") {
                say(tr!("rst.pkg.notool", "npm", names.len()));
            } else if dry || names.is_empty() {
                say(tr!("rst.pkg.would", "npm", names.len()));
            } else {
                let mut args = vec!["install", "-g"];
                args.extend(names.iter().map(|n| n.as_str()));
                let code = sys::run("npm", &args).map(|r| r.0).unwrap_or(-1);
                say(tr!("rst.pkg.done", "npm", names.len(), code));
            }
        }
        if !any {
            say(tr!("rst.none"));
        }
    }

    // ---- Scheduled tasks ----
    head(tr!("rst.h.tasks"));
    step += 1;
    out.progress(step, total, &tr!("rst.h.tasks"));
    if !on("tasks") {
        skip(tr!("rst.notselected"));
    } else {
        let mut any = false;
        for e in fs::read_dir(inv.join("tasks")).into_iter().flatten().flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            let Some(name) = file.strip_suffix(".xml") else { continue };
            any = true;
            if sys::task_state(name).is_some() {
                say(tr!("rst.task.exists", name));
            } else if dry {
                say(tr!("rst.task.would", name));
            } else {
                match sys::task_register(name, &strip_user_id(&read_utf16_or_utf8(&e.path()))) {
                    Ok(()) => say(tr!("rst.task.done", name)),
                    Err(err) => {
                        errors += 1;
                        say(tr!("rst.task.fail", name, err));
                    }
                }
            }
        }
        if !any {
            say(tr!("rst.none"));
        }
    }

    // ---- Startup shortcuts ----
    head(tr!("rst.h.startup"));
    step += 1;
    out.progress(step, total, &tr!("rst.h.startup"));
    if !on("startup") {
        skip(tr!("rst.notselected"));
    } else {
        let startup = sys::known_folder("Startup").unwrap_or_default();
        let mut any = false;
        for e in fs::read_dir(inv.join("startup")).into_iter().flatten().flatten() {
            any = true;
            let name = e.file_name().to_string_lossy().into_owned();
            let target = Path::new(&startup).join(&name);
            if target.exists() {
                say(tr!("rst.startup.exists", name));
            } else if dry {
                say(tr!("rst.startup.would", name));
            } else if fs::copy(e.path(), &target).is_ok() {
                say(tr!("rst.startup.done", name));
            } else {
                errors += 1;
            }
        }
        if !any {
            say(tr!("rst.none"));
        }
        let reg = inv.join("startup-registry.reg");
        if reg.exists() {
            say(tr!("rst.startup.reg", reg.display()));
        }
    }

    // ---- Optional: Wi-Fi networks and drivers ----
    head(tr!("rst.h.optional"));
    step += 1;
    out.progress(step, total, &tr!("rst.h.optional"));
    let wd = inv.join("wifi");
    if wd.is_dir() {
        let profiles: Vec<_> = fs::read_dir(&wd).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".xml")).collect();
        if !on("wifi") {
            skip(tr!("rst.wifi.off", profiles.len()));
        } else if dry {
            say(tr!("rst.wifi.would", profiles.len()));
        } else {
            for p in &profiles {
                sys::run("netsh", &["wlan", "add", "profile", &format!("filename={}", p.path().to_string_lossy()), "user=current"]);
            }
            say(tr!("rst.wifi.done", profiles.len()));
        }
    }
    let dd = kit.join("drivers");
    if dd.is_dir() {
        if !on("drivers") {
            skip(tr!("rst.drivers.off"));
        } else if dry {
            say(tr!("rst.drivers.would", fs::read_dir(&dd).map(|r| r.flatten().filter(|e| e.path().is_dir()).count()).unwrap_or(0)));
        } else if !sys::is_elevated() {
            say(tr!("rst.drivers.admin"));
        } else {
            sys::run("pnputil", &["/add-driver", &format!(r"{}\*.inf", dd.to_string_lossy()), "/subdirs", "/install"]);
            say(tr!("rst.drivers.done"));
        }
    }
    for e in fs::read_dir(kit.join("wsl")).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().ends_with(".tgz") {
            say(tr!("rst.wsl", e.path().display()));
        }
    }
    let layout = inv.join("system-layout.txt");
    if layout.exists() {
        say(tr!("rst.layout", layout.display()));
    }

    out.progress(total, total, &tr!("chk.done"));
    out.line("");
    if dry {
        out.line(tr!("rst.sum.dry"));
    } else if errors > 0 {
        out.line(tr!("rst.sum.err", errors, log.display()));
        journal(&format!("restore: errors {}", errors));
    } else {
        out.line(tr!("rst.sum.ok"));
        journal("restore: done");
    }
    if errors > 0 {
        1
    } else {
        0
    }
}
