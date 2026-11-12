//! "Check": is everything in order? Changes nothing.
use crate::lang::fmt_size;
use crate::out::{Msg, Out};
use crate::settings::{item_title, settings_path, Settings, KIT_FOLDER};
use crate::update::last_journal_line;
use crate::{sys, tr};
use chrono::{DateTime, Local};
use std::path::Path;

pub fn run(out: Out) -> i32 {
    let code = run_inner(&out);
    out.send(Msg::Done(code));
    code
}

fn run_inner(out: &Out) -> i32 {
    let Some(s) = Settings::load(&settings_path()) else {
        out.line(tr!("upd.notconfigured"));
        return 2;
    };
    let items = s.active_items();
    let dests = s.dests();
    if dests.is_empty() {
        out.line(tr!("upd.notconfigured"));
        return 2;
    }
    let (m_ok, m_bad) = (tr!("chk.mark.ok"), tr!("chk.mark.bad"));
    let mut bad = 0;
    let good = |t: String| out.line(format!("  {}  {}", m_ok, t));
    let note = |t: String| out.line(format!("        {}", t));
    macro_rules! bad {
        ($t:expr) => {{
            bad += 1;
            out.line(format!("  {}  {}", m_bad, $t));
        }};
    }
    let only_enc = s.secrets_only_encrypted;
    let secrets: Vec<_> = items.iter().filter(|m| m.secret).collect();
    let steps = dests.len() + 3;
    let mut step = 0;

    out.line(tr!("chk.h.kit"));
    let mut secrets_saved = false;
    for d in &dests {
        step += 1;
        out.progress(step, steps, d);
        if !sys::drive_there(d) {
            note(tr!("chk.dest.offline", d));
            continue;
        }
        let kit = Path::new(d).join(KIT_FOLDER);
        let enc = sys::is_encrypted(d);
        let st = kit.join("_updated.txt");
        let Ok(when) = std::fs::metadata(&st).and_then(|m| m.modified()) else {
            bad!(tr!("chk.dest.never", d));
            continue;
        };
        let age = when.elapsed().map(|e| e.as_secs_f64() / 86400.0).unwrap_or(0.0);
        if age <= 2.0 {
            good(tr!("chk.dest.fresh", d, DateTime::<Local>::from(when).format("%d.%m.%Y %H:%M")));
        } else {
            bad!(tr!("chk.dest.old", d, age as i64));
        }
        let mut missing = vec![];
        for m in &items {
            if m.secret && only_enc && !enc {
                continue;
            }
            if !Path::new(&sys::item_path(m)).is_dir() {
                continue;
            }
            if !kit.join(&m.dest).exists() {
                missing.push(item_title(m));
            }
        }
        if missing.is_empty() {
            good(tr!("chk.dest.complete", d, items.len()));
        } else {
            bad!(tr!("chk.dest.missing", d, missing.join(", ")));
        }
        if !kit.join("_kit").join("ReinstallKit.exe").exists() {
            bad!(tr!("chk.dest.noapp", d));
        }
        if !secrets.is_empty() {
            if enc {
                good(tr!("chk.enc.yes", d));
                secrets_saved = true;
            } else if only_enc {
                let leak: Vec<String> = secrets.iter().filter(|m| kit.join(&m.dest).exists()).map(|m| item_title(m)).collect();
                if leak.is_empty() {
                    good(tr!("chk.enc.clean", d));
                } else {
                    bad!(tr!("chk.enc.leak", d, leak.join(", ")));
                }
            } else {
                note(tr!("chk.enc.plain", d));
                secrets_saved = true;
            }
        }
        if let Some(root) = sys::root_of(d) {
            if let Some((free, total)) = sys::free_space(&root) {
                let line = tr!("chk.free", root, fmt_size(free), fmt_size(total));
                if free < 5 * 1024 * 1024 * 1024 {
                    bad!(format!("{} {}", line, tr!("chk.free.low")));
                } else {
                    good(line);
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(kit.join("_versions")) {
            note(tr!("chk.versions", rd.flatten().count()));
        }
    }
    if !secrets.is_empty() && !secrets_saved {
        bad!(tr!("chk.enc.nowhere"));
    }

    out.line("");
    out.line(tr!("chk.h.auto"));
    step += 1;
    out.progress(step, steps, &tr!("chk.h.auto"));
    if !s.schedule.enabled {
        note(tr!("chk.task.off"));
    } else if sys::task_state(sys::TASK_NAME) == Some(true) {
        good(tr!("chk.task.on", s.schedule.time));
    } else {
        bad!(tr!("chk.task.none"));
    }
    if let Some(last) = last_journal_line(" update: ") {
        note(format!("{} {}", tr!("chk.last"), last));
    }

    out.line("");
    out.line(tr!("chk.h.sources"));
    step += 1;
    out.progress(step, steps, &tr!("chk.h.sources"));
    let gone: Vec<String> = items.iter().filter(|m| !Path::new(&sys::item_path(m)).is_dir()).map(|m| item_title(m)).collect();
    if gone.is_empty() {
        good(tr!("chk.src.ok", items.len()));
    } else {
        note(tr!("chk.src.gone", gone.join(", ")));
    }
    if s.inventory.drivers && !sys::is_elevated() {
        note(tr!("upd.drivers.admin"));
    }

    out.progress(steps, steps, &tr!("chk.done"));
    out.line("");
    if bad > 0 {
        out.line(tr!("chk.sum.bad", bad, m_bad));
        1
    } else {
        out.line(tr!("chk.sum.ok"));
        0
    }
}
