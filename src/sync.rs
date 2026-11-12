//! The copying engine: mirrors a folder into the kit, keeping previous versions of changed and
//! deleted files. Nothing is ever deleted outright - whatever leaves the kit goes to "_versions".
use std::collections::HashSet;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const REPARSE: u32 = 0x400;

/// Lets paths be longer than 260 characters.
pub fn long(p: &Path) -> PathBuf {
    let s = p.to_string_lossy().replace('/', "\\");
    if s.starts_with(r"\\?\") {
        PathBuf::from(s)
    } else if let Some(rest) = s.strip_prefix(r"\\") {
        PathBuf::from(format!(r"\\?\UNC\{}", rest))
    } else if s.len() >= 3 && s.as_bytes()[1] == b':' {
        PathBuf::from(format!(r"\\?\{}", s.trim_end_matches('\\')))
    } else {
        PathBuf::from(s)
    }
}

// ---- Skip patterns ---------------------------------------------------------------
// "/logs/**"  - from the top of the folder;   "node_modules/**", "*.pyc" - at any depth;
// "*" does not cross a folder boundary, "**" does, "?" is one character. Case does not matter.
pub struct Filter {
    pats: Vec<(bool, Vec<char>)>,
}

fn gm(p: &[char], s: &[char]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    if p[0] == '*' {
        if p.len() > 1 && p[1] == '*' {
            let rest = &p[2..];
            let rest2 = if rest.first() == Some(&'/') { Some(&rest[1..]) } else { None };
            for i in 0..=s.len() {
                if gm(rest, &s[i..]) {
                    return true;
                }
                if let Some(r2) = rest2 {
                    if (i == 0 || s[i - 1] == '/') && gm(r2, &s[i..]) {
                        return true;
                    }
                }
            }
            false
        } else {
            let rest = &p[1..];
            for i in 0..=s.len() {
                if gm(rest, &s[i..]) {
                    return true;
                }
                if i < s.len() && s[i] == '/' {
                    break;
                }
            }
            false
        }
    } else if !s.is_empty() && ((p[0] == '?' && s[0] != '/') || p[0] == s[0]) {
        gm(&p[1..], &s[1..])
    } else {
        false
    }
}

impl Filter {
    pub fn new(patterns: &[String]) -> Filter {
        let pats = patterns
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| {
                let p = p.trim().replace('\\', "/").to_lowercase();
                match p.strip_prefix('/') {
                    Some(rest) => (true, rest.chars().collect()),
                    None => (false, p.chars().collect()),
                }
            })
            .collect();
        Filter { pats }
    }
    /// `rel` is the path inside the folder, with forward slashes.
    pub fn excluded(&self, rel: &str) -> bool {
        let s: Vec<char> = rel.to_lowercase().chars().collect();
        for (anchored, p) in &self.pats {
            if *anchored {
                if gm(p, &s) {
                    return true;
                }
            } else {
                let mut start = 0;
                loop {
                    if gm(p, &s[start..]) {
                        return true;
                    }
                    match s[start..].iter().position(|&c| c == '/') {
                        Some(i) => start += i + 1,
                        None => break,
                    }
                }
            }
        }
        false
    }
    /// True when everything inside the folder is skipped, so there is no need to look into it.
    pub fn dir_excluded(&self, rel: &str) -> bool {
        self.excluded(&format!("{}/\u{1}", rel))
    }
}

#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub copied: u64,
    pub bytes: u64,
    pub versioned: u64,
    pub errors: u64,
}

fn is_reparse(md: &fs::Metadata) -> bool {
    md.file_attributes() & REPARSE != 0
}

/// Lists files under `root` as (relative path with '/', full path, size, modified). Links and junctions are not followed.
fn list(root: &Path, filter: &Filter, errors: &mut Vec<String>) -> Vec<(String, PathBuf, u64, SystemTime)> {
    let mut out = vec![];
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                errors.push(format!("{}: {}", dir.display(), e));
                continue;
            }
        };
        for ent in rd.flatten() {
            let Ok(md) = ent.metadata() else { continue };
            if is_reparse(&md) {
                continue;
            }
            let name = ent.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name } else { format!("{}/{}", rel, name) };
            if md.is_dir() {
                if !filter.dir_excluded(&r) {
                    stack.push((ent.path(), r));
                }
            } else if !filter.excluded(&r) {
                out.push((r, ent.path(), md.len(), md.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
            }
        }
    }
    out
}

fn same(len: u64, time: SystemTime, md: &fs::Metadata) -> bool {
    if md.len() != len {
        return false;
    }
    let other = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let diff = match time.duration_since(other) {
        Ok(d) => d,
        Err(e) => e.duration(),
    };
    diff.as_secs() < 2 // FAT and exFAT keep time in 2-second steps
}

