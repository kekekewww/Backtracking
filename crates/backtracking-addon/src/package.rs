//! Inspect, install and build add-on packages without following package paths outside their root.

#[cfg(target_os = "macos")]
pub mod quarantine;

use crate::manifest::{validate_id, validate_relative_path, Manifest};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zip::{ZipArchive, ZipWriter};

pub const MAX_ENTRIES: usize = 4096;
pub const MAX_UNPACKED_SIZE: u64 = 512 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Information shown before the user decides whether to install a package.
#[derive(Debug)]
pub struct PackageInfo {
    pub manifest: Manifest,
    pub unpacked_size: u64,
    pub entry_count: usize,
    pub file_count: usize,
    /// The program selected for the running platform, relative to the install folder.
    pub program: String,
}

/// An installed folder, including folders whose manifest or program is broken.
#[derive(Debug)]
pub struct InstalledAddon {
    pub path: PathBuf,
    pub manifest: Result<Manifest>,
}

/// Validate every entry and its decompressed contents without writing any files.
pub fn inspect(path: impl AsRef<Path>) -> Result<PackageInfo> {
    let mut archive = open_archive(path.as_ref())?;
    inspect_archive(&mut archive)
}

/// Install beside the destination, then replace the old folder only after extraction succeeds.
///
/// An invalid package is rejected before the add-ons folder is created. A failed replacement
/// restores the old folder. A backup left by an interrupted process is not an installed add-on.
pub fn install(package: impl AsRef<Path>, addons: impl AsRef<Path>) -> Result<PackageInfo> {
    require_extension(package.as_ref())?;
    let file = File::open(package)?;
    #[cfg(target_os = "macos")]
    let quarantine = quarantine::Quarantine::read(&file)?;
    let mut archive = ZipArchive::new(file).map_err(zip_error)?;
    let info = inspect_archive(&mut archive)?;
    let addons = addons.as_ref();
    install_dir(addons, true)?;
    let addons = fs::canonicalize(addons)?;
    let _lock = OperationLock::acquire(&addons, &info.manifest.id)?;
    let target = addons.join(&info.manifest.id);
    reject_link(&target)?;
    let stage = create_stage(&addons, &info.manifest.id)?;
    let mut cleanup = CleanupPath(stage.clone());
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(zip_error)?;
        let name = archive_name(&entry)?;
        let destination = stage.join(name.trim_end_matches('/'));
        if entry.is_dir() {
            install_dir(&destination, true)?;
        } else {
            if let Some(parent) = destination.parent() {
                install_dir(parent, true)?;
            }
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?;
            let copied =
                std::io::copy(&mut entry.by_ref().take(MAX_UNPACKED_SIZE + 1), &mut output)?;
            if copied != entry.size() || copied > MAX_UNPACKED_SIZE {
                return Err(Error::new("Package changed while installing; its unpacked size does not match the inspected package."));
            }
            #[cfg(target_os = "macos")]
            quarantine.apply(&output)?;
            output.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = entry.unix_mode() {
                    // Retain executability but do not install setuid, setgid or sticky bits.
                    fs::set_permissions(&destination, fs::Permissions::from_mode(mode & 0o777))?;
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;
        let program = stage.join(&info.program);
        let mode = fs::metadata(&program)?.permissions().mode() & 0o777;
        fs::set_permissions(program, fs::Permissions::from_mode(mode | 0o500))?;
    }
    replace_install(&stage, &target, &addons, &info.manifest.id)?;
    cleanup.0 = PathBuf::new();
    Ok(info)
}

fn replace_install(stage: &Path, target: &Path, addons: &Path, id: &str) -> Result<()> {
    if target.exists() {
        if !target.is_dir() {
            return Err(Error::new(format!(
                "Cannot replace {}: it is not a folder.",
                target.display()
            )));
        }
        #[cfg(target_os = "macos")]
        if swap_directories(stage, target)? {
            // The previous version now occupies the private stage.
            let _ = fs::remove_dir_all(stage);
            return Ok(());
        }
        let backup = unused_path(addons, id, "backup");
        fs::rename(target, &backup)?;
        if let Err(error) = fs::rename(stage, target) {
            if let Err(rollback) = fs::rename(&backup, target) {
                return Err(Error::new(format!(
                    "Could not install the add-on ({error}) or restore its previous folder ({rollback}). The previous version is intact at {}.",
                    backup.display()
                )));
            }
            return Err(Error::new(format!(
                "Could not install the add-on; the previous version was restored: {error}"
            )));
        }
        // A loaded executable can prevent cleanup on Windows. Keep the successful install and
        // leave its hidden backup for cleanup rather than report that replacement failed.
        let _ = fs::remove_dir_all(backup);
    } else {
        fs::rename(stage, target)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn swap_directories(stage: &Path, target: &Path) -> std::io::Result<bool> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    extern "C" {
        fn renamex_np(
            from: *const std::ffi::c_char,
            to: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    let from = CString::new(stage.as_os_str().as_bytes())?;
    let to = CString::new(target.as_os_str().as_bytes())?;
    const RENAME_SWAP: u32 = 2;
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_SWAP) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::Unsupported {
        return Ok(false);
    }
    Err(error)
}

/// Validate an installed or privately renamed program folder against its intended id.
pub fn validate_installed_folder(path: &Path, id: &str) -> Result<Manifest> {
    reject_link(path)?;
    #[cfg(target_os = "macos")]
    if !fs::symlink_metadata(path.join("addon.json"))?.is_file() {
        return Err(Error::new("Expected addon.json to be a regular file."));
    }
    let manifest = read_folder_manifest(path)?;
    if id != manifest.id {
        return Err(Error::new(format!(
            "Folder {} does not match manifest id {}. Reinstall the add-on.",
            path.display(),
            manifest.id
        )));
    }
    validate_folder_programs(path, &manifest)?;
    Ok(manifest)
}

/// Read each installed folder. Unreadable entries and invalid manifests are returned individually.
pub fn list_installed(addons: impl AsRef<Path>) -> Result<Vec<InstalledAddon>> {
    let addons = addons.as_ref();
    let entries = match fs::read_dir(addons) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    list_installed_entries(addons, entries, |path| fs::symlink_metadata(path))
}

fn list_installed_entries(
    addons: &Path,
    entries: impl IntoIterator<Item = std::io::Result<fs::DirEntry>>,
    metadata: impl Fn(&Path) -> std::io::Result<Metadata>,
) -> Result<Vec<InstalledAddon>> {
    let mut installed = Vec::new();
    for (index, entry) in entries.into_iter().enumerate() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                installed.push(InstalledAddon {
                    // A failed directory read does not provide a name. This label cannot be
                    // mistaken for a valid add-on id by install or removal operations.
                    path: addons.join(format!("(unreadable entry {})", index + 1)),
                    manifest: Err(error.into()),
                });
                continue;
            }
        };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let metadata = match metadata(&entry.path()) {
            Ok(metadata) => metadata,
            Err(error) => {
                installed.push(InstalledAddon {
                    path: entry.path(),
                    manifest: Err(error.into()),
                });
                continue;
            }
        };
        if !metadata.is_dir() && !is_link(&metadata) {
            continue;
        }
        let path = entry.path();
        let manifest = validate_installed_folder(&path, entry.file_name().to_str().unwrap_or(""));
        installed.push(InstalledAddon { path, manifest });
    }
    installed.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(installed)
}

