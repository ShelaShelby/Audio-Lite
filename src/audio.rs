//! Core Audio: list the active playback devices, read the default one and set
//! a new default with the undocumented IPolicyConfig interface.

use crate::cycle::Device;
use crate::mixer::{Row, Session};
use std::collections::HashMap;
use std::ffi::c_void;
use windows::core::{IUnknown, Interface, BSTR, GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, STILL_ACTIVE, S_OK};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, AudioSessionStateExpired, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[allow(non_snake_case)]
mod policy {
    use std::ffi::c_void;
    use windows::core::{interface, IUnknown, IUnknown_Vtbl, HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::ERole;

    /// IPolicyConfig (Windows 7 and later). Undocumented; layout taken from
    /// EreTIk's PolicyConfig.h and EarTrumpet's IPolicyConfigWin7, which agree:
    /// ten methods we do not use, then SetDefaultEndpoint (vtable slot 13).
    #[interface("F8679F50-850A-41CF-9C72-430F290290C8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
        fn GetDeviceFormat(&self, device: PCWSTR, default: i32, format: *mut *mut c_void) -> HRESULT;
        fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(&self, device: PCWSTR, endpoint: *mut c_void, mix: *mut c_void) -> HRESULT;
        fn GetProcessingPeriod(&self, device: PCWSTR, default: i32, period: *mut i64, min: *mut i64) -> HRESULT;
        fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn GetPropertyValue(&self, device: PCWSTR, key: *const c_void, value: *mut c_void) -> HRESULT;
        fn SetPropertyValue(&self, device: PCWSTR, key: *const c_void, value: *mut c_void) -> HRESULT;
        fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }

    /// The generated methods are private to this module.
    pub fn set_default_endpoint(pc: &IPolicyConfig, device: PCWSTR, role: ERole) -> HRESULT {
        unsafe { pc.SetDefaultEndpoint(device, role) }
    }
}
use policy::IPolicyConfig;

/// CPolicyConfigVistaClient. Tried first, as asked for in the project brief.
const CLSID_POLICY_CONFIG_VISTA: GUID = GUID::from_u128(0x294935ce_f637_4e7c_a41b_ab255460b862);
/// CPolicyConfigClient, the class most tools use with IPolicyConfig. Fallback.
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Passed as the event context of every volume and mute change the app makes,
/// so the change events for them can be told apart from changes made elsewhere.
pub const APP_CONTEXT: GUID = GUID::from_u128(0x5e1a_0a17_3c4b_4d2e_9f61_a0d1_e7c3_b2a5);

/// True if an event comes from one of the app's own changes.
pub fn is_own(context: *const GUID) -> bool {
    !context.is_null() && unsafe { *context } == APP_CONTEXT
}

pub fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn device_id(device: &IMMDevice) -> windows::core::Result<String> {
    unsafe {
        let p: PWSTR = device.GetId()?;
        let id = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const c_void));
        Ok(id)
    }
}

fn device_name(device: &IMMDevice) -> String {
    let name = unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
            .ok()
            .and_then(|v| BSTR::try_from(&v).ok())
            .map(|b| b.to_string())
    };
    name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Unknown device".to_string())
}

/// All active playback devices, in Windows' order.
pub fn devices(e: &IMMDeviceEnumerator) -> windows::core::Result<Vec<Device>> {
    unsafe {
        let list = e.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..list.GetCount()? {
            let Ok(d) = list.Item(i) else { continue };
            let Ok(id) = device_id(&d) else { continue };
            out.push(Device { id, name: device_name(&d) });
        }
        Ok(out)
    }
}

/// The current default playback device (console role), if there is one.
pub fn default_device(e: &IMMDeviceEnumerator) -> Option<Device> {
    unsafe {
        let d = e.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        Some(Device { id: device_id(&d).ok()?, name: device_name(&d) })
    }
}

fn policy_config() -> windows::core::Result<IPolicyConfig> {
    unsafe {
        CoCreateInstance::<_, IUnknown>(&CLSID_POLICY_CONFIG_VISTA, None, CLSCTX_ALL)
            .and_then(|u| u.cast::<IPolicyConfig>())
            .or_else(|_| CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL))
    }
}

/// Make `id` the default for all three roles (console, multimedia, communications).
pub fn set_default(id: &str) -> Result<(), String> {
    let pc = policy_config().map_err(|e| format!("PolicyConfig not available: {}", e.message()))?;
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    for role in [eConsole, eMultimedia, eCommunications] {
        policy::set_default_endpoint(&pc, PCWSTR(wide.as_ptr()), role)
            .ok()
            .map_err(|e| format!("could not set default: {}", e.message()))?;
    }
    Ok(())
}