fn rel_path(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

/// Copies through a temporary name, so an interrupted copy never leaves a half-written file under the real name.
fn copy_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = dst.as_os_str().to_os_string();
    tmp.push(".rk-part");
    let tmp = PathBuf::from(tmp);
    let _ = fs::remove_file(&tmp);
    if let Err(e) = fs::copy(src, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    clear_readonly(&tmp);
    if dst.exists() {
        clear_readonly(dst);
        fs::remove_file(dst)?;
    }
    fs::rename(&tmp, dst)
}

fn clear_readonly(p: &Path) {
    if let Ok(md) = fs::metadata(p) {
        let mut perm = md.permissions();
        if perm.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = fs::set_permissions(p, perm);
        }
    }
}

fn move_to_versions(file: &Path, versions: &Path, rel: &str) -> std::io::Result<()> {
    let target = rel_path(versions, rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if target.exists() {
        clear_readonly(&target);
        fs::remove_file(&target)?; // several runs in one day: the day keeps the latest previous version
    }
    clear_readonly(file);
    fs::rename(file, &target)
}

fn remove_empty_dirs(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            if let Ok(md) = ent.metadata() {
                if md.is_dir() && !is_reparse(&md) {
                    remove_empty_dirs(&ent.path());
                    let _ = fs::remove_dir(ent.path()); // fails (and that is fine) when not empty
                }
            }
        }
    }
}

