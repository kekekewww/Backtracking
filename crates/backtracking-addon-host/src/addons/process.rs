//! macOS counterpart of desktop/windows/src/addons/process.rs at
//! e9a4efc0d2918bccadeecf2c7e586b84ddd5a4f4.
use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Child, ChildStdout, Command, Stdio},
};

pub(super) struct AddonProcess {
    guard: Child,
    id: u32,
    status: BufReader<ChildStdout>,
    #[cfg(test)]
    pub(super) fail_termination: bool,
}
impl AddonProcess {
    pub(super) fn launch(
        program: &Path,
        args: &[String],
        cwd: &Path,
        environment: &[(OsString, OsString)],
    ) -> Result<Self, String> {
        if !program.is_absolute() || !cwd.is_absolute() {
            return Err("The add-on program and working directory must be absolute paths".into());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = exe.parent().ok_or("Missing host directory")?;
        #[cfg(test)]
        let dir = if dir.ends_with("deps") {
            dir.parent().unwrap()
        } else {
            dir
        };
        let mut guard = Command::new(dir.join("bt-addon-guard"))
            .arg(program)
            .args(args)
            .current_dir(cwd)
            .envs(environment.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Start add-on guard: {e}"))?;
        let mut line = String::new();
        let mut status = BufReader::new(guard.stdout.take().unwrap());
        let id = status
            .read_line(&mut line)
            .ok()
            .and_then(|_| line.trim().parse::<u32>().ok());
        let Some(id) = id else {
            drop(guard.stdin.take());
            let _ = guard.wait();
            return Err(format!("Start add-on: {}", line.trim()));
        };
        Ok(Self {
            guard,
            id,
            status,
            #[cfg(test)]
            fail_termination: false,
        })
    }
    pub(super) fn id(&self) -> u32 {
        self.id
    }
    pub(super) fn try_wait(&mut self) -> Result<Option<u32>, String> {
        if self.guard.try_wait().map_err(|e| e.to_string())?.is_none() {
            return Ok(None);
        }
        let mut code = String::new();
        self.status
            .read_line(&mut code)
            .map_err(|e| e.to_string())?;
        Ok(Some(code.trim().parse().unwrap_or(1)))
    }
    pub(super) fn terminate(&mut self) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_termination {
            self.fail_termination = false;
            return Err("Injected termination failure".into());
        }
        drop(self.guard.stdin.take());
        self.guard.wait().map(|_| ()).map_err(|e| e.to_string())
    }
}
impl Drop for AddonProcess {
    fn drop(&mut self) {
        drop(self.guard.stdin.take());
        let _ = self.guard.wait();
    }
}
pub(super) fn random_token() -> Result<String, String> {
    let mut token = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut token))
        .map_err(|e| e.to_string())?;
    Ok(token.iter().map(|n| format!("{n:02x}")).collect())
}
#[cfg(test)]
mod tests {
    #[test]
    fn helper_child() {
        if std::env::var_os("BACKTRACKING_PROCESS_UNIT_TEST_CHILD").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(30))
        }
    }
}
