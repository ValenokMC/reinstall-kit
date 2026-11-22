//! Which disk is "the" disk for copies.
//!
//! Windows hands out drive letters in the order disks are plugged in, so the letter written in
//! settings.json can end up on somebody else's disk. A place is therefore found by a marker file
//! inside its kit folder (`<place>\ReinstallKit\.reinstallkit-disk`), not by the letter:
//!   * exactly one disk carries the marker -> that disk is used, whatever letter it has now;
//!   * no disk carries it, or several do    -> nothing is copied, and nothing is created anywhere.
//! The marker is written only by an explicit action of the user (the Set up window or `--mark`).
use crate::settings::KIT_FOLDER;
use std::path::Path;

pub const MARKER: &str = ".reinstallkit-disk";

#[derive(Debug, PartialEq)]
pub enum Place {
    /// Exactly one disk carries the marker: the place as it is spelled now (current drive letter).
    Ready(String),
    /// No disk carries the marker, but the drive from settings is connected. Holds the kit folders
    /// found without a marker (they are not the disk for copies, only listed for information).
    Unmarked(Vec<String>),
    /// No disk carries the marker and the drive from settings is not connected.
    Offline,
    /// The marker is on several disks (for example a kit folder was copied over): ambiguous, so nothing is written.
    Many(Vec<String>),
}

/// "G:\Backups" -> ('G', "Backups"); "J:\" -> ('J', ""). None for network paths.
fn split(path: &str) -> Option<(char, String)> {
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        let rest = path[2..].trim_matches(|c| c == '\\' || c == '/').to_string();
        Some(((b[0] as char).to_ascii_uppercase(), rest))
    } else {
        None
    }
}

/// Looks for `<root><rest>\ReinstallKit\MARKER` under every root (each root ends with a backslash).
/// Returns (places with the marker, kit folders without it).
fn find_in(roots: &[String], rest: &str) -> (Vec<String>, Vec<String>) {
    let (mut marked, mut unmarked) = (vec![], vec![]);
    for root in roots {
        let place = format!("{}{}", root, rest);
        let kit = Path::new(&place).join(KIT_FOLDER);
        if kit.join(MARKER).is_file() {
            marked.push(place);
        } else if kit.is_dir() {
            unmarked.push(kit.to_string_lossy().into_owned());
        }
    }
    (marked, unmarked)
}

/// Finds the place that `path` (as written in settings.json) stands for.
pub fn resolve(path: &str) -> Place {
    match split(path) {
        Some((letter, rest)) => {
            let roots: Vec<String> = ('C'..='Z').map(|c| format!("{}:\\", c)).collect();
            decide(find_in(&roots, &rest), Path::new(&format!("{}:\\", letter)).exists())
        }
        None => {
            // A network folder has no letter to move: it is either marked or not.
            let present = crate::sys::root_of(path).map(|r| Path::new(&r).exists()).unwrap_or(false);
            let kit = Path::new(path).join(KIT_FOLDER);
            if kit.join(MARKER).is_file() {
                Place::Ready(path.to_string())
            } else if present {
                Place::Unmarked(if kit.is_dir() { vec![kit.to_string_lossy().into_owned()] } else { vec![] })
            } else {
                Place::Offline
            }
        }
    }
}

fn decide((marked, unmarked): (Vec<String>, Vec<String>), configured_drive_there: bool) -> Place {
    match marked.len() {
        1 => Place::Ready(marked.into_iter().next().unwrap()),
        0 if configured_drive_there => Place::Unmarked(unmarked),
        0 => Place::Offline,
        _ => Place::Many(marked),
    }
}

/// The place as it is now when it can be found by its marker, otherwise as written in settings.
/// For showing and for reading only: never use it to decide where to write.
pub fn current(path: &str) -> String {
    match resolve(path) {
        Place::Ready(p) => p,
        _ => path.to_string(),
    }
}

