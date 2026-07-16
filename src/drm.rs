use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DrmStatus {
    Ready { library: PathBuf, version: String },
    EngineManaged,
    Missing,
    Rejected(String),
}

impl DrmStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Ready { library, version } => {
                format!("Widevine {version} ready ({})", library.display())
            }
            Self::EngineManaged => "EME/CDM managed by WebView2 runtime".into(),
            Self::Missing => "Widevine not installed; ClearKey-only host pending".into(),
            Self::Rejected(error) => format!("Widevine rejected: {error}"),
        }
    }
}

fn library_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "widevinecdm.dll"
    } else if cfg!(target_os = "macos") {
        "libwidevinecdm.dylib"
    } else {
        "libwidevinecdm.so"
    }
}

fn manifest_near(library: &Path) -> Option<PathBuf> {
    let mut directory = library.parent();
    for _ in 0..=4 {
        let candidate = directory?.join("manifest.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        directory = directory?.parent();
    }
    None
}

fn binary_matches_host(bytes: &[u8]) -> bool {
    #[cfg(target_os = "windows")]
    {
        if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
            return false;
        }
        let offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        if bytes.len() < offset + 6 || &bytes[offset..offset + 4] != b"PE\0\0" {
            return false;
        }
        let machine = u16::from_le_bytes(bytes[offset + 4..offset + 6].try_into().unwrap());
        return match std::env::consts::ARCH {
            "x86_64" => machine == 0x8664,
            "aarch64" => machine == 0xaa64,
            _ => false,
        };
    }

    #[cfg(target_os = "linux")]
    {
        if bytes.len() < 20 || &bytes[..4] != b"\x7fELF" || bytes[5] != 1 {
            return false;
        }
        let machine = u16::from_le_bytes(bytes[18..20].try_into().unwrap());
        return match std::env::consts::ARCH {
            "x86_64" => machine == 62,
            "aarch64" => machine == 183,
            _ => false,
        };
    }

    #[cfg(target_os = "macos")]
    {
        if bytes.len() < 8 {
            return false;
        }
        let magic = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        let cpu = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        return magic == 0xfeedfacf
            && match std::env::consts::ARCH {
                "x86_64" => cpu == 0x01000007,
                "aarch64" => cpu == 0x0100000c,
                _ => false,
            };
    }

    #[allow(unreachable_code)]
    false
}

fn validate(library: PathBuf) -> Result<DrmStatus, String> {
    if !library.is_file() {
        return Err("component library does not exist".into());
    }
    let bytes = fs::read(&library).map_err(|e| e.to_string())?;
    if !binary_matches_host(&bytes) {
        return Err(format!(
            "component is not a {} {} binary",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    let manifest_path = manifest_near(&library).ok_or("manifest.json is missing")?;
    let manifest: Value = serde_json::from_slice(
        &fs::read(manifest_path).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("invalid manifest: {e}"))?;
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
        .ok_or("manifest has no version")?
        .to_string();
    Ok(DrmStatus::Ready { library, version })
}

pub fn detect() -> DrmStatus {
    if let Some(explicit) = std::env::var_os("UBAR_WIDEVINE_PATH") {
        let path = PathBuf::from(explicit);
        let library = if path.is_dir() {
            path.join(library_name())
        } else {
            path
        };
        return validate(library).unwrap_or_else(DrmStatus::Rejected);
    }

    let managed = dirs::data_dir()
        .map(|path| path.join("ubar").join("cdm").join("widevine").join(library_name()));
    if let Some(library) = managed
        && library.exists()
    {
        return validate(library).unwrap_or_else(DrmStatus::Rejected);
    }

    if cfg!(target_os = "windows") {
        DrmStatus::EngineManaged
    } else {
        DrmStatus::Missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_binary_is_rejected() {
        assert!(!binary_matches_host(b"not a shared library"));
    }

    #[test]
    fn status_is_human_readable() {
        assert!(DrmStatus::Missing.label().contains("Widevine"));
    }
}
