//! Everything that talks to Windows: folders, drives, BitLocker, Task Scheduler, notifications, other programs.
use crate::settings::Item;
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use windows::core::{w, BSTR, HSTRING, PCWSTR, VARIANT};
use windows::Win32::Foundation::*;
use windows::Win32::Globalization::{GetUserDefaultLCID, GetUserDefaultUILanguage};
use windows::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeInformationW};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::TaskScheduler::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Shell::PropertiesSystem::{PSGetPropertyKeyFromName, PROPERTYKEY};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const TASK_NAME: &str = "Reinstall Kit";
const NO_WINDOW: u32 = 0x0800_0000;

// ---- Language ------------------------------------------------------------------
pub fn system_is_russian() -> bool {
    unsafe { (GetUserDefaultUILanguage() & 0x3ff) == 0x19 || (GetUserDefaultLCID() & 0x3ff) == 0x19 }
}

// ---- Paths ---------------------------------------------------------------------
fn shell_folder(value: &str) -> Option<String> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders")
        .ok()?;
    let v: String = key.get_value(value).ok()?;
    if v.is_empty() {
        None
    } else {
        Some(expand_env(&v))
    }
}

/// Real current location of a personal folder ("Desktop", "MyDocuments", "MyPictures", "MyVideos", "MyMusic", "Downloads").
pub fn known_folder(name: &str) -> Option<String> {
    let (value, fallback) = match name {
        "Desktop" => ("Desktop", "Desktop"),
        "MyDocuments" => ("Personal", "Documents"),
        "MyPictures" => ("My Pictures", "Pictures"),
        "MyVideos" => ("My Video", "Videos"),
        "MyMusic" => ("My Music", "Music"),
        "Downloads" => ("{374DE290-123F-4565-9164-39C4925E467B}", "Downloads"),
        "Startup" => ("Startup", r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"),
        _ => return None,
    };
    shell_folder(value).or_else(|| std::env::var("USERPROFILE").ok().map(|u| format!(r"{}\{}", u, fallback)))
}

fn expand_env(p: &str) -> String {
    let mut out = String::new();
    let mut rest = p;
    while let Some(a) = rest.find('%') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        match after.find('%') {
            Some(b) => {
                let name = &after[..b];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[b + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Expands %NAME% variables and the special %DOCUMENTS%.
pub fn expand(p: &str) -> String {
    let docs = known_folder("MyDocuments").unwrap_or_default();
    expand_env(&p.replace("%DOCUMENTS%", &docs))
}

/// Replaces the beginning of a path with %LOCALAPPDATA%, %APPDATA% or %USERPROFILE%, so the
/// settings still make sense on a new Windows with another user name.
pub fn tokenize(p: &str) -> String {
    for v in ["LOCALAPPDATA", "APPDATA", "USERPROFILE"] {
        if let Ok(base) = std::env::var(v) {
            if base.is_empty() {
                continue;
            }
            let (pl, bl) = (p.to_lowercase(), base.to_lowercase());
            if pl == bl {
                return format!("%{}%", v);
            }
            if pl.starts_with(&(bl + "\\")) {
                return format!("%{}%{}", v, &p[base.len()..]);
            }
        }
    }
    p.to_string()
}

/// Where an item lives on THIS computer.
pub fn item_path(m: &Item) -> String {
    if !m.known.is_empty() {
        if let Some(k) = known_folder(&m.known) {
            return k;
        }
    }
    expand(&m.path)
}

pub fn root_of(path: &str) -> Option<String> {
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        Some(format!("{}:\\", (b[0] as char).to_ascii_uppercase()))
    } else if path.starts_with(r"\\") {
        let parts: Vec<&str> = path[2..].split('\\').collect();
        if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            Some(format!(r"\\{}\{}\", parts[0], parts[1]))
        } else {
            None
        }
    } else {
        None
    }
}
pub fn drive_there(path: &str) -> bool {
    root_of(path).map(|r| Path::new(&r).exists()).unwrap_or(false)
}

// ---- Drives --------------------------------------------------------------------
#[derive(Clone, Debug, serde::Serialize)]
pub struct Drive {
    pub root: String,
    pub label: String,
    pub free: u64,
    pub total: u64,
    pub removable: bool,
    pub enc: bool,
    pub system: bool,
}

pub fn free_space(root: &str) -> Option<(u64, u64)> {
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(root), Some(&mut free), Some(&mut total), None).ok()? };
    Some((free, total))
}

pub fn drives() -> Vec<Drive> {
    let sys = std::env::var("SystemRoot").ok().and_then(|s| root_of(&s)).unwrap_or_else(|| "C:\\".into());
    let mut v = vec![];
    for c in b'A'..=b'Z' {
        let root = format!("{}:\\", c as char);
        let kind = unsafe { GetDriveTypeW(&HSTRING::from(root.as_str())) };
        if kind != 2 && kind != 3 {
            continue; // only removable and fixed drives
        }
        let Some((free, total)) = free_space(&root) else { continue };
        let mut name = [0u16; 261];
        let label = unsafe {
            if GetVolumeInformationW(&HSTRING::from(root.as_str()), Some(&mut name), None, None, None, None).is_ok() {
                String::from_utf16_lossy(&name[..name.iter().position(|&x| x == 0).unwrap_or(0)])
            } else {
                String::new()
            }
        };
        v.push(Drive { enc: bitlocker_on(&root), system: root.eq_ignore_ascii_case(&sys), root, label, free, total, removable: kind == 2 });
    }
    v
}

pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// BitLocker state of a volume without administrator rights - the same value Explorer shows.
/// Only "protection on" (1) counts; off, suspended, still encrypting or unknown are NOT encrypted.
pub fn bitlocker_on(root: &str) -> bool {
    com_init();
    unsafe {
        let mut key = PROPERTYKEY::default();
        if PSGetPropertyKeyFromName(w!("System.Volume.BitLockerProtection"), &mut key).is_err() {
            return false;
        }
        let item: windows::core::Result<IShellItem2> = SHCreateItemFromParsingName(&HSTRING::from(root), None);
        match item {
            Ok(it) => it.GetInt32(&key).map(|v| v == 1).unwrap_or(false),
            Err(_) => false,
        }
    }
}
pub fn is_encrypted(path: &str) -> bool {
    match root_of(path) {
        Some(r) if !r.starts_with(r"\\") => bitlocker_on(&r),
        _ => false,
    }
}

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

// ---- Being gentle with the computer ---------------------------------------------
pub fn low_priority_process() {
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
}
/// Lowers CPU, disk and memory priority of the calling (working) thread.
pub fn background_thread() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// Only one update at a time (the window and the scheduled run could collide). Keep the returned value alive.
pub struct Single(HANDLE);
impl Drop for Single {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub fn single_instance() -> Option<Single> {
    unsafe {
        let h = CreateMutexW(None, true, w!("ReinstallKitUpdate")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return None;
        }
        Some(Single(h))
    }
}

// ---- Other programs (winget, code, pip, npm, git, wsl, netsh, ...) ---------------
pub fn find_in_path(prog: &str) -> Option<PathBuf> {
    if prog.contains('\\') {
        return Some(PathBuf::from(prog));
    }
    let has_ext = Path::new(prog).extension().is_some();
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if has_ext {
            let c = dir.join(prog);
            if std::fs::symlink_metadata(&c).is_ok() {
                return Some(c);
            }
        } else {
            for ext in ["exe", "cmd", "bat", "com"] {
                let c = dir.join(format!("{}.{}", prog, ext));
                if std::fs::symlink_metadata(&c).is_ok() {
                    return Some(c);
                }
            }
        }
    }
    None
}
pub fn has_tool(prog: &str) -> bool {
    find_in_path(prog).is_some()
}

/// Runs a program with no visible window; returns its exit code and what it printed.
pub fn run(prog: &str, args: &[&str]) -> Option<(i32, String)> {
    let path = find_in_path(prog)?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut cmd = if ext == "cmd" || ext == "bat" {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(&path);
        c
    } else {
        Command::new(&path)
    };
    cmd.args(args).creation_flags(NO_WINDOW).stdin(Stdio::null()).stderr(Stdio::null());
    let out = cmd.output().ok()?;
    Some((out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned()))
}
pub fn run_ok(prog: &str, args: &[&str]) -> bool {
    matches!(run(prog, args), Some((0, _)))
}

pub fn wsl_distros() -> Vec<String> {
    let mut v = vec![];
    if let Ok(k) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Lxss") {
        for name in k.enum_keys().flatten() {
            if let Ok(sub) = k.open_subkey(&name) {
                if let Ok(d) = sub.get_value::<String, _>("DistributionName") {
                    if !d.is_empty() && !d.starts_with("docker-desktop") {
                        v.push(d);
                    }
                }
            }
        }
    }
    v
}

// ---- Task Scheduler -------------------------------------------------------------
unsafe fn task_root() -> windows::core::Result<ITaskFolder> {
    com_init();
    let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
    let e = VARIANT::default();
    svc.Connect(&e, &e, &e, &e)?;
    svc.GetFolder(&BSTR::from("\\"))
}

fn tag<'a>(xml: &'a str, name: &str) -> &'a str {
    let (open, close) = (format!("<{}>", name), format!("</{}>", name));
    match (xml.find(&open), xml.find(&close)) {
        (Some(a), Some(b)) if b > a => &xml[a + open.len()..b],
        _ => "",
    }
}