/// Makes `dst` a mirror of `src`. `only` (if not empty) limits it to these files from the top of `src`.
pub fn sync_dir(src: &Path, dst: &Path, versions: &Path, exclude: &[String], only: &[String], dry: bool, errors: &mut Vec<String>) -> Stats {
    let (src, dst, versions) = (long(src), long(dst), long(versions));
    let filter = Filter::new(exclude);
    let mut st = Stats::default();
    let before = errors.len();

    let files: Vec<(String, PathBuf, u64, SystemTime)> = if only.is_empty() {
        list(&src, &filter, errors)
    } else {
        only.iter()
            .filter_map(|f| {
                let p = src.join(f);
                let md = fs::metadata(&p).ok()?;
                if md.is_file() {
                    Some((f.clone(), p, md.len(), md.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
                } else {
                    None
                }
            })
            .collect()
    };
    // If the source could not be read completely, do not treat unseen files as deleted.
    let incomplete = errors.len() > before;

    let mut keep: HashSet<String> = HashSet::with_capacity(files.len());
    for (rel, path, len, time) in &files {
        keep.insert(rel.to_lowercase());
        let target = rel_path(&dst, rel);
        let existing = fs::metadata(&target).ok();
        if let Some(md) = &existing {
            if md.is_file() && same(*len, *time, md) {
                continue;
            }
        }
        if dry {
            st.copied += 1;
            st.bytes += len;
            if existing.is_some() {
                st.versioned += 1;
            }
            continue;
        }
        if existing.is_some() {
            match move_to_versions(&target, &versions, rel) {
                Ok(()) => st.versioned += 1,
                Err(e) => {
                    errors.push(format!("{}: {}", target.display(), e));
                    continue;
                }
            }
        }
        match copy_file(path, &target) {
            Ok(()) => {
                st.copied += 1;
                st.bytes += len;
            }
            Err(e) => errors.push(format!("{}: {}", path.display(), e)),
        }
    }

    // What is in the kit but no longer on the computer goes to versions.
    if dst.is_dir() && !incomplete {
        let none = Filter::new(&[]);
        let mut ignore = vec![];
        for (rel, path, _, _) in list(&dst, &none, &mut ignore) {
            if keep.contains(&rel.to_lowercase()) || rel.ends_with(".rk-part") {
                continue;
            }
            if dry {
                st.versioned += 1;
                continue;
            }
            match move_to_versions(&path, &versions, &rel) {
                Ok(()) => st.versioned += 1,
                Err(e) => errors.push(format!("{}: {}", path.display(), e)),
            }
        }
        if !dry {
            remove_empty_dirs(&dst);
        }
    }
    st.errors = (errors.len() - before) as u64;
    st
}

/// Restore: copies `src` into `dst`. Never deletes. With `keep_newer`, a file that is newer on the computer is left alone.
pub fn copy_dir(src: &Path, dst: &Path, keep_newer: bool, dry: bool, errors: &mut Vec<String>) -> Stats {
    let (src, dst) = (long(src), long(dst));
    let mut st = Stats::default();
    let before = errors.len();
    for (rel, path, len, time) in list(&src, &Filter::new(&[]), errors) {
        let target = rel_path(&dst, &rel);
        if let Ok(md) = fs::metadata(&target) {
            if md.is_file() && same(len, time, &md) {
                continue;
            }
            if keep_newer && md.modified().map(|m| m > time).unwrap_or(false) {
                continue;
            }
        }
        if dry {
            st.copied += 1;
            st.bytes += len;
            continue;
        }
        match copy_file(&path, &target) {
            Ok(()) => {
                st.copied += 1;
                st.bytes += len;
            }
            Err(e) => errors.push(format!("{}: {}", target.display(), e)),
        }
    }
    st.errors = (errors.len() - before) as u64;
    st
}

/// Folder size with a time limit, so a huge folder never stalls the scan. Returns (bytes, cut short).
pub fn measure(path: &Path, limit_ms: u128) -> (u64, bool) {
    let start = std::time::Instant::now();
    let mut bytes = 0u64;
    let mut stack = vec![long(path)];
    while let Some(dir) = stack.pop() {
        if start.elapsed().as_millis() > limit_ms {
            return (bytes, true);
        }
        if let Ok(rd) = fs::read_dir(&dir) {
            for ent in rd.flatten() {
                if let Ok(md) = ent.metadata() {
                    if is_reparse(&md) {
                        continue;
                    }
                    if md.is_dir() {
                        stack.push(ent.path());
                    } else {
                        bytes += md.len();
                    }
                }
            }
        }
    }
    (bytes, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(p: &[&str]) -> Filter {
        Filter::new(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn patterns() {
        let x = f(&["node_modules/**", "*.pyc", "/logs/**", "**/Cache/**", "lockfile", "/tdata/user_data*/**", "/globalStorage/**/cache/**"]);
        assert!(x.excluded("node_modules/a/b.js"));
        assert!(x.excluded("app/node_modules/a.js"));
        assert!(x.dir_excluded("app/node_modules"));
        assert!(!x.excluded("app/my_node_modules/a.js"));
        assert!(x.excluded("a/b/c.PYC"));
        assert!(x.excluded("logs/x.txt"));
        assert!(!x.excluded("sub/logs/x.txt"));
        assert!(x.excluded("Default/Cache/data_0"));
        assert!(x.excluded("Cache/data_0"));
        assert!(x.excluded("sub/lockfile"));
        assert!(!x.excluded("sub/lockfile.txt"));
        assert!(x.excluded("tdata/user_data#2/x"));
        assert!(x.excluded("globalStorage/a/b/cache/z"));
        assert!(!x.dir_excluded("src"));
    }
    #[test]
    fn mirror_and_versions() {
        let base = std::env::temp_dir().join(format!("rk-test-{}", std::process::id()));
        let (src, dst, ver) = (base.join("src"), base.join("dst"), base.join("ver"));
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::create_dir_all(src.join("node_modules")).unwrap();
        fs::write(src.join("a.txt"), "one").unwrap();
        fs::write(src.join("sub/b.txt"), "b").unwrap();
        fs::write(src.join("gone.txt"), "x").unwrap();
        fs::write(src.join("node_modules/junk.js"), "x").unwrap();
        let ex = vec!["node_modules/**".to_string()];
        let mut errs = vec![];
        let s = sync_dir(&src, &dst, &ver, &ex, &[], false, &mut errs);
        assert_eq!((s.copied, s.versioned, s.errors), (3, 0, 0));
        assert!(!dst.join("node_modules").exists());
        // nothing changed -> nothing copied
        let s = sync_dir(&src, &dst, &ver, &ex, &[], false, &mut errs);
        assert_eq!((s.copied, s.versioned), (0, 0));
        // change one, delete one
        fs::write(src.join("a.txt"), "two!").unwrap();
        fs::remove_file(src.join("gone.txt")).unwrap();
        let s = sync_dir(&src, &dst, &ver, &ex, &[], true, &mut errs);
        assert_eq!((s.copied, s.versioned), (1, 2)); // dry run only counts
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "one");
        let s = sync_dir(&src, &dst, &ver, &ex, &[], false, &mut errs);
        assert_eq!((s.copied, s.versioned), (1, 2));
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "two!");
        assert_eq!(fs::read_to_string(ver.join("a.txt")).unwrap(), "one");
        assert!(ver.join("gone.txt").exists() && !dst.join("gone.txt").exists());
        // restore never overwrites a newer file when asked not to
        fs::remove_file(src.join("sub/b.txt")).unwrap();
        let s = copy_dir(&dst, &src, true, false, &mut errs);
        assert_eq!(s.copied, 1);
        assert!(src.join("sub/b.txt").exists());
        assert!(errs.is_empty());
        let _ = fs::remove_dir_all(&base);
    }
}
