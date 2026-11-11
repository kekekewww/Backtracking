use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ICON: &str = "../../assets/backtracking.ico";
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_MANIFEST: u16 = 24;
const APPLICATION_MANIFEST: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity name="Backtracking.Desktop" version="1.0.0.0" type="win32"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-env-changed=BACKTRACKING_UPDATE_PUBLIC_KEY");
    if let Ok(key) = env::var("BACKTRACKING_UPDATE_PUBLIC_KEY") {
        assert!(
            key.len() == 64
                && key
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
            "BACKTRACKING_UPDATE_PUBLIC_KEY must be a 32-byte lowercase hexadecimal Ed25519 public key"
        );
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let resource = out.join("backtracking.res");
    let icon = fs::read(ICON).expect("read the Backtracking ICO asset");
    fs::write(&resource, icon_resource(&icon)).expect("write the Backtracking icon resource");

    // MSVC accepts .res directly. MinGW's linker needs a COFF object.
    let object = if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        let object = out.join("backtracking-icon.o");
        let tool = windres();
        let status = Command::new(&tool)
            .args(["-J", "res", "-O", "coff", "-i"])
            .arg(&resource)
            .arg("-o")
            .arg(&object)
            .status()
            .unwrap_or_else(|error| panic!("could not launch {tool:?}: {error}"));
        assert!(status.success(), "{tool:?} could not compile the icon");
        object
    } else {
        println!("cargo:rustc-link-arg-bin=Backtracking=/MANIFEST:NO");
        resource
    };
    println!("cargo:rustc-link-arg-bin=Backtracking={}", object.display());
}

fn windres() -> OsString {
    println!("cargo:rerun-if-env-changed=WINDRES");
    let target = env::var("TARGET").unwrap();
    let linker_key = format!(
        "CARGO_TARGET_{}_LINKER",
        target.replace('-', "_").to_uppercase()
    );
    println!("cargo:rerun-if-env-changed={linker_key}");
    if let Some(tool) = env::var_os("WINDRES") {
        return tool;
    }
    if let Some(linker) = env::var_os(&linker_key).or_else(|| env::var_os("RUSTC_LINKER")) {
        let linker = Path::new(&linker);
        if let Some((prefix, _)) = linker
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.rsplit_once('-'))
        {
            let tool =
                linker.with_file_name(format!("{prefix}-windres{}", env::consts::EXE_SUFFIX));
            if tool.is_file() {
                return tool.into_os_string();
            }
        }
    }
    if env::var("HOST").as_deref() == Ok(&target) {
        return "windres".into();
    }
    let arch = target.split('-').next().unwrap();
    format!("{arch}-w64-mingw32-windres").into()
}

fn icon_resource(icon: &[u8]) -> Vec<u8> {
    let mut resource = Vec::new();
    // A Win32 .res file starts with an empty record.
    append_resource(&mut resource, 0, 0, 0, &[]);
    assert_eq!(&icon[..4], &[0, 0, 1, 0], "expected a Windows ICO file");
    let count = u16::from_le_bytes(icon[4..6].try_into().unwrap());
    let mut group = icon[..6].to_vec();
    // ICO entries store an image offset. The group resource replaces it with an RT_ICON ID.
    let (entries, _) = icon[6..6 + usize::from(count) * 16].as_chunks::<16>();
    for (index, entry) in entries.iter().enumerate() {
        let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let id = (index + 1) as u16;
        append_resource(
            &mut resource,
            RT_ICON,
            id,
            0x0409,
            &icon[offset..offset + length],
        );
        group.extend_from_slice(&entry[..12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    append_resource(&mut resource, RT_GROUP_ICON, 1, 0x0409, &group);
    append_resource(&mut resource, RT_MANIFEST, 1, 0, APPLICATION_MANIFEST);
    resource
}

fn append_resource(out: &mut Vec<u8>, kind: u16, id: u16, language: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    for word in [0xffff, kind, 0xffff, id, 0, 0, 0, language, 0, 0, 0, 0] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(4), 0);
}