fn my_sid() -> String {
    run("whoami", &["/user", "/fo", "csv", "/nh"])
        .map(|(_, o)| o.trim().rsplit(',').next().unwrap_or("").trim_matches('"').to_string())
        .unwrap_or_default()
}

/// Tasks in the root folder that the user made (run as this user), without well-known updaters. Returns (name, xml).
pub fn user_tasks() -> Vec<(String, String)> {
    let user = std::env::var("USERNAME").unwrap_or_default().to_lowercase();
    let sid = my_sid().to_lowercase();
    let skip = ["onedrive", "microsoftedgeupdate", "googleupdate", "googleupdater", "user_feed_synchronization", "opera ", "brave", "adobe",
                "dropbox", "zoom", "ccleaner", "npcap", "createexplorershellunelevatedtask", "optimize start menu"];
    let mut v = vec![];
    unsafe {
        let Ok(folder) = task_root() else { return v };
        let Ok(tasks) = folder.GetTasks(0) else { return v };
        let n = tasks.Count().unwrap_or(0);
        for i in 1..=n {
            let Ok(t) = tasks.get_Item(&VARIANT::from(i)) else { continue };
            let name = t.Name().map(|b| b.to_string()).unwrap_or_default();
            let lname = name.to_lowercase();
            if name.is_empty() || name == TASK_NAME || skip.iter().any(|s| lname.starts_with(s)) {
                continue;
            }
            let xml = t.Xml().map(|b| b.to_string()).unwrap_or_default();
            let uid = tag(&xml, "UserId").to_lowercase();
            let author = tag(&xml, "Author").to_lowercase();
            let mine = !uid.is_empty() && (uid == sid || uid == user || uid.ends_with(&format!("\\{}", user)));
            let by_me = author.is_empty() || author == user || author.ends_with(&format!("\\{}", user));
            if mine && by_me {
                v.push((name, xml));
            }
        }
    }
    v
}

