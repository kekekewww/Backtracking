//! Parsing and validation for `addon.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The capabilities the host can offer an add-on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Source,
    OpenFiles,
    Settings,
    Actions,
    Clips,
    Appearance,
    /// Ask Backtracking to capture, select, save and change sources, once the user allows it.
    Control,
}

/// File types accepted by an add-on with the `open-files` capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenFiles {
    pub description: String,
    pub extensions: Vec<String>,
}

/// A program path and its arguments for one platform.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunEntry {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// An add-on manifest. Call [`Manifest::validate`] after constructing it directly.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Manifest {
    pub contract: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    pub capabilities: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_files: Option<OpenFiles>,
    pub run: BTreeMap<String, RunEntry>,
}

impl Manifest {
    /// Parse JSON, check the manifest rules, and require a program for this platform.
    pub fn parse(input: &str) -> Result<Self> {
        let object: serde_json::Value = serde_json::from_str(input)
            .map_err(|error| Error::new(format!("Cannot read addon.json: {error}")))?;
        if object
            .get("open_files")
            .is_some_and(serde_json::Value::is_null)
        {
            return Err(Error::new(
                "addon.json open_files must be an object when open-files is declared, and otherwise must be omitted",
            ));
        }
        if let Some(capabilities) = object
            .get("capabilities")
            .and_then(serde_json::Value::as_array)
        {
            for capability in capabilities {
                if let Some(name) = capability
                    .as_str()
                    .filter(|_| Capability::deserialize(capability).is_err())
                {
                    return Err(Error::new(format!(
                        "Unknown add-on capability {name:?}. A newer Backtracking is needed."
                    )));
                }
            }
        }
        let manifest: Self = serde_json::from_str(input)
            .map_err(|error| Error::new(format!("Cannot read addon.json: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Check the manifest rules, including support for the current platform.
    /// Program existence is checked by the package module.
    pub fn validate(&self) -> Result<()> {
        if self.contract != 1 {
            return Err(Error::new(format!(
                "addon.json contract {} is not supported; use contract 1",
                self.contract
            )));
        }
        validate_id(&self.id)?;
        validate_text("name", &self.name, 80)?;
        validate_text("version", &self.version, 32)?;
        validate_text("author", &self.author, 80)?;
        validate_text("description", &self.description, 400)?;
        for (field, value) in [("name", &self.name), ("version", &self.version)] {
            if value.trim().is_empty() {
                return Err(Error::new(format!("addon.json {field} must not be empty")));
            }
        }
        if self
            .homepage
            .as_ref()
            .is_some_and(|homepage| !homepage.starts_with("https://"))
        {
            return Err(Error::new("addon.json homepage must start with https://"));
        }
        match (
            self.capabilities.contains(&Capability::OpenFiles),
            &self.open_files,
        ) {
            (true, None) => {
                return Err(Error::new(
                    "addon.json must include open_files when the open-files capability is declared",
                ));
            }
            (false, Some(_)) => {
                return Err(Error::new(
                    "addon.json must omit open_files unless the open-files capability is declared",
                ));
            }
            (_, Some(open_files)) => {
                if !(1..=32).contains(&open_files.extensions.len()) {
                    return Err(Error::new(
                        "addon.json open_files.extensions must have 1 to 32 entries",
                    ));
                }
                for extension in &open_files.extensions {
                    if !(1..=12).contains(&extension.len())
                        || !extension
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                    {
                        return Err(Error::new(format!(
                            "addon.json extension {extension:?} must contain 1 to 12 lower-case letters or digits, without a dot"
                        )));
                    }
                }
            }
            _ => {}
        }
        for (platform, entry) in &self.run {
            if !PLATFORMS.contains(&platform.as_str()) {
                return Err(Error::new(format!(
                    "addon.json run key {platform:?} is not supported; use one of {}",
                    PLATFORMS.join(", ")
                )));
            }
            validate_relative_path(&entry.program).map_err(|error| {
                Error::new(format!("addon.json run.{platform}.program: {error}"))
            })?;
        }
        self.current_run()?;
        Ok(())
    }

    /// Return the program for a platform key such as `windows-x86_64`.
    pub fn run_for(&self, platform: &str) -> Result<&RunEntry> {
        self.run.get(platform).ok_or_else(|| {
            Error::new(format!(
                "This add-on has no run entry for {platform}; install a package that supports this platform"
            ))
        })
    }

    /// Return the program that should run on this computer.
    pub fn current_run(&self) -> Result<&RunEntry> {
        self.run_for(current_platform()?)
    }
}

const PLATFORMS: [&str; 5] = [
    "windows-x86_64",
    "windows-aarch64",
    "macos-aarch64",
    "macos-x86_64",
    "linux-x86_64",
];

/// Return this target's contract platform key, or explain why it is unsupported.
pub fn current_platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("windows-x86_64"),
        ("windows", "aarch64") => Ok("windows-aarch64"),
        ("macos", "aarch64") => Ok("macos-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("linux", "x86_64") => Ok("linux-x86_64"),
        (os, arch) => Err(Error::new(format!(
            "Add-ons do not support platform {os}-{arch} in contract 1"
        ))),
    }
}

/// Check an add-on id before using it as a folder name.
pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
    {
        return Err(Error::new(
            "addon.json id must have 1 to 64 lower-case letters, digits, dots or hyphens, and start and end with a letter or digit",
        ));
    }
    if is_windows_device_name(id) {
        return Err(Error::new(format!(
            "addon.json id {id:?} is a reserved Windows device name; choose a different id"
        )));
    }
    Ok(())
}

/// Check a portable path inside a package, without following any filesystem links.
/// Directory callers should remove their final `/` before calling this function.
pub fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty() || path.starts_with('/') || path.contains(['\\', ':']) {
        return Err(Error::new(format!(
            "Path {path:?} must be a relative package path with / separators and no drive or root"
        )));
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(Error::new(format!(
                "Path {path:?} must not contain empty, . or .. components"
            )));
        }
        if component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|character| character.is_control() || "<>\"|?*".contains(character))
            || is_windows_device_name(component)
        {
            return Err(Error::new(format!(
                "Path {path:?} contains a name Windows cannot safely use; rename that entry"
            )));
        }
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, maximum: usize) -> Result<()> {
    if value.chars().any(char::is_control) {
        return Err(Error::new(format!(
            "addon.json {field} must be plain text without control characters"
        )));
    }
    if value.chars().count() > maximum {
        return Err(Error::new(format!(
            "addon.json {field} must have at most {maximum} characters"
        )));
    }
    Ok(())
}