/// Parse a private transaction basename (without its leading dot) as generated by the installer.
#[cfg(unix)]
pub fn transaction_id<'a>(name: &'a str, separator: &str) -> Option<&'a str> {
    let (id, suffix) = name.rsplit_once(separator)?;
    validate_id(id).ok()?;
    let (pid, sequence) = suffix.split_once('-')?;
    if !suffix.bytes().all(|b| b == b'-' || b.is_ascii_digit()) {
        return None;
    }
    pid.parse::<u32>().ok()?;
    sequence.parse::<u64>().ok()?;
    Some(id)
}

#[cfg(target_os = "macos")]
fn removal_backup(addons: &Path, id: &str) -> Result<Option<PathBuf>> {
    let mut backup = None;
    for entry in fs::read_dir(addons)? {
        let entry = entry?;
        let name = entry.file_name();
        let owner = name
            .to_str()
            .and_then(|name| name.strip_prefix('.'))
            .and_then(|name| transaction_id(name, ".backup-"));
        if owner != Some(id) {
            continue;
        }
        // A reserved name alone does not authorize deleting unrelated contents.
        let path = entry.path();
        validate_installed_folder(&path, id).map_err(|error| {
            Error::new(format!(
                "Cannot remove {id}: unverified backup {}: {error}",
                path.display()
            ))
        })?;
        if backup.replace(path).is_some() {
            return Err(Error::new(format!(
                "Cannot remove {id}: multiple valid backups"
            )));
        }
    }
    Ok(backup)
}

