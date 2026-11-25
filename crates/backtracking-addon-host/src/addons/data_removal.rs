//! Remove private data relative to the primary lease's directory descriptor.
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::fs::MetadataExt;
use std::ptr::NonNull;

use backtracking_addon::manifest::validate_id;

pub(super) fn open_at(
    parent: &File,
    name: &CStr,
    flags: i32,
    mode: libc::c_uint,
) -> io::Result<File> {
    // The caller supplies a single component. Refuse links at every lookup.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn child_dir(parent: &File, name: &CStr) -> io::Result<File> {
    let dir = open_at(parent, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    if dir.metadata()?.dev() != parent.metadata()?.dev() {
        return Err(io::Error::other(
            "Add-on data crosses a filesystem boundary",
        ));
    }
    Ok(dir)
}

fn still_named(parent: &File, name: &CStr, dir: &File) -> io::Result<()> {
    let mut stat = MaybeUninit::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let named = unsafe { stat.assume_init() };
    let held = dir.metadata()?;
    if named.st_dev as u64 != held.dev() || named.st_ino as u64 != held.ino() {
        return Err(io::Error::other(
            "Add-on data directory changed during removal",
        ));
    }
    Ok(())
}

struct DataDirectory {
    parent: File,
    target: File,
    name: CString,
}

impl DataDirectory {
    fn open(root: &File, id: &str) -> io::Result<Option<Self>> {
        validate_id(id).map_err(io::Error::other)?;
        let name = CString::new(id)?;
        // Missing directories are an idempotent success. Symlinks, non-directories
        // and lookup failures are errors, including when the final child is absent.
        let found: io::Result<Self> = (|| {
            let parent = child_dir(root, c"addon-data")?;
            let target = child_dir(&parent, &name)?;
            Ok(Self {
                parent,
                target,
                name,
            })
        })();
        match found {
            Ok(d) => Ok(Some(d)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn check_binding(&self, root: &File) -> io::Result<()> {
        still_named(root, c"addon-data", &self.parent)?;
        still_named(&self.parent, &self.name, &self.target)
    }
}

pub(super) fn remove(root: &File, id: &str) -> Result<(), String> {
    let Some(data) = DataDirectory::open(root, id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    remove_open_dir(root, &data).map_err(|e| e.to_string())
}

fn unlink(parent: &File, name: &CStr, flags: i32) -> io::Result<()> {
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Entries(NonNull<libc::DIR>);

impl Entries {
    fn open(dir: &File) -> io::Result<Self> {
        let copy = dir.try_clone()?;
        let ptr = NonNull::new(unsafe { libc::fdopendir(copy.as_raw_fd()) })
            .ok_or_else(io::Error::last_os_error)?;
        let _ = copy.into_raw_fd(); // closed by closedir
        Ok(Self(ptr))
    }
}

impl Drop for Entries {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0.as_ptr());
        }
    }
}

// Keep the traversal stack off the worker thread's call stack. Each frame owns
// the descriptor for the directory whose entries it reads and unlinks.
struct OpenDirectory {
    name: CString,
    dir: File,
    entries: Entries,
}

impl OpenDirectory {
    fn new(name: CString, dir: File) -> io::Result<Self> {
        let entries = Entries::open(&dir)?;
        Ok(Self { name, dir, entries })
    }
}

fn remove_open_dir(root: &File, data: &DataDirectory) -> io::Result<()> {
    let mut stack = vec![OpenDirectory::new(
        data.name.clone(),
        data.target.try_clone()?,
    )?];
    while let Some(current) = stack.last() {
        data.check_binding(root)?;
        for pair in stack.windows(2) {
            still_named(&pair[0].dir, &pair[1].name, &pair[1].dir)?;
        }
        let entry = unsafe { libc::readdir(current.entries.0.as_ptr()) };
        if entry.is_null() {
            // rmdir also verifies no unvisited entries remain.
            let done = stack.pop().expect("current directory");
            let owner = stack.last().map_or(&data.parent, |frame| &frame.dir);
            still_named(owner, &done.name, &done.dir)?;
            unlink(owner, &done.name, libc::AT_REMOVEDIR)?;
            continue;
        }
        let child = unsafe { CStr::from_ptr((&raw const (*entry).d_name).cast()) };
        if child == c"." || child == c".." {
            continue;
        }
        // With zero flags unlinkat removes a leaf, never a directory or a link target.
        match unlink(&current.dir, child, 0) {
            Ok(()) => continue,
            Err(e) if matches!(e.raw_os_error(), Some(libc::EISDIR | libc::EPERM)) => {}
            Err(e) => return Err(e),
        }
        let opened = child_dir(&current.dir, child)?;
        stack.push(OpenDirectory::new(child.to_owned(), opened)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::TestDir;
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn remove_v3_authorization_refuses_parent_or_leaf_links_and_changed_bindings() {
        let fixture = TestDir::new();
        let root = File::open(&fixture.0).unwrap();
        let id = "com.example.test";
        let parent = fixture.0.join("addon-data");
        let target = parent.join(id);
        let spare = fixture.0.join("spare");
        fs::create_dir(&spare).unwrap();
        symlink(&spare, &parent).unwrap();
        assert!(DataDirectory::open(&root, id).is_err());
        fs::remove_file(&parent).unwrap();
        fs::create_dir(&parent).unwrap();
        symlink(&spare, &target).unwrap();
        assert!(DataDirectory::open(&root, id).is_err());
        assert!(DataDirectory::open(&root, "../spare").is_err());
        fs::remove_file(&target).unwrap();
        fs::create_dir(&target).unwrap();
        let bound = DataDirectory::open(&root, id).unwrap().unwrap();
        bound.check_binding(&root).unwrap();
        fs::rename(&target, parent.join("original")).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(bound.check_binding(&root).is_err());
        let bound = DataDirectory::open(&root, id).unwrap().unwrap();
        fs::rename(&parent, fixture.0.join("original-parent")).unwrap();
        fs::create_dir_all(&target).unwrap();
        assert!(bound.check_binding(&root).is_err());
        // Binding checks above never invoke the deletion backend.
        assert!(target.is_dir() && spare.is_dir());
    }

    #[test]
    fn remove_v3_private_data_normal_nested_and_missing() {
        let fixture = TestDir::new();
        let root = File::open(&fixture.0).unwrap();
        let id = "com.example.test";
        let parent = fixture.0.join("addon-data");
        let target = parent.join(id);
        remove(&root, id).unwrap();
        fs::create_dir_all(target.join("nested")).unwrap();
        fs::write(target.join("nested/file"), "synthetic").unwrap();
        let sibling = parent.join("com.example.other");
        fs::create_dir(&sibling).unwrap();
        fs::write(sibling.join("keep"), "private").unwrap();
        remove(&root, id).unwrap();
        assert!(!target.exists());
        remove(&root, id).unwrap();
        assert_eq!(fs::read_to_string(sibling.join("keep")).unwrap(), "private");
    }
}
