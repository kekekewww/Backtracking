use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE, PROPERTYKEY, S_FALSE, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, AudioSessionStateInactive, DEVICE_STATE_ACTIVE, EDataFlow,
    IAudioSessionControl, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, eCapture, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CLSCTX_ALL, CoTaskMemFree, STGM_READ};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::core::{GUID, Interface, PWSTR};

use super::{Apartment, audio_error, endpoint_enumerator, endpoint_id};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CaptureSource {
    System,
    DefaultMicrophone,
    Microphone { id: String, name: String },
    Application { path: String, name: String },
}

impl CaptureSource {
    pub(crate) fn label(&self) -> &str {
        match self {
            Self::System => "System audio",
            Self::DefaultMicrophone => "Default microphone",
            Self::Microphone { name, .. } | Self::Application { name, .. } => name,
        }
    }

    pub(crate) fn same_source(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::System, Self::System) | (Self::DefaultMicrophone, Self::DefaultMicrophone) => {
                true
            }
            (Self::Microphone { id, .. }, Self::Microphone { id: other, .. }) => id == other,
            (Self::Application { path, .. }, Self::Application { path: other, .. }) => {
                same_path(path, other)
            }
            _ => false,
        }
    }

    pub(crate) fn supports_app_capture() -> bool {
        windows_build() >= 20_348
    }
}

pub(super) struct OwnedHandle(pub(super) HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub(super) struct RunningProcess {
    pub(super) id: u32,
    handle: OwnedHandle,
}

impl RunningProcess {
    pub(super) fn running(&self) -> bool {
        // Holding the process handle makes this check specific to the activated process instance.
        unsafe { WaitForSingleObject(self.handle.0, 0) == WAIT_TIMEOUT }
    }
}

pub(super) fn target_process(executable: &str) -> Result<RunningProcess, String> {
    if !CaptureSource::supports_app_capture() {
        return Err("指定應用程式擷取需要 Windows build 20348 或更新版本".into());
    }
    let Some(filename) = Path::new(executable).file_name().and_then(|n| n.to_str()) else {
        return Err("無效的應用程式執行檔路徑".into());
    };
    let processes = processes()?;
    let mut matching = HashMap::new();
    for process in &processes {
        if process.id == std::process::id() || !same_path(&process.executable, filename) {
            continue;
        }
        if let Some((path, handle)) = process_path(process.id, true)
            && same_path(&path, executable)
        {
            matching.insert(process.id, handle);
        }
    }
    // Browsers run many copies of their executable. Select the app root so a single
    // Process Loopback stream also captures its renderer and audio helper descendants.
    let candidate = processes
        .iter()
        .filter(|process| matching.contains_key(&process.id))
        .min_by_key(|process| (matching.contains_key(&process.parent), process.id));
    candidate
        .and_then(|p| {
            matching
                .remove(&p.id)
                .map(|handle| RunningProcess { id: p.id, handle })
        })
        .ok_or_else(|| format!("等待應用程式啟動或允許存取（{filename}）"))
}

pub(super) fn available_sources() -> Vec<CaptureSource> {
    let mut sources = vec![CaptureSource::System, CaptureSource::DefaultMicrophone];
    let Ok(_apartment) = Apartment::initialize() else {
        return sources;
    };
    let Ok(enumerator) = endpoint_enumerator() else {
        return sources;
    };
    sources.extend(
        active_endpoints(&enumerator, eCapture).filter_map(|device| {
            let id = endpoint_id(&device).ok()?;
            let name = endpoint_name(&device).unwrap_or_else(|| id.clone());
            Some(CaptureSource::Microphone { id, name })
        }),
    );
    if CaptureSource::supports_app_capture() {
        sources.extend(playback_applications(&enumerator));
    }
    sources
}

fn active_endpoints(
    enumerator: &IMMDeviceEnumerator,
    flow: EDataFlow,
) -> impl Iterator<Item = IMMDevice> + use<> {
    let collection = unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }.ok();
    let count = collection
        .as_ref()
        .and_then(|devices| unsafe { devices.GetCount() }.ok())
        .unwrap_or_default();
    (0..count).filter_map(move |index| unsafe { collection.as_ref()?.Item(index) }.ok())
}