/// Remove the program folder only. The caller handles the user's choice to delete add-on data.
pub fn remove(addons: impl AsRef<Path>, id: &str) -> Result<()> {
    validate_id(id)?;
    let addons = addons.as_ref();
    #[cfg(target_os = "macos")]
    reject_link(addons)?;
    if !addons.exists() {
        return Ok(());
    }
    let addons = fs::canonicalize(addons)?;
    let _lock = OperationLock::acquire(&addons, id)?;
    let target = addons.join(id);
    reject_link(&target)?;
    #[cfg(target_os = "macos")]
    let backup = removal_backup(&addons, id)?;
    if target.exists() {
        if fs::canonicalize(&target)?.parent() != Some(addons.as_path()) {
            return Err(Error::new(
                "The installed add-on folder escapes the add-ons folder.",
            ));
        }
        fs::remove_dir_all(&target)?;
    }
    #[cfg(target_os = "macos")]
    if let Some(backup) = backup {
        // Never recursively delete a recoverable backup in place. Keep it complete
        // until the visible slot is empty, then move it there under the same lock.
        // A failed final deletion leaves a visible package that remove can retry.
        fs::rename(backup, &target)?;
        fs::remove_dir_all(target)?;
    }
    Ok(())
}

/// Validate a folder, including every platform's program, and write a `.btaddon` ZIP.
pub fn pack(folder: impl AsRef<Path>, output: impl AsRef<Path>) -> Result<PackageInfo> {
    let folder = fs::canonicalize(folder)?;
    let output = output.as_ref();
    require_extension(output)?;
    let manifest = read_folder_manifest(&folder)?;
    validate_folder_programs(&folder, &manifest)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent)?;
    if parent.starts_with(&folder) {
        return Err(Error::new(
            "Write the .btaddon file outside the folder being packaged.",
        ));
    }
    let mut entries = Vec::new();
    collect_folder(&folder, &folder, &mut entries)?;
    let mut size = 0_u64;
    let mut files = 0;
    for (_, metadata) in &entries {
        if metadata.is_file() {
            size = size.checked_add(metadata.len()).ok_or_else(size_error)?;
            files += 1;
        }
    }
    if size > MAX_UNPACKED_SIZE {
        return Err(size_error());
    }
    let stage = unused_path(&parent, &manifest.id, "pack");
    let _cleanup = CleanupPath(stage.clone());
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)?;
    let mut zip = ZipWriter::new(file);
    for (name, metadata) in &entries {
        let mut options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            options = options.unix_permissions(metadata.permissions().mode() & 0o777);
        }
        #[cfg(not(unix))]
        {
            // Extensionless and .exe files may be launched on the platforms they target.
            options = options.unix_permissions(
                if manifest.run.values().any(|run| run.program == *name) {
                    0o755
                } else {
                    0o644
                },
            );
        }
        if metadata.is_dir() {
            zip.add_directory(format!("{name}/"), options)
                .map_err(zip_error)?;
        } else {
            zip.start_file(name, options).map_err(zip_error)?;
            let mut input = File::open(folder.join(name))?;
            let copied = std::io::copy(
                &mut Read::by_ref(&mut input).take(metadata.len() + 1),
                &mut zip,
            )?;
            if copied != metadata.len() {
                return Err(Error::new(format!(
                    "File {name} changed size while packaging; try again."
                )));
            }
        }
    }
    zip.finish().map_err(zip_error)?.sync_all()?;
    // Inspect the finished ZIP before exposing it, including CRC and decompressed lengths.
    let mut archive = ZipArchive::new(File::open(&stage)?).map_err(zip_error)?;
    let info = inspect_archive(&mut archive)?;
    drop(archive);
    if output.exists() {
        return Err(Error::new(format!(
            "{} already exists. Choose another output file or remove it first.",
            output.display()
        )));
    }
    fs::rename(stage, output)?;
    debug_assert_eq!(info.unpacked_size, size);
    debug_assert_eq!(info.file_count, files);
    Ok(info)
}