/// Some(enabled) when the task exists.
pub fn task_state(name: &str) -> Option<bool> {
    unsafe {
        let folder = task_root().ok()?;
        let t = folder.GetTask(&BSTR::from(name)).ok()?;
        Some(t.Enabled().map(|b| b.as_bool()).unwrap_or(false))
    }
}

/// Creates or replaces a task for the current user from its XML definition.
pub fn task_register(name: &str, xml: &str) -> Result<(), String> {
    unsafe {
        let folder = task_root().map_err(|e| e.message().to_string())?;
        let e = VARIANT::default();
        folder
            .RegisterTask(&BSTR::from(name), &BSTR::from(xml), TASK_CREATE_OR_UPDATE.0, &e, &e, TASK_LOGON_INTERACTIVE_TOKEN, &e)
            .map(|_| ())
            .map_err(|e| e.message().to_string())
    }
}
pub fn task_delete(name: &str) {
    unsafe {
        if let Ok(folder) = task_root() {
            let _ = folder.DeleteTask(&BSTR::from(name), 0);
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The daily task: runs "ReinstallKit.exe --auto" as the current user, no administrator rights needed.
/// StartWhenAvailable: if the computer was off at that time, the copy is made after it is switched on.
pub fn schedule_daily(time: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let t = if time.len() == 4 { format!("0{}", time) } else { time.to_string() };
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Reinstall Kit: daily update of the backup kit</Description></RegistrationInfo>
  <Triggers><CalendarTrigger><StartBoundary>2024-01-01T{t}:00</StartBoundary><Enabled>true</Enabled><ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay></CalendarTrigger></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT8H</ExecutionTimeLimit>
    <Priority>7</Priority>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>--auto</Arguments></Exec></Actions>
</Task>"#,
        t = t,
        exe = xml_escape(&exe.to_string_lossy())
    );
    task_register(TASK_NAME, &xml)
}

// ---- Notification ---------------------------------------------------------------
fn put(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().collect();
    let n = w.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&w[..n]);
    dst[n] = 0;
}

unsafe extern "system" fn notify_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

/// A plain Windows notification ("copy is made") from the notification area; stays for a few seconds.
pub fn balloon(title: &str, text: &str) {
    unsafe {
        let Ok(hmod) = GetModuleHandleW(None) else { return };
        let hinst: HINSTANCE = hmod.into();
        let cls = w!("ReinstallKitNotify");
        let wc = WNDCLASSW { lpfnWndProc: Some(notify_proc), hInstance: hinst, lpszClassName: cls, ..Default::default() };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), cls, w!("Reinstall Kit"), WINDOW_STYLE(0), 0, 0, 0, 0, None, None, hinst, None) else { return };
        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_TIP | NIF_INFO;
        // icon number 1 is written into the exe by the build (src/bin/stamp.rs)
        nid.hIcon = LoadIconW(hinst, PCWSTR(1 as *const u16)).or_else(|_| LoadIconW(None, IDI_INFORMATION)).unwrap_or_default();
        put(&mut nid.szTip, "Reinstall Kit");
        put(&mut nid.szInfoTitle, title);
        put(&mut nid.szInfo, text);
        nid.dwInfoFlags = NIIF_INFO;
        if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(9);
            let mut msg = MSG::default();
            while std::time::Instant::now() < end {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
        let _ = DestroyWindow(hwnd);
    }
}

// ---- Window look ----------------------------------------------------------------
/// Dark title bar and the Windows 11 frosted-glass backdrop. Returns false where glass is unavailable (Windows 10).
pub fn apply_glass(hwnd: isize) -> bool {
    unsafe {
        let h = HWND(hwnd as *mut c_void);
        let on: i32 = 1;
        let _ = DwmSetWindowAttribute(h, DWMWINDOWATTRIBUTE(20), &on as *const i32 as *const c_void, 4);
        let m = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let _ = DwmExtendFrameIntoClientArea(h, &m);
        let acrylic: i32 = 3;
        DwmSetWindowAttribute(h, DWMWINDOWATTRIBUTE(38), &acrylic as *const i32 as *const c_void, 4).is_ok()
    }
}
