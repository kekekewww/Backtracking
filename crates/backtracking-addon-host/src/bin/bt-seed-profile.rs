#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    backtracking_addon_host::test_home::check().expect("Isolate the test home");
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Pass a new absolute test root")?;
    backtracking_addon_host::seed::seed(std::path::Path::new(&path))
}
#[cfg(not(unix))]
fn main() {
    eprintln!("Use the Windows seed_test_profile in desktop/windows");
    std::process::exit(1)
}