fn open_archive(path: &Path) -> Result<ZipArchive<File>> {
    require_extension(path)?;
    ZipArchive::new(File::open(path)?).map_err(zip_error)
}

fn require_extension(path: &Path) -> Result<()> {
    if !path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("btaddon"))
    {
        return Err(Error::new(
            "An add-on package must have the .btaddon extension.",
        ));
    }
    Ok(())
}

struct PackagePath {
    text: String,
    #[cfg(windows)]
    wide: Vec<u16>,
    #[cfg(not(windows))]
    folded: String,
}

impl PackagePath {
    fn new(path: &str) -> Self {
        Self {
            text: path.into(),
            #[cfg(windows)]
            wide: path.encode_utf16().collect(),
            #[cfg(not(windows))]
            folded: path.to_uppercase(),
        }
    }
}

impl PartialEq for PackagePath {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for PackagePath {}

impl PartialOrd for PackagePath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackagePath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        #[cfg(windows)]
        {
            #[link(name = "kernel32")]
            extern "system" {
                fn CompareStringOrdinal(
                    first: *const u16,
                    first_len: i32,
                    second: *const u16,
                    second_len: i32,
                    ignore_case: i32,
                ) -> i32;
            }
            // Use Windows' ordinal uppercase table, not linguistic casing (which can expand
            // one character into several). ZIP path lengths fit in an i32.
            let comparison = unsafe {
                CompareStringOrdinal(
                    self.wide.as_ptr(),
                    self.wide.len() as i32,
                    other.wide.as_ptr(),
                    other.wide.len() as i32,
                    1,
                )
            };
            match comparison {
                1 => std::cmp::Ordering::Less,
                2 => std::cmp::Ordering::Equal,
                3 => std::cmp::Ordering::Greater,
                _ => unreachable!("valid UTF-16 paths and lengths"),
            }
        }
        #[cfg(not(windows))]
        self.folded.cmp(&other.folded)
    }
}

fn inspect_archive(archive: &mut ZipArchive<File>) -> Result<PackageInfo> {
    if archive.len() > MAX_ENTRIES {
        return Err(Error::new(
            "An add-on package may contain at most 4096 ZIP entries.",
        ));
    }
    let mut paths = BTreeMap::new();
    let mut files = BTreeSet::new();
    let mut unpacked_size = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(zip_error)?;
        let name = archive_name(&entry)?;
        let normalized = PackagePath::new(name.trim_end_matches('/'));
        if paths.insert(normalized, entry.is_dir()).is_some() {
            return Err(Error::new(format!(
                "Package has duplicate paths: {name}. Rename one of them."
            )));
        }
        if !entry.is_dir() {
            files.insert(name);
        } else if entry.size() != 0 {
            return Err(Error::new("A ZIP directory entry must not contain data."));
        }
        unpacked_size = unpacked_size
            .checked_add(entry.size())
            .ok_or_else(size_error)?;
        if unpacked_size > MAX_UNPACKED_SIZE {
            return Err(size_error());
        }
    }
    for name in paths.keys() {
        let mut ancestor = name.text.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if paths.get(&PackagePath::new(parent)) == Some(&false) {
                return Err(Error::new(format!(
                    "Package path {parent} is both a file and a parent folder."
                )));
            }
            ancestor = parent;
        }
    }
    if !files.contains("addon.json") {
        return Err(Error::new(
            "The package must contain addon.json at its root.",
        ));
    }
    let mut manifest = None;
    let mut actual_total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(zip_error)?;
        if entry.is_dir() {
            continue;
        }
        let expected = entry.size();
        let remaining = MAX_UNPACKED_SIZE - actual_total;
        let copied = if entry.name() == "addon.json" {
            let mut content = Vec::new();
            let copied = entry
                .by_ref()
                .take(remaining + 1)
                .read_to_end(&mut content)? as u64;
            let text = std::str::from_utf8(&content)
                .map_err(|error| Error::new(format!("addon.json must be UTF-8: {error}")))?;
            manifest = Some(Manifest::parse(text)?);
            copied
        } else {
            std::io::copy(
                &mut entry.by_ref().take(remaining + 1),
                &mut std::io::sink(),
            )?
        };
        actual_total += copied;
        if actual_total > MAX_UNPACKED_SIZE {
            return Err(size_error());
        }
        if copied != expected {
            return Err(Error::new(
                "A ZIP entry's actual unpacked size does not match its declared size.",
            ));
        }
    }
    let manifest =
        manifest.ok_or_else(|| Error::new("The package must contain addon.json at its root."))?;
    for (platform, run) in &manifest.run {
        if !files.contains(&run.program) {
            return Err(Error::new(format!(
                "Program {} for {platform} is missing from the package.",
                run.program
            )));
        }
    }
    let program = manifest.current_run()?.program.clone();
    Ok(PackageInfo {
        manifest,
        unpacked_size,
        entry_count: archive.len(),
        file_count: files.len(),
        program,
    })
}

