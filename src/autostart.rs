//! "Start with Windows": a value under HKCU\Software\Microsoft\Windows\CurrentVersion\Run
//! (no admin rights needed). Off until the user turns it on.

use std::path::Path;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

/// The Run value: the full exe path in quotes.
fn startup_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
/// Name of our value under the Run key.
const RUN_VALUE_NAME: &str = "AudioLite";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The command stored under `name`, if the entry exists.
fn read_run_value(name: &str) -> Option<String> {
    let name = wide(name);
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_QUERY_VALUE, &mut key).is_err() {
            return None;
        }
        let mut bytes = 0u32;
        let probe = RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, None, Some(&mut bytes));
        let mut out = None;
        if probe.is_ok() {
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
            let mut size = (buf.len() * 2) as u32;
            let r = RegQueryValueExW(
                key,
                PCWSTR(name.as_ptr()),
                None,
                None,
                Some(buf.as_mut_ptr() as *mut u8),
                Some(&mut size),
            );
            if r.is_ok() {
                let n = (size as usize / 2).min(buf.len());
                out = Some(String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string());
            }
        }
        let _ = RegCloseKey(key);
        out
    }
}

/// Set (`Some(command)`) or remove (`None`) the entry `name`. Removing a missing entry is fine.
fn write_run_value(name: &str, command: Option<&str>) -> Result<(), String> {
    let name = wide(name);
    unsafe {
        let mut key = HKEY::default();
        if command.is_some() {
            // The Run key may not exist yet (fresh profiles): create it.
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
            .ok()
            .map_err(|e| e.to_string())?;
        } else {
            let r = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut key);
            if r == ERROR_FILE_NOT_FOUND || r == ERROR_PATH_NOT_FOUND {
                return Ok(()); // no key, so nothing to remove
            }
            r.ok().map_err(|e| e.to_string())?;
        }
        let res = match command {
            Some(c) => {
                let cmd = wide(c);
                let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
                RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes)).ok()
            }
            None => match RegDeleteValueW(key, PCWSTR(name.as_ptr())) {
                e if e == ERROR_FILE_NOT_FOUND => Ok(()),
                e => e.ok(),
            },
        };
        let _ = RegCloseKey(key);
        res.map_err(|e| e.to_string())
    }
}

/// Add or remove the entry `name` for `exe`.
fn set_entry(name: &str, on: bool, exe: &Path) -> Result<(), String> {
    if on {
        write_run_value(name, Some(&startup_command(exe)))
    } else {
        write_run_value(name, None)
    }
}

/// Keep the entry pointing at the exe that is running now (it may have been moved or rebuilt).
fn refresh_entry(name: &str, exe: &Path) -> Result<(), String> {
    let want = startup_command(exe);
    if read_run_value(name).as_deref() == Some(want.as_str()) {
        return Ok(());
    }
    write_run_value(name, Some(&want))
}

pub fn enabled() -> bool {
    read_run_value(RUN_VALUE_NAME).is_some()
}

pub fn set(on: bool, exe: &Path) -> Result<(), String> {
    set_entry(RUN_VALUE_NAME, on, exe)
}

pub fn refresh(exe: &Path) -> Result<(), String> {
    refresh_entry(RUN_VALUE_NAME, exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These touch the real HKCU Run key, but only under a throw-away name.
    fn test_name(tag: &str) -> String {
        format!("AudioLiteTest-{}-{}", std::process::id(), tag)
    }

    #[test]
    fn toggle_creates_and_removes_the_run_entry() {
        let name = test_name("toggle");
        let exe = Path::new("C:\\Tools\\audio-lite.exe");
        assert_eq!(read_run_value(&name), None);

        set_entry(&name, true, exe).unwrap();
        assert_eq!(read_run_value(&name).as_deref(), Some("\"C:\\Tools\\audio-lite.exe\""));

        set_entry(&name, false, exe).unwrap();
        assert_eq!(read_run_value(&name), None);
        // Turning it off again is harmless.
        set_entry(&name, false, exe).unwrap();
    }

    #[test]
    fn moved_exe_updates_the_entry() {
        let name = test_name("moved");
        set_entry(&name, true, Path::new("C:\\Old\\audio-lite.exe")).unwrap();
        refresh_entry(&name, Path::new("C:\\New dir\\audio-lite.exe")).unwrap();
        assert_eq!(read_run_value(&name).as_deref(), Some("\"C:\\New dir\\audio-lite.exe\""));
        set_entry(&name, false, Path::new("x")).unwrap();
        assert_eq!(read_run_value(&name), None);
    }
}
