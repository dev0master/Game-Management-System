//! Stable identity for removable drives.
//!
//! Drive letters are not identity. On this machine a desktop shortcut points at
//! `F:\Ghost of Tsushimaa\...\GhostOfTsushima.exe`, but `F:` is currently a Zorin OS
//! installer stick — the letter was reassigned to a different device entirely. Steam's
//! own `libraryfolders.vdf` references `E:\SteamLibrary`, a path that no longer exists.
//! A catalogue keyed on letters would show games as available on the wrong physical
//! device, and a copy job would read from whatever happened to be mounted there.
//!
//! So a drive is keyed on its **volume GUID**, stable for the life of the volume, with
//! the **volume serial** recorded alongside because the serial travels with the
//! filesystem to other machines. The mount point is re-derived on every poll and is
//! never treated as identity.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDiskFreeSpaceExW,
    GetDriveTypeW, GetVolumeInformationW, GetVolumePathNamesForVolumeNameW,
    FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceProperty, IOCTL_STORAGE_QUERY_PROPERTY,
    STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// How a volume is physically attached.
///
/// This decides whether a drive is "external" — **not** `GetDriveType`. This machine's
/// USB games drive reports `DRIVE_FIXED` because the enclosure presents it as
/// non-removable, so keying externality on drive type would misclassify the user's
/// main games drive as an internal disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusType {
    Usb,
    Nvme,
    Sata,
    Scsi,
    Sd,
    Virtual,
    Other,
}

impl BusType {
    /// Values from the `STORAGE_BUS_TYPE` enumeration in `ntddstor.h`.
    fn from_raw(v: i32) -> Self {
        match v {
            0x07 => BusType::Usb,
            0x11 => BusType::Nvme,
            0x0B => BusType::Sata,
            0x01 | 0x0A => BusType::Scsi,
            0x0C | 0x0D => BusType::Sd,
            0x0E | 0x0F => BusType::Virtual,
            _ => BusType::Other,
        }
    }

    /// True for buses whose drives come and go — the ones the catalogue must be able
    /// to display while disconnected.
    pub fn is_removable_bus(self) -> bool {
        matches!(self, BusType::Usb | BusType::Sd)
    }
}

/// A volume as seen right now.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VolumeInfo {
    /// `\\?\Volume{GUID}\` — the stable local key.
    pub volume_guid: String,
    /// Filesystem serial as Windows displays it (e.g. `BEE4EA3B`).
    pub volume_serial: String,
    /// Current mount point, e.g. `E:\`. Derived, never identity. `None` when the
    /// volume has no drive letter.
    pub mount_point: Option<String>,
    pub label: String,
    pub filesystem: String,
    pub bus_type: BusType,
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// True when the filesystem has no journal, so an unsafe eject can leave a
    /// directory entry pointing at data that was never written. Drives the transfer
    /// engine's per-file flush policy and the "safe to unplug" indicator.
    pub is_unjournaled: bool,
    /// Largest single file the filesystem can hold. FAT32 caps just under 4 GiB, which
    /// matters because the user's RAR volumes are 8.5 GB each.
    pub max_file_bytes: Option<u64>,
}

