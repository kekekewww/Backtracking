//! Windows fonts for the text the app does not write itself.
//!
//! The built-in typeface covers Latin, Greek and Cyrillic. Device names, folder paths and
//! Windows error messages can be in any language, and a character without a glyph shows as
//! an empty box. The fonts Windows ships fill the gaps. They are mapped, not read: a CJK
//! font is tens of megabytes, and a mapping leaves it in the file cache and touches only
//! the pages a glyph needs.

use std::fs::File;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::Arc;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READONLY,
    UnmapViewOfFile,
};

/// A whole file mapped read-only for as long as this value lives.
struct MappedFile {
    view: *const u8,
    length: usize,
}

// The view is read-only and never remapped, so any thread may read it.
unsafe impl Send for MappedFile {}
unsafe impl Sync for MappedFile {}

impl MappedFile {
    fn open(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let length = usize::try_from(file.metadata().ok()?.len()).ok()?;
        // An empty file cannot be mapped, and is not a font either.
        if length == 0 {
            return None;
        }
        unsafe {
            let mapping =
                CreateFileMappingW(file.as_raw_handle().cast(), null(), PAGE_READONLY, 0, 0, null());
            if mapping.is_null() {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            // The view keeps the mapping alive, and the mapping keeps the file.
            CloseHandle(mapping);
            (!view.Value.is_null()).then(|| Self { view: view.Value.cast(), length })
        }
    }
}

impl AsRef<[u8]> for MappedFile {
    fn as_ref(&self) -> &[u8] {
        // The view covers `length` bytes and stays mapped until `self` is dropped.
        unsafe { std::slice::from_raw_parts(self.view, self.length) }
    }
}

impl Drop for MappedFile {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view.cast_mut().cast() });
        }
    }
}

const SEGOE: &str = "segoeui.ttf";
const TRADITIONAL_CHINESE: &str = "msjh.ttc";
const SIMPLIFIED_CHINESE: &str = "msyh.ttc";
const JAPANESE: &str = "YuGothM.ttc";
const KOREAN: &str = "malgun.ttf";

/// The font files to fall back on, most wanted first.
///
/// Chinese, Japanese and Korean share many characters but draw them differently, so the
/// font of the user's own language (a Windows language identifier) goes first. Every other
/// one follows, because a device or a folder can be named in any of them.
fn candidates(language: u16) -> [&'static str; 5] {
    const CHINESE: u16 = 0x04;
    const JAPANESE_LANGUAGE: u16 = 0x11;
    const KOREAN_LANGUAGE: u16 = 0x12;
    // Mainland China and Singapore write simplified characters.
    const SIMPLIFIED_REGIONS: [u16; 2] = [0x02, 0x04];
    let (primary, region) = (language & 0x3ff, language >> 10);
    let cjk = match primary {
        CHINESE if SIMPLIFIED_REGIONS.contains(&region) => {
            [SIMPLIFIED_CHINESE, TRADITIONAL_CHINESE, JAPANESE, KOREAN]
        }
        JAPANESE_LANGUAGE => [JAPANESE, TRADITIONAL_CHINESE, SIMPLIFIED_CHINESE, KOREAN],
        KOREAN_LANGUAGE => [KOREAN, TRADITIONAL_CHINESE, SIMPLIFIED_CHINESE, JAPANESE],
        _ => [TRADITIONAL_CHINESE, SIMPLIFIED_CHINESE, JAPANESE, KOREAN],
    };
    // Segoe UI adds the rest of Latin, plus Arabic, Hebrew and more, and has no CJK.
    [SEGOE, cjk[0], cjk[1], cjk[2], cjk[3]]
}

fn fonts_folder() -> PathBuf {
    ["WINDIR", "SystemRoot"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts")
}

/// Appends every installed fallback font to the end of every family in `fonts`, so the
/// app's own typefaces still draw whatever they can.
pub fn add_system_font_fallbacks(fonts: &mut egui::FontDefinitions) {
    let folder = fonts_folder();
    for file in candidates(unsafe { GetUserDefaultUILanguage() }) {
        let Some(mapped) = MappedFile::open(&folder.join(file)) else {
            continue;
        };
        let name = format!("system-{file}");
        fonts
            .font_data
            .insert(name.clone(), Arc::new(egui::FontData::from_blob(Arc::new(mapped), 0)));
        for family in fonts.families.values_mut() {
            family.push(name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_language_picks_which_cjk_font_draws_shared_characters() {
        let first_cjk = |language| candidates(language)[1];
        assert_eq!(first_cjk(0x0404), TRADITIONAL_CHINESE); // Taiwan
        assert_eq!(first_cjk(0x0c04), TRADITIONAL_CHINESE); // Hong Kong
        assert_eq!(first_cjk(0x0804), SIMPLIFIED_CHINESE); // Mainland China
        assert_eq!(first_cjk(0x1004), SIMPLIFIED_CHINESE); // Singapore
        assert_eq!(first_cjk(0x0411), JAPANESE);
        assert_eq!(first_cjk(0x0412), KOREAN);
        assert_eq!(first_cjk(0x0409), TRADITIONAL_CHINESE); // English (United States)

        // Whatever the language, Segoe UI leads and every font is offered once.
        for language in [0x0404, 0x0804, 0x0411, 0x0412, 0x0409, 0x0407] {
            let mut files = candidates(language);
            assert_eq!(files[0], SEGOE);
            files.sort_unstable();
            let mut all = [SEGOE, TRADITIONAL_CHINESE, SIMPLIFIED_CHINESE, JAPANESE, KOREAN];
            all.sort_unstable();
            assert_eq!(files, all);
        }
    }

    #[test]
    fn a_mapped_file_reads_back_its_bytes_and_bad_files_are_skipped() {
        let folder = std::env::temp_dir().join(format!("backtracking-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("bytes.bin");
        let bytes: Vec<u8> = (0..=255).cycle().take(70_000).collect();
        std::fs::write(&path, &bytes).unwrap();

        let mapped = MappedFile::open(&path).expect("map an ordinary file");
        assert_eq!(mapped.as_ref(), bytes.as_slice());
        drop(mapped);

        let empty = folder.join("empty.bin");
        std::fs::write(&empty, []).unwrap();
        assert!(MappedFile::open(&empty).is_none());
        assert!(MappedFile::open(&folder.join("missing.bin")).is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn fallbacks_come_after_the_apps_own_fonts_in_every_family() {
        let mut fonts = egui::FontDefinitions::default();
        let own: Vec<_> = fonts
            .families
            .iter()
            .map(|(family, names)| (family.clone(), names.clone()))
            .collect();
        add_system_font_fallbacks(&mut fonts);

        // Segoe UI ships with every Windows this app runs on.
        let segoe = format!("system-{SEGOE}");
        assert!(fonts.font_data.contains_key(&segoe));
        for (family, before) in own {
            let after = &fonts.families[&family];
            assert_eq!(after[..before.len()], before[..], "{family:?}");
            assert!(after[before.len()..].contains(&segoe), "{family:?}");
            assert!(after[before.len()..].iter().all(|name| name.starts_with("system-")));
        }
    }
}