fn playback_applications(enumerator: &IMMDeviceEnumerator) -> Vec<CaptureSource> {
    let mut seen_processes = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut applications: Vec<_> = active_endpoints(enumerator, eRender)
        .flat_map(|device| playback_sessions(&device).into_iter().flatten())
        .filter_map(playback_process_id)
        .filter(|&pid| seen_processes.insert(pid))
        .filter_map(|pid| {
            let (path, _) = process_path(pid, false)?;
            let name = Path::new(&path).file_name()?.to_string_lossy().into_owned();
            seen_paths
                .insert(path.to_lowercase())
                .then_some(CaptureSource::Application { path, name })
        })
        .collect();
    applications.sort_by_cached_key(|source| source.label().to_lowercase());
    applications
}

fn playback_sessions(
    device: &IMMDevice,
) -> windows::core::Result<impl Iterator<Item = IAudioSessionControl> + use<>> {
    let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }?;
    let sessions = unsafe { manager.GetSessionEnumerator() }?;
    let count = unsafe { sessions.GetCount() }?;
    Ok((0..count).filter_map(move |index| unsafe { sessions.GetSession(index) }.ok()))
}

fn playback_process_id(session: IAudioSessionControl) -> Option<u32> {
    // The Windows mixer keeps an inactive session while it still owns an audio stream.
    let state = unsafe { session.GetState() }.ok()?;
    if state != AudioSessionStateActive && state != AudioSessionStateInactive {
        return None;
    }
    let session: IAudioSessionControl2 = session.cast().ok()?;
    if unsafe { session.IsSystemSoundsSession() } != S_FALSE {
        return None;
    }
    let pid = unsafe { session.GetProcessId() }.ok()?;
    (pid != 0 && pid != std::process::id()).then_some(pid)
}

fn endpoint_name(device: &IMMDevice) -> Option<String> {
    const FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 14,
    };
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    let mut property = unsafe { store.GetValue(&FRIENDLY_NAME) }.ok()?;
    let result = unsafe { PropVariantToStringAlloc(&property) };
    let _ = unsafe { PropVariantClear(&mut property) };
    let name = result.ok()?;
    if name.is_null() {
        return None;
    }
    let value = unsafe { name.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(name.as_ptr().cast())) };
    value.filter(|text| !text.is_empty())
}

struct ProcessEntry {
    id: u32,
    parent: u32,
    executable: String,
}

fn processes() -> Result<Vec<ProcessEntry>, String> {
    let handle = OwnedHandle(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|e| audio_error("無法讀取應用程式清單", e))?,
    );
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    unsafe { Process32FirstW(handle.0, &mut entry) }
        .map_err(|e| audio_error("無法讀取應用程式清單", e))?;
    let mut list = Vec::new();
    loop {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        list.push(ProcessEntry {
            id: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            executable: String::from_utf16_lossy(&entry.szExeFile[..len]),
        });
        if unsafe { Process32NextW(handle.0, &mut entry) }.is_err() {
            break;
        }
    }
    Ok(list)
}

fn process_path(pid: u32, watch: bool) -> Option<(String, OwnedHandle)> {
    let permissions = PROCESS_QUERY_LIMITED_INFORMATION
        | if watch {
            PROCESS_SYNCHRONIZE
        } else {
            Default::default()
        };
    let handle = OwnedHandle(unsafe { OpenProcess(permissions, false, pid) }.ok()?);
    let mut buffer = [0u16; 32_768];
    let mut length = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .ok()?;
    Some((
        String::from_utf16_lossy(buffer.get(..length as usize)?),
        handle,
    ))
}

fn same_path(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
}

fn windows_build() -> u32 {
    use std::sync::OnceLock;
    static BUILD: OnceLock<u32> = OnceLock::new();

    #[repr(C)]
    struct VersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform_id: u32,
        service_pack: [u16; 128],
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(version: *mut VersionInfo) -> i32;
    }

    *BUILD.get_or_init(|| {
        let mut version = VersionInfo {
            size: size_of::<VersionInfo>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform_id: 0,
            service_pack: [0; 128],
        };
        if unsafe { RtlGetVersion(&mut version) } < 0 {
            return 0;
        }
        version.build
    })
}
