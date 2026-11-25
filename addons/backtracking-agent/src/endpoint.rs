//! The connection file the running add-on writes into its data folder, and the token it
//! keeps there.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform;

/// The file name of the connection file in the add-on's data folder.
pub const ENDPOINT_FILE: &str = "endpoint.json";
/// The file name of the kept token.
pub const TOKEN_FILE: &str = "token";
/// Names a connection file to use instead of the one found from the program's own place.
pub const ENDPOINT_ENV: &str = "BACKTRACKING_AGENT_ENDPOINT";

/// What a client needs to reach the running add-on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// `http://127.0.0.1:<port>/mcp`.
    pub url: String,
    /// Sent as `Authorization: Bearer <token>`.
    pub token: String,
    /// The add-on's process, so that a file left behind by a run that ended is recognised.
    pub pid: u32,
    /// The add-on's program.
    pub program: PathBuf,
    pub version: String,
}

impl Endpoint {
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let mut text = serde_json::to_vec_pretty(self)?;
        text.push(b'\n');
        platform::write_private(path, &text)
    }

    pub fn read(path: &Path) -> io::Result<Self> {
        let text = std::fs::read(path)?;
        serde_json::from_slice(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Whether the process that wrote this file is still running.
    pub fn is_current(&self, file: &Path) -> bool {
        let Ok(current) = std::env::current_exe() else {
            return false;
        };
        trusted_program(&self.program, &current, file)
            && platform::process_runs(self.pid, &self.program)
    }
}

// The caller can explicitly select another installation's endpoint. Derive its
// installed program from that selected layout, never from fields in the JSON.
fn trusted_program(claim: &Path, current: &Path, endpoint: &Path) -> bool {
    let Ok(claim) = claim.canonicalize() else {
        return false;
    };
    let matches = |path: &Path| path.canonicalize().is_ok_and(|p| p == claim);
    if matches(current) {
        return true;
    }
    let Some(data) = endpoint.parent() else {
        return false;
    };
    if !data.ends_with(Path::new("addon-data").join(crate::ID)) {
        return false;
    }
    let Some(root) = data.parent().and_then(Path::parent) else {
        return false;
    };
    let bin = format!("backtracking-agent{}", std::env::consts::EXE_SUFFIX);
    matches(&root.join("addons").join(crate::ID).join("bin").join(bin))
}

/// A new random token: 32 bytes as 64 hexadecimal digits.
pub fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The kept token in `data`, made and stored the first time.
pub fn load_or_make_token(data: &Path) -> io::Result<String> {
    let path = data.join(TOKEN_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let token = text.trim();
        if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(token.to_owned());
        }
    }
    let token = new_token()?;
    platform::write_private(&path, token.as_bytes())?;
    Ok(token)
}

/// Replaces the kept token.
pub fn renew_token(data: &Path) -> io::Result<String> {
    let token = new_token()?;
    platform::write_private(&data.join(TOKEN_FILE), token.as_bytes())?;
    Ok(token)
}

/// Where the stdio forwarder looks for the connection file: `explicit` (from `--endpoint`),
/// else `BACKTRACKING_AGENT_ENDPOINT`, else the data folder that belongs to the install folder
/// this program runs from (`<data>/addons/<id>/bin/…` → `<data>/addon-data/<id>/`), else the
/// installed add-on's folder in `%LOCALAPPDATA%\Backtracking`.
pub fn locate(explicit: Option<PathBuf>, program: &Path) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    if let Some(path) = std::env::var_os(ENDPOINT_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(path) = beside_install(program) {
        return path;
    }
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Backtracking")
        .join("addon-data")
        .join(crate::ID)
        .join(ENDPOINT_FILE)
}

/// `<data>/addon-data/<id>/endpoint.json` when `program` sits in `<data>/addons/<id>/…`.
fn beside_install(program: &Path) -> Option<PathBuf> {
    let mut folder = program.parent();
    while let Some(current) = folder {
        if current.join("addon.json").is_file() {
            let id = current.file_name()?;
            let addons = current.parent()?;
            if !addons.file_name()?.eq_ignore_ascii_case("addons") {
                return None;
            }
            return Some(
                addons
                    .parent()?
                    .join("addon-data")
                    .join(id)
                    .join(ENDPOINT_FILE),
            );
        }
        folder = current.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc1_stdio_program_is_local_or_belongs_to_selected_installation() {
        let root = std::env::temp_dir().join(format!("rc1-program-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let current = root.join("current");
        let unrelated = root.join("other");
        let bin = root.join("addons").join(crate::ID).join("bin");
        let installed = bin.join(format!(
            "backtracking-agent{}",
            std::env::consts::EXE_SUFFIX
        ));
        let endpoint = root
            .join("addon-data")
            .join(crate::ID)
            .join("endpoint.json");
        std::fs::create_dir_all(bin).unwrap();
        for file in [&current, &unrelated, &installed] {
            std::fs::write(file, []).unwrap();
        }
        assert!(trusted_program(&current, &current, &endpoint));
        assert!(trusted_program(&installed, &current, &endpoint));
        assert!(!trusted_program(&unrelated, &current, &endpoint));
        assert!(!trusted_program(
            &installed,
            &current,
            &root.join("other.json")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_connection_file_is_found_beside_the_install_folder() {
        let root = std::env::temp_dir().join(format!("agent-locate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let install = root.join("Backtracking/addons").join(crate::ID);
        std::fs::create_dir_all(install.join("bin")).unwrap();
        std::fs::write(install.join("addon.json"), "{}").unwrap();
        let program = install.join("bin/backtracking-agent.exe");
        assert_eq!(
            beside_install(&program).unwrap(),
            root.join("Backtracking/addon-data")
                .join(crate::ID)
                .join(ENDPOINT_FILE)
        );
        // An explicit path wins over everything.
        assert_eq!(
            locate(Some(PathBuf::from("X:\\e.json")), &program),
            PathBuf::from("X:\\e.json")
        );
        // A program outside an install folder is not taken for one.
        assert!(beside_install(&root.join("elsewhere/agent.exe")).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_token_is_kept_until_it_is_renewed() {
        let data = std::env::temp_dir().join(format!("agent-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        let first = load_or_make_token(&data).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(load_or_make_token(&data).unwrap(), first);
        let renewed = renew_token(&data).unwrap();
        assert_ne!(renewed, first);
        assert_eq!(load_or_make_token(&data).unwrap(), renewed);
        // A damaged token file gets a new token, not an empty one.
        std::fs::write(data.join(TOKEN_FILE), "short").unwrap();
        assert_eq!(load_or_make_token(&data).unwrap().len(), 64);
        std::fs::remove_dir_all(&data).unwrap();
    }
}
