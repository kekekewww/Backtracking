#[cfg(target_os = "linux")]
use std::ffi::c_void;
use std::io;
use std::process::ExitCode;

use backtracking_core::{discover_snapshot_targets, request_sender_snapshot};

struct Route {
    target: Option<String>,
}

impl Route {
    fn save(&self) -> io::Result<()> {
        if let Some(id) = &self.target {
            return request_sender_snapshot(id);
        }

        let targets = discover_snapshot_targets();
        match targets.as_slice() {
            [target] => request_sender_snapshot(&target.id),
            [] => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no running Backtracking plug-in instance",
            )),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "multiple Backtracking plug-in instances are running; use --list and --target ID",
            )),
        }
    }
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Backtracking shortcut: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl Iterator<Item = String>) -> io::Result<()> {
    let mut arguments = arguments.peekable();
    let mut route = Route { target: None };
    let mut save = false;
    while let Some(arg) = arguments.next() {
        match arg.as_str() {
            "--target" if route.target.is_none() => {
                let target = arguments
                    .next()
                    .filter(|target| !target.is_empty() && !target.starts_with("--"))
                    .ok_or_else(|| invalid_argument("--target requires an instance ID"))?;
                route.target = Some(target);
            }
            "--save" => save = true,
            "--list" if !save && route.target.is_none() && arguments.peek().is_none() => {
                for target in discover_snapshot_targets() {
                    println!("{}\t{}", target.id, target.name);
                }
                return Ok(());
            }
            "-h" | "--help" => {
                println!(
                    "backtracking-shortcut [--target ID] [--save]\n\
                     backtracking-shortcut --list\n\
                     With no arguments, listens for the system-wide snapshot shortcut.\n\
                     Use --save for a one-shot request and --target to select one plug-in instance."
                );
                return Ok(());
            }
            _ => return Err(invalid_argument(&format!("unknown argument {arg:?}"))),
        }
    }

    if save {
        route.save()
    } else {
        run_hotkey(&route)
    }
}

fn invalid_argument(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(target_os = "linux")]
fn run_hotkey(route: &Route) -> io::Result<()> {
    extern "C" {
        fn bt_linux_hotkey_run(
            wayland: i32,
            save: extern "C" fn(*const c_void),
            data: *const c_void,
        ) -> i32;
    }

    extern "C" fn save(data: *const c_void) {
        // The native loop calls back synchronously on this thread while route is alive.
        let route = unsafe { &*data.cast::<Route>() };
        if let Err(error) = route.save() {
            eprintln!("Backtracking shortcut: {error}");
        }
    }

    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value == "wayland");
    let result = unsafe { bt_linux_hotkey_run(i32::from(wayland), save, (route as *const Route).cast()) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::other("could not keep the global shortcut active"))
    }
}

#[cfg(not(target_os = "linux"))]
fn run_hotkey(_route: &Route) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "global shortcut registration in this helper requires Linux",
    ))
}