impl VolumeInfo {
    /// Short label for the UI: the volume label if set, else the mount point.
    pub fn display_name(&self) -> String {
        if !self.label.is_empty() {
            self.label.clone()
        } else if let Some(m) = &self.mount_point {
            m.trim_end_matches('\\').to_string()
        } else {
            self.volume_guid.clone()
        }
    }

    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    /// True when this volume cannot store a file of `bytes`. Checked in transfer
    /// preflight so an 8.5 GB volume is refused before the copy starts rather than
    /// failing partway through.
    pub fn rejects_file_size(&self, bytes: u64) -> bool {
        self.max_file_bytes.is_some_and(|max| bytes > max)
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Enumerate every volume currently present.
///
/// Volumes are enumerated directly rather than by walking drive letters, so a volume
/// mounted without a letter, or mounted at a folder, is still catalogued.
pub fn enumerate() -> Vec<VolumeInfo> {
    let mut out = Vec::new();
    let mut buf = [0u16; 261];

    unsafe {
        let handle = FindFirstVolumeW(buf.as_mut_ptr(), buf.len() as u32);
        if handle == INVALID_HANDLE_VALUE {
            return out;
        }
        loop {
            let guid = from_wide(&buf);
            if let Some(info) = describe(&guid) {
                out.push(info);
            }
            buf = [0u16; 261];
            if FindNextVolumeW(handle, buf.as_mut_ptr(), buf.len() as u32) == 0 {
                break;
            }
        }
        FindVolumeClose(handle);
    }
    out
}

/// Describe one volume by its `\\?\Volume{GUID}\` path.
pub fn describe(volume_guid: &str) -> Option<VolumeInfo> {
    let guid_w = wide(volume_guid);
    let mut label_buf = [0u16; 261];
    let mut fs_buf = [0u16; 261];
    let mut serial: u32 = 0;

    // Fails for a volume with no media — an empty card reader — which is exactly the
    // case we want to skip rather than report as a phantom drive.
    let ok = unsafe {
        GetVolumeInformationW(
            guid_w.as_ptr(),
            label_buf.as_mut_ptr(),
            label_buf.len() as u32,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_buf.as_mut_ptr(),
            fs_buf.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }

    let filesystem = from_wide(&fs_buf);
    let fs_upper = filesystem.to_uppercase();
    let (total_bytes, free_bytes) = free_space(volume_guid).unwrap_or((0, 0));

    Some(VolumeInfo {
        volume_guid: volume_guid.to_string(),
        volume_serial: format!("{serial:08X}"),
        mount_point: first_mount_point(volume_guid),
        label: from_wide(&label_buf),
        bus_type: query_bus_type(volume_guid).unwrap_or(BusType::Other),
        total_bytes,
        free_bytes,
        // NTFS is the only journalled filesystem in normal use on Windows removable
        // media; exFAT and the FAT family are not.
        is_unjournaled: !fs_upper.is_empty() && fs_upper != "NTFS",
        max_file_bytes: match fs_upper.as_str() {
            "FAT32" | "FAT" | "FAT16" => Some(4 * 1024 * 1024 * 1024 - 1),
            _ => None,
        },
        filesystem,
    })
}

/// First drive letter or mount folder for a volume, if it has one.
fn first_mount_point(volume_guid: &str) -> Option<String> {
    let guid_w = wide(volume_guid);
    let mut len: u32 = 0;
    unsafe {
        // The sizing call is expected to fail with ERROR_MORE_DATA when names exist.
        GetVolumePathNamesForVolumeNameW(guid_w.as_ptr(), std::ptr::null_mut(), 0, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if GetVolumePathNamesForVolumeNameW(
            guid_w.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut len,
        ) == 0
        {
            return None;
        }
        let first = from_wide(&buf);
        (!first.is_empty()).then_some(first)
    }
}

fn free_space(path: &str) -> Option<(u64, u64)> {
    let w = wide(path);
    let mut free_to_caller: u64 = 0;
    let mut total: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            w.as_ptr(),
            &mut free_to_caller,
            &mut total,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some((total, free_to_caller))
}

/// Ask the storage driver how the device is attached.
///
/// The handle is opened with zero access rights: `IOCTL_STORAGE_QUERY_PROPERTY` is an
/// informational query, and requesting no access avoids needing administrator rights.
fn query_bus_type(volume_guid: &str) -> Option<BusType> {
    // A device path must not carry the trailing backslash that volume GUID paths do.
    let w = wide(volume_guid.trim_end_matches('\\'));

    unsafe {
        let handle = CreateFileW(
            w.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        // The descriptor is variable-length; only its fixed header is needed.
        let mut buf = [0u8; 1024];
        let mut returned: u32 = 0;

        let ok = DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const c_void,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        );
        CloseHandle(handle);

        if ok == 0 || (returned as usize) < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
            return None;
        }
        let desc = &*(buf.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR);
        Some(BusType::from_raw(desc.BusType as i32))
    }
}

/// Map currently-present volumes by GUID, for reconciling against the catalogue.
pub fn by_guid() -> HashMap<String, VolumeInfo> {
    enumerate()
        .into_iter()
        .map(|v| (v.volume_guid.clone(), v))
        .collect()
}

/// Resolve a path to the volume GUID of the drive holding it.
///
/// Called as soon as the user picks a folder to scan, so a drive letter is converted
/// to stable identity immediately and nothing downstream ever stores a letter.
pub fn guid_for_path(path: &Path) -> Option<String> {
    let root = path.components().next()?;
    let root_str = format!(
        "{}\\",
        root.as_os_str().to_string_lossy().trim_end_matches('\\')
    );
    enumerate()
        .into_iter()
        .find(|v| {
            v.mount_point
                .as_deref()
                .is_some_and(|m| m.eq_ignore_ascii_case(&root_str))
        })
        .map(|v| v.volume_guid)
}

/// Drive type as Windows reports it. Kept for display only — see [`BusType`] for why
/// it must not decide whether a drive is external.
pub fn raw_drive_type(mount: &str) -> u32 {
    let w = wide(mount);
    unsafe { GetDriveTypeW(w.as_ptr()) }
}

/// Scan roots worth offering for a volume: its mount point, when it has one.
pub fn default_scan_roots(v: &VolumeInfo) -> Vec<PathBuf> {
    v.mount_point.iter().map(PathBuf::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs against the real machine, asserting only invariants that hold on any
    /// Windows system, so it stays valid as drives are plugged and unplugged.
    #[test]
    fn enumerates_volumes_with_stable_identity() {
        let vols = enumerate();
        assert!(!vols.is_empty(), "a Windows machine always has a volume");
        for v in &vols {
            assert!(
                v.volume_guid.starts_with("\\\\?\\Volume{"),
                "identity must be a volume GUID path, got {:?}",
                v.volume_guid
            );
            assert_eq!(v.volume_serial.len(), 8, "serial is 8 hex digits");
            assert!(v.total_bytes >= v.free_bytes);
        }
        let mut guids: Vec<&str> = vols.iter().map(|v| v.volume_guid.as_str()).collect();
        guids.sort_unstable();
        let before = guids.len();
        guids.dedup();
        assert_eq!(before, guids.len(), "volume GUIDs must be unique");
    }

    /// The journalling flag drives the transfer engine's durability policy, so it has
    /// to be correct for the filesystems actually in use on this machine.
    #[test]
    fn journalling_is_derived_from_the_filesystem() {
        for v in enumerate() {
            match v.filesystem.to_uppercase().as_str() {
                "NTFS" => assert!(!v.is_unjournaled, "NTFS is journalled"),
                "EXFAT" | "FAT32" | "FAT" => {
                    assert!(v.is_unjournaled, "{} is not journalled", v.filesystem)
                }
                _ => {}
            }
        }
    }

    /// FAT32 cannot hold a file of 4 GiB or more, and the user's RAR volumes are
    /// 8.5 GB, so transfer preflight depends on this being populated.
    #[test]
    fn fat32_reports_a_max_file_size_that_rejects_large_volumes() {
        for v in enumerate() {
            if v.filesystem.eq_ignore_ascii_case("FAT32") {
                assert_eq!(v.max_file_bytes, Some(4 * 1024 * 1024 * 1024 - 1));
                assert!(v.rejects_file_size(8_704 * 1024 * 1024));
            } else if v.filesystem.eq_ignore_ascii_case("NTFS") {
                assert_eq!(v.max_file_bytes, None);
                assert!(!v.rejects_file_size(u64::MAX));
            }
        }
    }

    #[test]
    fn a_mounted_path_resolves_to_a_guid() {
        let guid = guid_for_path(Path::new("C:\\")).expect("C:\\ must resolve");
        assert!(guid.starts_with("\\\\?\\Volume{"));
    }

    /// The system disk is not on a removable bus; asserting this guards the
    /// bus-type decode from silently returning a constant.
    #[test]
    fn system_drive_is_not_reported_as_removable() {
        let guid = guid_for_path(Path::new("C:\\")).unwrap();
        let v = describe(&guid).unwrap();
        assert!(!v.bus_type.is_removable_bus(), "got {:?}", v.bus_type);
    }
}
