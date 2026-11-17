use backtracking_addon::package;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> backtracking_addon::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let usage = || backtracking_addon::Error::new("Usage: btaddon-pack <folder> <out.btaddon>");
    let folder = PathBuf::from(args.next().ok_or_else(usage)?);
    let output = PathBuf::from(args.next().ok_or_else(usage)?);
    if args.next().is_some() {
        return Err(usage());
    }
    let info = package::pack(folder, &output)?;
    println!(
        "Wrote {} ({} files, {} unpacked bytes; runs {}).",
        output.display(),
        info.file_count,
        info.unpacked_size,
        info.program
    );
    Ok(())
}