fn archive_name<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> Result<String> {
    let name = std::str::from_utf8(entry.name_raw())
        .map_err(|_| Error::new("ZIP entry names must be UTF-8."))?;
    validate_relative_path(name.trim_end_matches('/'))?;
    if entry.is_symlink() {
        return Err(Error::new(format!(
            "Package entry {name} is a symbolic link. Include the actual file instead."
        )));
    }
    if let Some(mode) = entry.unix_mode() {
        let kind = mode & 0o170000;
        if kind != 0 && kind != 0o100000 && kind != 0o040000 {
            return Err(Error::new(format!(
                "Package entry {name} is not a regular file or folder."
            )));
        }
    }
    Ok(name.to_owned())
}

fn read_folder_manifest(folder: &Path) -> Result<Manifest> {
    reject_link(&folder.join("addon.json"))?;
    Manifest::parse(&fs::read_to_string(folder.join("addon.json"))?)
}

fn validate_folder_programs(folder: &Path, manifest: &Manifest) -> Result<()> {
    for (platform, run) in &manifest.run {
        let path = folder.join(&run.program);
        let mut component = folder.to_path_buf();
        for part in run.program.split('/') {
            component.push(part);
            reject_link(&component)?;
        }
        if !path.is_file() {
            return Err(Error::new(format!(
                "Program {} for {platform} is missing or is not a file.",
                run.program
            )));
        }
    }
    Ok(())
}

fn collect_folder(root: &Path, folder: &Path, output: &mut Vec<(String, Metadata)>) -> Result<()> {
    let mut entries = fs::read_dir(folder)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let metadata = fs::symlink_metadata(entry.path())?;
        if is_link(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
            return Err(Error::new(format!(
                "{} is a symbolic link or special file. Include only regular files and folders.",
                entry.path().display()
            )));
        }
        let path = entry.path();
        let name = path
            .strip_prefix(root)
            .map_err(|error| Error::new(error.to_string()))?
            .to_str()
            .ok_or_else(|| Error::new("Package paths must be UTF-8."))?
            .replace('\\', "/");
        validate_relative_path(&name)?;
        let is_dir = metadata.is_dir();
        output.push((name, metadata));
        if output.len() > MAX_ENTRIES {
            return Err(Error::new(
                "An add-on package may contain at most 4096 ZIP entries.",
            ));
        }
        if is_dir {
            collect_folder(root, &path, output)?;
        }
    }
    Ok(())
}

fn zip_error(error: zip::result::ZipError) -> Error {
    Error::new(format!("Cannot read or write the add-on ZIP: {error}"))
}

