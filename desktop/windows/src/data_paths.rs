use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Every app-owned folder starts at this root. Tests supply their own root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    pub(crate) fn new(root: PathBuf) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Backtracking data directory must be absolute",
            ));
        }
        Ok(Self { root })
    }

    /// Resolve the user's data root once. Do not use this constructor in tests.
    pub(crate) fn for_current_user() -> io::Result<Self> {
        static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
        let root = ROOT.get_or_init(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join("Backtracking"))
        });
        root.clone().map(Self::new).unwrap_or_else(|| {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Windows local app data directory unavailable",
            ))
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn settings(&self) -> PathBuf {
        self.root.join("Settings")
    }

    pub(crate) fn addons(&self) -> PathBuf {
        self.root.join("addons")
    }

    pub(crate) fn addon_data(&self) -> PathBuf {
        self.root.join("addon-data")
    }

    pub(crate) fn senders(&self) -> PathBuf {
        self.root.join("senders-v1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_root_owns_every_app_folder() {
        let root = std::env::temp_dir().join("Backtracking-data-path-test");
        let paths = DataPaths::new(root.clone()).unwrap();
        assert_eq!(paths.root(), root);
        assert_eq!(paths.settings(), root.join("Settings"));
        assert_eq!(paths.addons(), root.join("addons"));
        assert_eq!(paths.addon_data(), root.join("addon-data"));
        assert_eq!(paths.senders(), root.join("senders-v1"));
    }

    #[test]
    fn relative_data_root_is_rejected() {
        assert_eq!(
            DataPaths::new(PathBuf::from("relative"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