/// Puts the marker into `<place>\ReinstallKit`, creating that folder. Call it only when the user has
/// just confirmed that this very disk is the one for copies.
pub fn mark(place: &str) -> std::io::Result<()> {
    let kit = Path::new(place).join(KIT_FOLDER);
    std::fs::create_dir_all(&kit)?;
    let file = kit.join(MARKER);
    if file.is_file() {
        return Ok(());
    }
    let text = format!(
        "Reinstall Kit: this is the disk for copies. Do not delete this file.\r\n\
         Reinstall Kit writes copies only to a disk that has this file in its ReinstallKit folder,\r\n\
         wherever Windows puts the drive letter. Without the file it copies nothing.\r\n\
         Marked on {} as {}\r\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        place
    );
    std::fs::write(&file, text)?;
    hide(&file);
    Ok(())
}

fn hide(file: &Path) {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
    unsafe {
        let _ = SetFileAttributesW(&HSTRING::from(file.as_os_str()), FILE_ATTRIBUTE_HIDDEN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn root(base: &Path, name: &str) -> String {
        let p = base.join(name);
        fs::create_dir_all(&p).unwrap();
        format!("{}\\", p.display())
    }

    #[test]
    fn splits_paths() {
        assert_eq!(split(r"G:\Бэкапы"), Some(('G', "Бэкапы".into())));
        assert_eq!(split(r"j:\"), Some(('J', String::new())));
        assert_eq!(split(r"J:"), Some(('J', String::new())));
        assert_eq!(split(r"\\nas\share\kit"), None);
    }

    #[test]
    fn marker_decides_not_the_letter() {
        let base = std::env::temp_dir().join(format!("rk-disk-test-{}", std::process::id()));
        let (real, stranger, empty) = (root(&base, "real"), root(&base, "stranger"), root(&base, "empty"));
        let roots = vec![stranger.clone(), real.clone(), empty.clone()];

        // The stranger has a kit folder without a marker (what the old program left behind): never chosen.
        fs::create_dir_all(Path::new(&stranger).join(KIT_FOLDER)).unwrap();
        let (marked, unmarked) = find_in(&roots, "");
        assert!(marked.is_empty());
        assert_eq!(unmarked.len(), 1);
        assert_eq!(decide((marked, unmarked), true), Place::Unmarked(vec![format!("{}{}", stranger, KIT_FOLDER)]));

        // No marker anywhere and the configured drive is not there at all.
        assert_eq!(decide((vec![], vec![]), false), Place::Offline);

        // The real disk gets the marker: it is found, the stranger is still ignored.
        mark(&real).unwrap();
        assert!(Path::new(&real).join(KIT_FOLDER).join(MARKER).is_file());
        assert_eq!(decide(find_in(&roots, ""), true), Place::Ready(real.clone()));

        // Marking twice changes nothing; the stranger and the empty disk got no marker and no new folders.
        mark(&real).unwrap();
        assert!(!Path::new(&stranger).join(KIT_FOLDER).join(MARKER).exists());
        assert!(!Path::new(&empty).join(KIT_FOLDER).exists());

        // The kit folder is copied over to another disk together with its marker: ambiguous, refuse.
        mark(&empty).unwrap();
        match decide(find_in(&roots, ""), true) {
            Place::Many(v) => assert_eq!(v.len(), 2),
            other => panic!("expected Many, got {:?}", other),
        }

        // A place inside a folder: the part after the letter is kept.
        let sub = root(&base, "real2\\Backups");
        let roots2 = vec![root(&base, "real2")];
        assert_eq!(decide(find_in(&roots2, "Backups"), true), Place::Unmarked(vec![]));
        mark(&sub.trim_end_matches('\\')).unwrap();
        assert_eq!(decide(find_in(&roots2, "Backups"), true), Place::Ready(format!("{}Backups", roots2[0])));

        let _ = fs::remove_dir_all(&base);
    }
}
