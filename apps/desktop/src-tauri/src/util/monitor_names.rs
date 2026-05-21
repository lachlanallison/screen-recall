//! Best-effort friendly monitor names (e.g. "AW3225QF") on Windows.
//! Falls back to the raw GDI device name on other platforms or when the
//! display API doesn't return a model string.

use std::collections::HashMap;

/// Try to map a GDI monitor device name (e.g. `\\.\DISPLAY1`) to a
/// human-friendly model name like "AW3225QF".
pub fn friendly_monitor_name(gdi_name: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        // Build the DisplayConfig map once per call. In practice this is called
        // once per monitor per capture tick (2–3 times), which is cheap enough.
        let map = build_display_config_map();
        if let Some(name) = map.get(gdi_name) {
            return Some(name.clone());
        }
        // Fallback to the older EnumDisplayDevices API.
        friendly_monitor_name_enum(gdi_name)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = gdi_name;
        None
    }
}

/* ------------------------------------------------------------------ */
/* Windows: DisplayConfig (modern, reliable)                          */
/* ------------------------------------------------------------------ */

#[cfg(target_os = "windows")]
fn build_display_config_map() -> HashMap<String, String> {
    use windows::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
    };

    let mut path_count = 0u32;
    let mut mode_count = 0u32;

    let ok = unsafe {
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
    };
    if ok.is_err() {
        return HashMap::new();
    }

    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];

    let ok = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        )
    };
    if ok.is_err() {
        return HashMap::new();
    }

    let mut map = HashMap::new();
    for path in &paths[..path_count as usize] {
        // 1) Get the GDI source device name.
        let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };

        let source_ok = unsafe { DisplayConfigGetDeviceInfo(&mut source_name.header) >= 0 };
        if !source_ok {
            continue;
        }

        let gdi = utf16_to_string(&source_name.viewGdiDeviceName);
        let gdi_trimmed = gdi.trim().to_string();
        if gdi_trimmed.is_empty() {
            continue;
        }

        // 2) Get the friendly monitor target name.
        let mut target_name = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };

        let target_ok = unsafe { DisplayConfigGetDeviceInfo(&mut target_name.header) >= 0 };
        if !target_ok {
            continue;
        }

        let friendly = utf16_to_string(&target_name.monitorFriendlyDeviceName);
        let friendly_trimmed = friendly.trim();
        if !friendly_trimmed.is_empty()
            && !friendly_trimmed.eq_ignore_ascii_case("generic pnp monitor")
        {
            map.insert(gdi_trimmed, friendly_trimmed.to_string());
        }
    }

    map
}

#[cfg(target_os = "windows")]
fn utf16_to_string(arr: &[u16]) -> String {
    let len = arr.iter().position(|&c| c == 0).unwrap_or(arr.len());
    String::from_utf16_lossy(&arr[..len])
}

/* ------------------------------------------------------------------ */
/* Windows: EnumDisplayDevicesW fallback                              */
/* ------------------------------------------------------------------ */

#[cfg(target_os = "windows")]
fn friendly_monitor_name_enum(gdi_name: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};

    let wide: Vec<u16> = gdi_name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut device = DISPLAY_DEVICEW {
        cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };

    let ok = unsafe { EnumDisplayDevicesW(PCWSTR(wide.as_ptr()), 0, &mut device, 0).as_bool() };
    if !ok {
        return None;
    }

    let name = utf16_to_string(&device.DeviceString);
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("generic pnp monitor") {
        return None;
    }
    Some(trimmed.to_string())
}
