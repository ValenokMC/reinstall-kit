//! Development helper, not shipped: writes an .ico file into a built .exe as its icon.
//!   stamp <program.exe> <icon.ico>
//! Done with Windows' own resource-update functions, so no resource compiler is needed.
use std::ffi::c_void;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::LibraryLoader::{BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

fn id(n: u16) -> PCWSTR {
    PCWSTR(n as usize as *const u16)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: stamp <program.exe> <icon.ico>");
        std::process::exit(2);
    }
    let ico = std::fs::read(&args[2]).expect("cannot read the icon file");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let mut group: Vec<u8> = ico[..6].to_vec();
    unsafe {
        let h = BeginUpdateResourceW(&HSTRING::from(args[1].as_str()), false).expect("cannot open the exe");
        for i in 0..count {
            let e = &ico[6 + i * 16..6 + i * 16 + 16];
            let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
            let offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
            let image = &ico[offset..offset + size];
            UpdateResourceW(h, id(RT_ICON), id(i as u16 + 1), 0, Some(image.as_ptr() as *const c_void), size as u32).expect("cannot write an icon image");
            group.extend_from_slice(&e[..12]);
            group.extend_from_slice(&(i as u16 + 1).to_le_bytes());
        }
        UpdateResourceW(h, id(RT_GROUP_ICON), id(1), 0, Some(group.as_ptr() as *const c_void), group.len() as u32).expect("cannot write the icon group");
        EndUpdateResourceW(h, false).expect("cannot save the exe");
    }
    println!("icon written into {}", args[1]);
}
