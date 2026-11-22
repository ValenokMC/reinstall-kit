//! Reinstall Kit - a backup kit for reinstalling Windows.
//! One program, no console window: the window (ui.rs) plus a few headless modes used by
//! Task Scheduler, the installer and tests.
#![windows_subsystem = "windows"]

mod check;
mod disk;
mod lang;
mod out;
mod restore;
mod scan;
mod settings;
mod sync;
mod sys;
mod ui;
mod update;

use out::{Msg, Out};
use settings::Settings;
use std::io::Write;
use std::sync::mpsc::channel;

/// Runs a job without a window and returns its exit code and everything it reported.
fn headless(job: impl FnOnce(Out) -> i32) -> (i32, Vec<Msg>) {
    let (tx, rx) = channel();
    let code = job(Out::new(tx, None));
    (code, rx.try_iter().collect())
}

/// Prints the report to standard output (visible when the output is redirected to a file or a pipe).
fn print(msgs: &[Msg]) {
    let mut o = std::io::stdout();
    for m in msgs {
        if let Msg::Line(s) = m {
            let _ = writeln!(o, "{}", s);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let s = Settings::load(&settings::settings_path());
    lang::init(s.as_ref().map(|s| s.language.as_str()).unwrap_or("auto"));
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();

    let code = match arg(0).as_str() {
        // From Task Scheduler: no window at all, low priority, a notification when the copy is made.
        "--auto" => {
            sys::low_priority_process();
            let (code, msgs) = headless(|o| update::run(false, true, o));
            for m in msgs {
                if let Msg::Toast(title, text) = m {
                    sys::balloon(&title, &text);
                }
            }
            code
        }
        // Used by the installer and by "Save settings".
        "--schedule" => {
            let sc = s.map(|s| s.schedule).unwrap_or_default();
            if sc.enabled {
                sys::schedule_daily(&sc.time).map(|_| 0).unwrap_or(1)
            } else {
                sys::task_delete(sys::TASK_NAME);
                0
            }
        }
        "--unschedule" => {
            sys::task_delete(sys::TASK_NAME);
            0
        }
        // Marks the disk for copies: `ReinstallKit.exe --mark G:\Backups`. Only on the user's own command.
        "--mark" => {
            let place = arg(1);
            if sys::root_of(&place).is_none() {
                let _ = writeln!(std::io::stdout(), "{}", tr!("wiz.err.badpath", place));
                2
            } else {
                match disk::mark(&place) {
                    Ok(()) => {
                        let _ = writeln!(std::io::stdout(), "{}", tr!("mark.done", place));
                        0
                    }
                    Err(e) => {
                        let _ = writeln!(std::io::stdout(), "{}", tr!("wiz.err.mark", place, e));
                        1
                    }
                }
            }
        }
        "--toast" => {
            let a = arg(1);
            let (t, x) = a.split_once('|').unwrap_or((a.as_str(), ""));
            sys::balloon(t, x);
            0
        }
        // Headless runs for tests and for those who prefer a command line.
        "--update" | "--update-dry" => {
            let (code, msgs) = headless(|o| update::run(arg(0) == "--update-dry", false, o));
            print(&msgs);
            code
        }
        "--check" => {
            let (code, msgs) = headless(check::run);
            print(&msgs);
            code
        }
        "--restore-dry" => {
            let steps = restore::STEPS.iter().take(8).map(|s| s.to_string()).collect();
            let (code, msgs) = headless(|o| restore::run(arg(1), true, steps, o));
            print(&msgs);
            code
        }
        "--scan" => {
            let (tx, _rx) = channel();
            let r = scan::scan(s.as_ref(), &Out::new(tx, None));
            let _ = writeln!(std::io::stdout(), "{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            0
        }
        _ => {
            ui::run(args);
            0
        }
    };
    std::process::exit(code);
}