/// A string Windows allocated for us, copied and freed.
fn take_pwstr(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe {
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

/// The default playback device and its master volume control.
pub fn default_endpoint(e: &IMMDeviceEnumerator) -> windows::core::Result<(IMMDevice, IAudioEndpointVolume)> {
    unsafe {
        let d = e.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let v = d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?;
        Ok((d, v))
    }
}

pub fn device_display_name(d: &IMMDevice) -> String {
    device_name(d)
}

/// Master volume (0.0-1.0) and mute.
pub fn endpoint_state(v: &IAudioEndpointVolume) -> windows::core::Result<(f32, bool)> {
    unsafe { Ok((v.GetMasterVolumeLevelScalar()?, v.GetMute()?.as_bool())) }
}

pub fn set_endpoint_volume(v: &IAudioEndpointVolume, scalar: f32) {
    unsafe {
        let _ = v.SetMasterVolumeLevelScalar(scalar, &APP_CONTEXT);
    }
}

pub fn set_endpoint_mute(v: &IAudioEndpointVolume, mute: bool) {
    unsafe {
        let _ = v.SetMute(mute, &APP_CONTEXT);
    }
}

pub fn set_session_volume(v: &ISimpleAudioVolume, scalar: f32) {
    unsafe {
        let _ = v.SetMasterVolume(scalar, &APP_CONTEXT);
    }
}

pub fn set_session_mute(v: &ISimpleAudioVolume, mute: bool) {
    unsafe {
        let _ = v.SetMute(mute, &APP_CONTEXT);
    }
}

/// Current volume and mute of one session.
pub fn session_state(v: &ISimpleAudioVolume) -> Option<(f32, bool)> {
    unsafe { Some((v.GetMasterVolume().ok()?, v.GetMute().ok()?.as_bool())) }
}

/// Full exe path of a process, if Windows lets us read it.
fn exe_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]))
    }
}

/// True if no process has this id any more (its session is a leftover that
/// Windows has not ended yet). False when it runs but cannot be read.
fn process_gone(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(h) => {
            let mut code = 0u32;
            let ended = unsafe { GetExitCodeProcess(h, &mut code) }.is_ok() && code != STILL_ACTIVE.0 as u32;
            unsafe {
                let _ = CloseHandle(h);
            }
            ended
        }
        Err(e) => e.code() == ERROR_INVALID_PARAMETER.to_hresult(),
    }
}

/// The "File description" from an exe's version info ("Google Chrome").
fn file_description(path: &str) -> Option<String> {
    let wpath: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), 0, size, data.as_mut_ptr() as *mut c_void).ok()?;
        // The first language/code page pair, then the usual US English fallbacks.
        let mut langs = Vec::new();
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let q: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
        if VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(q.as_ptr()), &mut ptr, &mut len).as_bool() && len >= 4
        {
            let pair = std::slice::from_raw_parts(ptr as *const u16, 2);
            langs.push(format!("{:04x}{:04x}", pair[0], pair[1]));
        }
        langs.extend(["040904b0".to_string(), "040904e4".to_string()]);
        for lang in langs {
            let q: Vec<u16> = format!("\\StringFileInfo\\{lang}\\FileDescription\0").encode_utf16().collect();
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            if VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(q.as_ptr()), &mut ptr, &mut len).as_bool()
                && len > 0
            {
                let chars = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let text = String::from_utf16_lossy(chars).trim_end_matches('\0').trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}

/// Exe path and description per process id, so the version info is not read
/// again every refresh.
#[derive(Default)]
pub struct NameCache(HashMap<u32, (Option<String>, Option<String>)>);

impl NameCache {
    /// Keep only the processes that still have a row.
    pub fn retain_rows<T>(&mut self, rows: &[Row<T>]) {
        let keep: Vec<u32> = rows.iter().flat_map(|r| r.pids.iter().copied()).collect();
        self.0.retain(|pid, _| keep.contains(pid));
    }

    fn get(&mut self, pid: u32) -> (Option<String>, Option<String>) {
        self.0
            .entry(pid)
            .or_insert_with(|| {
                let exe = exe_path(pid);
                let description = exe.as_deref().and_then(file_description);
                (exe, description)
            })
            .clone()
    }
}

/// One session's volume control, and the control its events are registered on.
pub struct Handle {
    pub volume: ISimpleAudioVolume,
    pub control: IAudioSessionControl,
}

/// The session manager of `device`. Its session list must be read once
/// (`sessions`) before it reports new sessions.
pub fn session_manager(device: &IMMDevice) -> windows::core::Result<IAudioSessionManager2> {
    unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }
}

/// One session, read on the audio thread. `i` names it if it has no id.
pub fn session(control: &IAudioSessionControl, i: i32, names: &mut NameCache) -> Option<Session<Handle>> {
    unsafe {
        let c2 = control.cast::<IAudioSessionControl2>().ok()?;
        let volume = control.cast::<ISimpleAudioVolume>().ok()?;
        let pid = c2.GetProcessId().unwrap_or(0);
        let system = c2.IsSystemSoundsSession() == S_OK;
        let (exe, description) = if system { (None, None) } else { names.get(pid) };
        let (level, muted) = session_state(&volume).unwrap_or((1.0, false));
        Some(Session {
            id: c2.GetSessionInstanceIdentifier().ok().and_then(take_pwstr).unwrap_or_else(|| format!("#{i}")),
            pid,
            exe,
            description,
            display_name: control.GetDisplayName().ok().and_then(take_pwstr),
            system,
            // A session of a process that has ended is not shown (Windows keeps
            // some of them for a while, they would be "Process 1234" rows).
            expired: control.GetState() == Ok(AudioSessionStateExpired) || (!system && process_gone(pid)),
            volume: level,
            muted,
            handle: Handle { volume, control: control.clone() },
        })
    }
}

/// All audio sessions of `manager`, expired ones included (the merge drops them).
pub fn sessions(manager: &IAudioSessionManager2, names: &mut NameCache) -> windows::core::Result<Vec<Session<Handle>>> {
    unsafe {
        let list = manager.GetSessionEnumerator()?;
        let mut out = Vec::new();
        for i in 0..list.GetCount()? {
            let Ok(control) = list.GetSession(i) else { continue };
            out.extend(session(&control, i, names));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_default_endpoint_is_vtable_slot_13() {
        let slot = std::mem::offset_of!(policy::IPolicyConfig_Vtbl, SetDefaultEndpoint) / std::mem::size_of::<usize>();
        assert_eq!(slot, 13);
    }
}