fn is_windows_device_name(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or(value)
        .trim_end_matches(' ');
    let stem = stem.to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || (stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && b"123456789".contains(&stem.as_bytes()[3]))
        || matches!(
            stem.as_str(),
            "COM\u{00b9}"
                | "COM\u{00b2}"
                | "COM\u{00b3}"
                | "LPT\u{00b9}"
                | "LPT\u{00b2}"
                | "LPT\u{00b3}"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn valid() -> Value {
        let mut run = serde_json::Map::new();
        run.insert(
            current_platform().unwrap().to_owned(),
            json!({"program":"bin/tone", "args":["--quiet"]}),
        );
        json!({
            "contract":1,"id":"com.example.test-tone","name":"Test tone",
            "version":"0.1.0","author":"Example","description":"A tone.",
            "capabilities":["source","settings","actions"],"run":run
        })
    }

    fn parse(value: &Value) -> Result<Manifest> {
        Manifest::parse(&value.to_string())
    }

    #[test]
    fn valid_manifests_support_all_capabilities_optional_fields_and_default_args() {
        let mut value = valid();
        assert!(parse(&value).is_ok());
        value["homepage"] = json!("https://example.com");
        value["license"] = json!("MIT");
        value["capabilities"] = json!([
            "source",
            "open-files",
            "settings",
            "actions",
            "clips",
            "appearance",
            "control"
        ]);
        value["open_files"] = json!({"description":"Audio files","extensions":["wav","flac1"]});
        value["run"][current_platform().unwrap()] = json!({"program":"bin/tone"});
        let manifest = parse(&value).unwrap();
        assert!(manifest.current_run().unwrap().args.is_empty());
        assert_eq!(manifest.current_run().unwrap().program, "bin/tone");
        assert_eq!(manifest.capabilities.len(), 7);
        assert!(manifest.capabilities.contains(&Capability::Appearance));
        assert!(manifest.capabilities.contains(&Capability::Control));
        value["name"] = json!("音".repeat(80));
        value["author"] = json!("a".repeat(80));
        value["version"] = json!("v".repeat(32));
        value["description"] = json!("d".repeat(400));
        value["id"] = json!("a".repeat(64));
        assert!(parse(&value).is_ok());
    }

    #[test]
    fn every_required_field_is_required() {
        for field in [
            "contract",
            "id",
            "name",
            "version",
            "author",
            "description",
            "capabilities",
            "run",
        ] {
            let mut value = valid();
            value.as_object_mut().unwrap().remove(field);
            assert!(parse(&value).is_err(), "missing {field}");
        }
        let mut value = valid();
        value["run"][current_platform().unwrap()] = json!({});
        assert!(parse(&value).is_err(), "missing program");
        value["run"][current_platform().unwrap()] = json!({"program":"bin/tone", "args":[1]});
        assert!(parse(&value).is_err(), "non-string argument");
    }

    #[test]
    fn invalid_manifests_are_rejected_one_rule_at_a_time() {
        let cases: Vec<(&str, Value)> = vec![
            ("contract", json!(0)),
            ("contract", json!(2)),
            ("id", json!("")),
            ("id", json!("a".repeat(65))),
            ("id", json!("Upper")),
            ("id", json!("a_b")),
            ("id", json!(".abc")),
            ("id", json!("abc-")),
            ("id", json!("é")),
            ("name", json!("n".repeat(81))),
            ("author", json!("a".repeat(81))),
            ("version", json!("v".repeat(33))),
            ("description", json!("d".repeat(401))),
            ("name", json!("a\nb")),
            ("author", json!("a\rb")),
            ("version", json!("a\tb")),
            ("description", json!("a\u{7f}b")),
            ("homepage", json!("http://example.com")),
            ("capabilities", json!(["unknown"])),
            ("capabilities", json!("source")),
            ("run", json!({})),
            (
                "run",
                json!({"windows-x86_64":{"program":"bin/tone"},"unknown":{"program":"bin/tone"}}),
            ),
        ];
        for (field, replacement) in cases {
            let mut value = valid();
            value[field] = replacement;
            assert!(parse(&value).is_err(), "accepted {field}: {}", value[field]);
        }
    }

    #[test]
    fn unknown_capability_names_the_requirement_for_a_newer_backtracking() {
        let mut value = valid();
        value["capabilities"] = json!(["appearance", "future-feature"]);
        let error = parse(&value).unwrap_err().to_string();
        assert!(
            error.contains("future-feature") && error.contains("newer Backtracking is needed"),
            "{error}"
        );
    }

    #[test]
    fn open_files_requires_matching_capability_and_valid_extension_count_and_spelling() {
        let mut value = valid();
        value["open_files"] = json!({"description":"Audio","extensions":["wav"]});
        assert!(parse(&value).is_err());
        value.as_object_mut().unwrap().remove("open_files");
        value["capabilities"] = json!(["open-files"]);
        assert!(parse(&value).is_err());
        value["open_files"] = Value::Null;
        assert!(parse(&value).is_err());
        for extensions in [
            json!([]),
            json!(vec!["wav"; 33]),
            json!([""]),
            json!(["abcdefghijklmnop"]),
            json!(["WAV"]),
            json!([".wav"]),
            json!(["a-b"]),
            json!(["音"]),
        ] {
            value["open_files"] = json!({"description":"Audio","extensions":extensions});
            assert!(parse(&value).is_err(), "accepted {}", value["open_files"]);
        }
        value["open_files"] = json!({"description":"Audio","extensions":vec!["abcdefghijkl";32]});
        assert!(parse(&value).is_ok());
    }

    #[test]
    fn programs_must_be_safe_relative_paths() {
        for program in [
            "",
            "/tone",
            "../tone",
            "bin/../tone",
            "C:/tone",
            "C:tone",
            "\\\\server\\tone",
            "bin\\tone",
            "./tone",
            "bin//tone",
            "bin/CON.exe",
            "bin/tone.",
            "bin/tone ",
            "bin/a:b",
            "bin/a\0b",
            "bin/CONIN$",
            "bin/CONOUT$",
            "bin/NUL .txt",
            "bin/COM\u{00b9}.exe",
            "bin/COM\u{00b2}.exe",
            "bin/COM\u{00b3}.exe",
            "bin/LPT\u{00b9}.exe",
            "bin/LPT\u{00b2}.exe",
            "bin/LPT\u{00b3}.exe",
        ] {
            let mut value = valid();
            value["run"][current_platform().unwrap()]["program"] = json!(program);
            assert!(parse(&value).is_err(), "accepted {program:?}");
        }
        assert!(validate_relative_path("bin/some program.exe").is_ok());
    }

    #[test]
    fn missing_current_platform_error_names_the_platform() {
        let mut manifest = parse(&valid()).unwrap();
        manifest.run.clear();
        let error = manifest.validate().unwrap_err().to_string();
        assert!(error.contains(current_platform().unwrap()));
    }
}
