use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const PLATFORM: &str = "windows-x64";
pub(crate) const ARTIFACT: &str = "Backtracking-update-windows-x64.exe";
pub(crate) const MANIFEST: &str = "Backtracking-update-windows-x64.json";
pub(crate) const SIGNATURE: &str = "Backtracking-update-windows-x64.json.sig";
pub(crate) const MAX_MANIFEST: usize = 8 * 1024;
pub(crate) const MAX_ARTIFACT: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    #[default]
    Ask,
    Automatic,
    Manual,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct Policy {
    pub(crate) mode: Mode,
    pub(crate) skipped: Option<String>,
}

impl Policy {
    pub(crate) fn allows_prompt(&self, version: &Version) -> bool {
        self.mode == Mode::Ask
            && self
                .skipped
                .as_deref()
                .and_then(|value| Version::parse(value).ok())
                .is_none_or(|skipped| &skipped < version)
    }
}

#[derive(Deserialize)]
pub(crate) struct Release {
    pub(crate) tag_name: String,
    pub(crate) body: Option<String>,
    pub(crate) draft: bool,
    pub(crate) prerelease: bool,
    pub(crate) assets: Vec<Asset>,
}

#[derive(Deserialize)]
pub(crate) struct Asset {
    name: String,
}

impl Release {
    pub(crate) fn update_version(&self, current: &Version) -> Option<Version> {
        if self.draft || self.prerelease {
            return None;
        }
        let version = stable_version(self.tag_name.strip_prefix('v')?)?;
        (version > *current).then_some(version)
    }

    pub(crate) fn has_update_assets(&self) -> bool {
        [ARTIFACT, MANIFEST, SIGNATURE]
            .into_iter()
            .all(|expected| self.assets.iter().any(|asset| asset.name == expected))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) version: String,
    platform: String,
    artifact: String,
    pub(crate) size: usize,
    sha256: String,
}

impl Manifest {
    pub(crate) fn verify(
        key: &VerifyingKey,
        json: &[u8],
        signature: &[u8],
        expected_version: &Version,
    ) -> Result<Self, String> {
        if json.len() > MAX_MANIFEST {
            return Err("Update manifest is too large.".into());
        }
        let signature = Signature::from_slice(signature)
            .map_err(|_| "The release has an invalid update signature.")?;
        key.verify_strict(json, &signature)
            .map_err(|_| "The release signature does not match the trusted key.")?;
        let manifest: Self =
            serde_json::from_slice(json).map_err(|_| "The update manifest is invalid.")?;
        if stable_version(&manifest.version).as_ref() != Some(expected_version)
            || manifest.platform != PLATFORM
            || manifest.artifact != ARTIFACT
            || manifest.size == 0
            || manifest.size > MAX_ARTIFACT
            || hex_array::<32>(&manifest.sha256).is_none()
        {
            return Err("The manifest does not match the requested Windows update.".into());
        }
        Ok(manifest)
    }

    pub(crate) fn verify_artifact(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() != self.size {
            return Err("The update download is incomplete.".into());
        }
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        if Some(digest) != hex_array::<32>(&self.sha256) {
            return Err("The update download failed its SHA-256 integrity check.".into());
        }
        if !is_x64_executable(bytes) {
            return Err("The update is not a Windows x64 executable.".into());
        }
        Ok(())
    }
}

pub(crate) fn stable_version(value: &str) -> Option<Version> {
    let version = Version::parse(value).ok()?;
    (version.pre.is_empty() && version.build.is_empty() && version.to_string() == value)
        .then_some(version)
}

pub(crate) fn hex_array<const N: usize>(value: &str) -> Option<[u8; N]> {
    let mut output = [0; N];
    if value.len() != N * 2 {
        return None;
    }
    let nibble = |byte| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    for (pair, byte) in value.as_bytes().as_chunks::<2>().0.iter().zip(&mut output) {
        *byte = nibble(pair[0])? << 4 | nibble(pair[1])?;
    }
    Some(output)
}

fn is_x64_executable(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"MZ") {
        return false;
    }
    let Some(offset) = bytes
        .get(0x3c..0x40)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .map(|value| value as usize)
    else {
        return false;
    };
    offset.checked_add(6).and_then(|end| bytes.get(offset..end)) == Some(b"PE\0\0\x64\x86")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn only_newer_canonical_stable_tags_are_offered() {
        let current = Version::new(0, 1, 5);
        let mut release = Release {
            tag_name: "Save-Fix".into(),
            body: Some("release notes".into()),
            draft: false,
            prerelease: false,
            assets: [ARTIFACT, MANIFEST, SIGNATURE]
                .map(|name| Asset { name: name.into() })
                .into_iter()
                .collect(),
        };
        assert!(release.has_update_assets());
        assert_eq!(release.body.as_deref(), Some("release notes"));
        for tag in ["Save-Fix", "v0.1.5", "v0.1.4", "v0.2.0-rc.1", "0.2.0"] {
            release.tag_name = tag.into();
            assert_eq!(release.update_version(&current), None, "{tag}");
        }
        release.tag_name = "v0.2.0".into();
        assert_eq!(
            release.update_version(&current),
            Some(Version::new(0, 2, 0))
        );
        release.prerelease = true;
        assert_eq!(release.update_version(&current), None);
    }

    #[test]
    fn skipping_once_only_suppresses_that_version_or_older() {
        let mut policy = Policy {
            mode: Mode::Ask,
            skipped: Some("0.2.0".into()),
        };
        assert!(!policy.allows_prompt(&Version::new(0, 2, 0)));
        assert!(policy.allows_prompt(&Version::new(0, 2, 1)));
        policy.mode = Mode::Manual;
        assert!(!policy.allows_prompt(&Version::new(0, 2, 1)));
    }

    #[test]
    fn signed_update_rejects_tampering_wrong_keys_and_wrong_artifacts() {
        let signing = SigningKey::from_bytes(&[0x5a; 32]);
        let wrong_key = SigningKey::from_bytes(&[0xa5; 32]).verifying_key();
        let mut exe = vec![0u8; 128];
        exe[..2].copy_from_slice(b"MZ");
        exe[0x3c] = 0x40;
        exe[0x40..0x46].copy_from_slice(b"PE\0\0\x64\x86");
        let hash = format!("{:x}", Sha256::digest(&exe));
        let json = serde_json::to_vec(&serde_json::json!({
            "version": "0.2.0",
            "platform": PLATFORM,
            "artifact": ARTIFACT,
            "size": exe.len(),
            "sha256": hash,
        }))
        .unwrap();
        let sig = signing.sign(&json).to_bytes();
        let key = signing.verifying_key();
        let version = Version::new(0, 2, 0);
        let manifest = Manifest::verify(&key, &json, &sig, &version).unwrap();
        assert!(manifest.verify_artifact(&exe).is_ok());
        exe[127] ^= 1;
        assert!(manifest.verify_artifact(&exe).is_err());
        assert!(Manifest::verify(&wrong_key, &json, &sig, &version).is_err());
        assert!(Manifest::verify(&key, &json, &sig[..63], &version).is_err());
        assert!(Manifest::verify(&key, &json, &sig, &Version::new(0, 3, 0)).is_err());
        let mut tampered = json.clone();
        tampered.push(b' ');
        assert!(Manifest::verify(&key, &tampered, &sig, &version).is_err());
    }
}
