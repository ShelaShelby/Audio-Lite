//! Core Audio: list the active playback devices, read the default one and set
//! a new default with the undocumented IPolicyConfig interface.

use crate::cycle::Device;
use std::ffi::c_void;
use windows::core::{IUnknown, Interface, BSTR, GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_default_endpoint_is_vtable_slot_13() {
        let slot = std::mem::offset_of!(policy::IPolicyConfig_Vtbl, SetDefaultEndpoint) / std::mem::size_of::<usize>();
        assert_eq!(slot, 13);
    }
}