fn size_error() -> Error {
    Error::new("An add-on package may unpack to at most 512 MiB.")
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn reject_link(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) => Err(Error::new(format!(
            "{} is a symbolic link or reparse point; remove it before continuing.",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn unused_path(parent: &Path, id: &str, purpose: &str) -> PathBuf {
    loop {
        let path = parent.join(format!(
            ".{id}.{purpose}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        if !path.exists() {
            return path;
        }
    }
}

fn install_dir(path: &Path, recursive: bool) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.recursive(recursive).create(path)
}

fn create_stage(parent: &Path, id: &str) -> Result<PathBuf> {
    loop {
        let path = unused_path(parent, id, "install");
        match install_dir(&path, false) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

struct CleanupPath(PathBuf);

impl Drop for CleanupPath {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            if self.0.is_dir() {
                let _ = fs::remove_dir_all(&self.0);
            } else {
                let _ = fs::remove_file(&self.0);
            }
        }
    }
}

struct OperationLock(PathBuf);

impl OperationLock {
    fn acquire(parent: &Path, id: &str) -> Result<Self> {
        let path = parent.join(format!(".{id}.lock"));
        OpenOptions::new().write(true).create_new(true).open(&path).map_err(|error| {
            Error::new(format!("Cannot change add-on {id}: {error}. Another install or removal may be running; an interrupted operation may have left {}.", path.display()))
        })?;
        Ok(Self(path))
    }
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Seek, SeekFrom, Write};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(target_os = "macos")]
    #[test]
    fn rc1_directory_exchange_publishes_whole_stage() {
        let dir = TestDir::new();
        let old = dir.0.join("com.example.test");
        let new = dir.0.join(".com.example.test.install-1-1");
        for (folder, contents) in [(&old, "old"), (&new, "new")] {
            fs::create_dir(folder).unwrap();
            fs::write(folder.join(contents), contents).unwrap();
        }
        assert!(swap_directories(&new, &old).unwrap());
        assert_eq!(fs::read(old.join("new")).unwrap(), b"new");
        assert_eq!(fs::read(new.join("old")).unwrap(), b"old");
    }

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "backtracking-package-{}-{}",
                std::process::id(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn manifest(version: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "contract": 1, "id": "com.example.tone", "name": "Tone", "version": version,
            "author": "Example", "description": "Test", "capabilities": ["source"],
            "run": { crate::manifest::current_platform().unwrap(): {"program":"bin/tone", "args":[]}}
        })).unwrap()
    }

    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn inspect_returns_manifest_sizes_and_selected_program_without_unpacking() {
        let dir = TestDir::new();
        let path = dir.0.join("tone.btaddon");
        let data = manifest("1");
        archive(&path, &[("addon.json", &data), ("bin/tone", b"program")]);
        let info = inspect(&path).unwrap();
        assert_eq!(info.manifest.id, "com.example.tone");
        assert_eq!(info.unpacked_size, data.len() as u64 + 7);
        assert_eq!(info.file_count, 2);
        assert_eq!(info.program, "bin/tone");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn unsafe_paths_are_rejected_before_any_install_output_is_written() {
        let dir = TestDir::new();
        let data = manifest("1");
        let path = dir.0.join("unsafe.btaddon");
        let addons = dir.0.join("addons");
        for bad in [
            "../escape",
            "/absolute",
            "C:/drive",
            "C:drive",
            "a/../escape",
            "a\\escape",
            "//server/file",
            "bin/NUL",
            "bin/tone.",
        ] {
            archive(
                &path,
                &[
                    ("addon.json", &data),
                    ("bin/tone", b"program"),
                    (bad, b"bad"),
                ],
            );
            assert!(install(&path, &addons).is_err(), "accepted {bad}");
            assert!(!addons.exists(), "wrote output for {bad}");
        }
    }

    #[test]
    fn symlinks_are_rejected() {
        let dir = TestDir::new();
        let path = dir.0.join("link.btaddon");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.add_symlink("link", "outside", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        assert!(inspect(path).unwrap_err().to_string().contains("symbolic"));
    }

    #[test]
    fn more_than_4096_entries_are_rejected() {
        let dir = TestDir::new();
        let path = dir.0.join("many.btaddon");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        for index in 0..4097 {
            zip.start_file(
                format!("file-{index}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        }
        zip.finish().unwrap();
        assert!(inspect(path).unwrap_err().to_string().contains("4096"));
    }

    #[test]
    fn oversized_declared_entry_is_rejected_without_decompression() {
        let dir = TestDir::new();
        let path = dir.0.join("large.btaddon");
        archive(&path, &[("large", b"x")]);
        let mut bytes = fs::read(&path).unwrap();
        let offset = bytes.windows(4).position(|v| v == b"PK\x01\x02").unwrap();
        bytes[offset + 24..offset + 28].copy_from_slice(&(512_u32 * 1024 * 1024 + 1).to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(inspect(path).unwrap_err().to_string().contains("512 MiB"));
    }

    #[test]
    fn missing_program_for_any_platform_is_rejected() {
        let dir = TestDir::new();
        let path = dir.0.join("missing.btaddon");
        let mut value: serde_json::Value = serde_json::from_slice(&manifest("1")).unwrap();
        value["run"]["macos-aarch64"] = json!({"program":"bin/missing"});
        archive(
            &path,
            &[
                ("addon.json", &serde_json::to_vec(&value).unwrap()),
                ("bin/tone", b"program"),
            ],
        );
        assert!(inspect(path)
            .unwrap_err()
            .to_string()
            .contains("bin/missing"));
    }

    #[test]
    fn corrupt_payload_is_rejected_and_the_previous_version_survives() {
        let dir = TestDir::new();
        let old = dir.0.join("old.btaddon");
        let broken = dir.0.join("broken.btaddon");
        let addons = dir.0.join("addons");
        archive(
            &old,
            &[("addon.json", &manifest("1")), ("bin/tone", b"old-program")],
        );
        install(old, &addons).unwrap();
        archive(
            &broken,
            &[("addon.json", &manifest("2")), ("bin/tone", b"new-program")],
        );
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&broken)
            .unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        let offset = bytes.windows(11).position(|v| v == b"new-program").unwrap();
        file.seek(SeekFrom::Start(offset as u64)).unwrap();
        file.write_all(b"bad-program").unwrap();
        drop(file);
        assert!(install(broken, &addons).is_err());
        assert_eq!(
            fs::read(addons.join("com.example.tone/bin/tone")).unwrap(),
            b"old-program"
        );
        assert_eq!(
            list_installed(&addons).unwrap()[0]
                .manifest
                .as_ref()
                .unwrap()
                .version,
            "1"
        );
    }

    #[test]
    fn failed_final_swap_restores_the_previous_folder() {
        let dir = TestDir::new();
        let target = dir.0.join("com.example.tone");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("old-program"), b"old").unwrap();
        // A staging folder can disappear before the final rename, for example after a disk or
        // external cleanup failure. Exercise both real renames used to restore the old version.
        let missing_stage = dir.0.join("missing-stage");
        assert!(replace_install(&missing_stage, &target, &dir.0, "com.example.tone").is_err());
        assert_eq!(fs::read(target.join("old-program")).unwrap(), b"old");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_install_keeps_an_existing_destination_and_cleans_the_stage() {
        let dir = TestDir::new();
        let path = dir.0.join("tone.btaddon");
        let addons = dir.0.join("addons");
        fs::create_dir(&addons).unwrap();
        let target = addons.join("com.example.tone");
        fs::write(&target, b"existing destination").unwrap();
        archive(
            &path,
            &[("addon.json", &manifest("2")), ("bin/tone", b"new")],
        );
        assert!(install(&path, &addons).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"existing destination");
        assert_eq!(fs::read_dir(&addons).unwrap().count(), 1);
    }

    #[test]
    fn an_unavailable_staged_update_restores_the_old_install() {
        let root = TestDir::new();
        let id = "com.example.restore";
        let target = root.0.join(id);
        fs::create_dir(&target).unwrap();
        fs::write(target.join("version"), "original").unwrap();
        assert!(replace_install(&root.0.join("missing-stage"), &target, &root.0, id).is_err());
        assert_eq!(
            fs::read_to_string(target.join("version")).unwrap(),
            "original"
        );
    }

    #[test]
    fn update_replaces_the_old_folder_and_remove_keeps_addon_data() {
        let dir = TestDir::new();
        let path = dir.0.join("tone.btaddon");
        let addons = dir.0.join("addons");
        let data = dir.0.join("addon-data/com.example.tone");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("setting"), b"retained").unwrap();
        archive(
            &path,
            &[
                ("addon.json", &manifest("1")),
                ("bin/tone", b"old"),
                ("obsolete", b"old"),
            ],
        );
        install(&path, &addons).unwrap();
        archive(
            &path,
            &[("addon.json", &manifest("2")), ("bin/tone", b"new")],
        );
        install(path, &addons).unwrap();
        assert!(!addons.join("com.example.tone/obsolete").exists());
        assert_eq!(
            fs::read(addons.join("com.example.tone/bin/tone")).unwrap(),
            b"new"
        );
        assert!(remove(&addons, "../escape").is_err());
        remove(&addons, "com.example.tone").unwrap();
        assert!(!addons.join("com.example.tone").exists());
        assert_eq!(fs::read(data.join("setting")).unwrap(), b"retained");
    }

    #[test]
    fn broken_installations_are_returned_instead_of_hidden() {
        let dir = TestDir::new();
        fs::create_dir(dir.0.join("broken")).unwrap();
        fs::create_dir(dir.0.join("mismatch")).unwrap();
        fs::write(dir.0.join("mismatch/addon.json"), manifest("1")).unwrap();
        let entries = list_installed(&dir.0).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.manifest.is_err()));
    }

    #[test]
    fn unreadable_entry_metadata_does_not_hide_other_addons() {
        let dir = TestDir::new();
        let good = dir.0.join("com.example.tone");
        fs::create_dir_all(good.join("bin")).unwrap();
        fs::write(good.join("addon.json"), manifest("1")).unwrap();
        fs::write(good.join("bin/tone"), b"program").unwrap();
        let broken = dir.0.join("com.example.unreadable");
        fs::create_dir(&broken).unwrap();
        let entries = list_installed_entries(&dir.0, fs::read_dir(&dir.0).unwrap(), |path| {
            if path == broken {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "cannot read this entry",
                ))
            } else {
                fs::symlink_metadata(path)
            }
        })
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .find(|entry| entry.path == good)
            .unwrap()
            .manifest
            .is_ok());
        assert!(entries
            .iter()
            .find(|entry| entry.path == broken)
            .unwrap()
            .manifest
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("cannot read this entry"));
    }

    #[test]
    fn unreadable_directory_entry_does_not_fail_the_list() {
        let dir = TestDir::new();
        fs::create_dir(dir.0.join("broken")).unwrap();
        let entries = std::iter::once(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "cannot enumerate this entry",
        )))
        .chain(fs::read_dir(&dir.0).unwrap());
        let entries =
            list_installed_entries(&dir.0, entries, |path| fs::symlink_metadata(path)).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry
            .manifest
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("cannot enumerate this entry")));
    }

    #[test]
    fn non_ascii_case_duplicates_are_rejected_during_inspection() {
        let dir = TestDir::new();
        let path = dir.0.join("collision.btaddon");
        for names in [
            ["bin/\u{00e4}", "bin/\u{00c4}"],
            ["bin/\u{0451}", "bin/\u{0401}"],
            ["b\u{00ed}n", "B\u{00cd}N/file"],
        ] {
            archive(
                &path,
                &[
                    ("addon.json", &manifest("1")),
                    ("bin/tone", b"program"),
                    (names[0], b"a"),
                    (names[1], b"b"),
                ],
            );
            assert!(inspect(&path).is_err(), "accepted {names:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn package_case_comparison_matches_windows_file_creation() {
        for names in [
            ["\u{00e4}", "\u{00c4}"],
            ["\u{03c2}", "\u{03c3}"],
            ["\u{0131}", "I"],
            ["\u{00df}", "SS"],
            ["\u{00e9}", "e\u{0301}"],
        ] {
            let dir = TestDir::new();
            File::create(dir.0.join(names[0])).unwrap();
            let collision = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.0.join(names[1]))
            {
                Ok(_) => false,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => true,
                Err(error) => panic!("could not test the filesystem: {error}"),
            };
            let path = dir.0.join("comparison.btaddon");
            archive(
                &path,
                &[
                    ("addon.json", &manifest("1")),
                    ("bin/tone", b"program"),
                    (names[0], b"a"),
                    (names[1], b"b"),
                ],
            );
            assert_eq!(
                inspect(&path).is_err(),
                collision,
                "comparison disagrees for {names:?}"
            );
        }
    }

    #[test]
    fn duplicate_or_file_directory_collisions_are_rejected() {
        let dir = TestDir::new();
        let path = dir.0.join("collision.btaddon");
        for names in [["bin/tone", "BIN/TONE"], ["bin", "bin/tone"]] {
            archive(
                &path,
                &[
                    ("addon.json", &manifest("1")),
                    (names[0], b"a"),
                    (names[1], b"b"),
                ],
            );
            assert!(inspect(&path).is_err());
        }
    }

    #[test]
    fn pack_validates_folder_and_round_trips_a_real_archive() {
        let dir = TestDir::new();
        let folder = dir.0.join("folder");
        fs::create_dir_all(folder.join("bin")).unwrap();
        fs::write(folder.join("addon.json"), manifest("1")).unwrap();
        let path = dir.0.join("tone.btaddon");
        assert!(pack(&folder, &path).is_err());
        assert!(!path.exists());
        fs::write(folder.join("bin/tone"), b"program").unwrap();
        let info = pack(&folder, &path).unwrap();
        assert_eq!(info.file_count, 2);
        assert_eq!(inspect(path).unwrap().manifest.id, "com.example.tone");
    }
}
