use crate::package::PackageKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use url::Url;

const GESTURE_LIFETIME_MS: u64 = 5_000;
const MAX_LIVE_GESTURES: usize = 32;
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtensionStore {
    MozillaAddons,
    ChromeWebStore,
    EdgeAddons,
    OperaAddons,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UaVersionPolicy {
    CurrentStable,
    FixedMajor(u16),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAgentMask {
    pub product: String,
    pub version_policy: UaVersionPolicy,
    pub template: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreCompatibility {
    pub store: ExtensionStore,
    pub canonical_origin: String,
    pub user_agent: UserAgentMask,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GestureGrant {
    pub token: String,
    pub expires_at_ms: u64,
    pub store: ExtensionStore,
    pub initiator_origin: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreDownloadCandidate {
    pub initiator_url: String,
    pub download_url: String,
    pub mime_type: Option<String>,
    pub suggested_filename: Option<String>,
    pub content_length: Option<u64>,
    pub gesture_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallIntent {
    pub store: ExtensionStore,
    pub initiator_origin: String,
    pub download_url: String,
    pub package_kind: PackageKind,
    pub suggested_filename: Option<String>,
    pub gesture_consumed_at_ms: u64,
}

/// Gesture grants must only be minted from a native trusted-pointer/keyboard event.
/// Keep tokens native-side; never inject them or `begin_user_gesture` into page script.
#[derive(Default)]
pub struct StoreInstallBridge {
    gestures: BTreeMap<String, PendingGesture>,
}

struct PendingGesture {
    expires_at_ms: u64,
    store: ExtensionStore,
    initiator_origin: String,
}

impl StoreInstallBridge {
    pub fn begin_user_gesture(
        &mut self,
        initiator_url: &str,
        now_ms: u64,
    ) -> Result<GestureGrant, String> {
        let compatibility = classify_store_url(initiator_url)
            .ok_or("extension-install gesture did not occur on an official store")?;
        self.gestures.retain(|_, gesture| gesture.expires_at_ms >= now_ms);
        if self.gestures.len() >= MAX_LIVE_GESTURES {
            return Err("too many pending extension-install gestures".into());
        }
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|error| error.to_string())?;
        let token = hex(&random);
        let expires_at_ms = now_ms.checked_add(GESTURE_LIFETIME_MS)
            .ok_or("gesture timestamp overflow")?;
        self.gestures.insert(token.clone(), PendingGesture {
            expires_at_ms,
            store: compatibility.store,
            initiator_origin: compatibility.canonical_origin.clone(),
        });
        Ok(GestureGrant {
            token,
            expires_at_ms,
            store: compatibility.store,
            initiator_origin: compatibility.canonical_origin,
        })
    }

    pub fn create_install_intent(
        &mut self,
        candidate: StoreDownloadCandidate,
        now_ms: u64,
    ) -> Result<InstallIntent, String> {
        let gesture = self.gestures.remove(&candidate.gesture_token)
            .ok_or("extension install requires a trusted user gesture")?;
        if now_ms > gesture.expires_at_ms { return Err("extension-install gesture expired".into()); }
        if candidate.content_length.is_some_and(|length| length == 0 || length > MAX_PACKAGE_BYTES) {
            return Err("extension package size is invalid".into());
        }

        let compatibility = classify_store_url(&candidate.initiator_url)
            .ok_or("extension install initiator is not an official store")?;
        if compatibility.store != gesture.store
            || compatibility.canonical_origin != gesture.initiator_origin {
            return Err("extension-install gesture belongs to another store origin".into());
        }
        let download = parse_trusted_https(&candidate.download_url)
            .ok_or("extension download URL is not trusted HTTPS")?;
        let kind = classify_download(
            compatibility.store,
            &download,
            candidate.mime_type.as_deref(),
            candidate.suggested_filename.as_deref(),
        ).ok_or("extension download does not match its initiating store")?;

        Ok(InstallIntent {
            store: compatibility.store,
            initiator_origin: compatibility.canonical_origin,
            download_url: download.to_string(),
            package_kind: kind,
            suggested_filename: candidate.suggested_filename,
            gesture_consumed_at_ms: now_ms,
        })
    }
}

pub fn classify_store_url(value: &str) -> Option<StoreCompatibility> {
    let url = parse_trusted_https(value)?;
    let store = match url.host_str()? {
        "addons.mozilla.org" => ExtensionStore::MozillaAddons,
        "chromewebstore.google.com" => ExtensionStore::ChromeWebStore,
        "chrome.google.com" if url.path().starts_with("/webstore") => ExtensionStore::ChromeWebStore,
        "microsoftedge.microsoft.com" => ExtensionStore::EdgeAddons,
        "addons.opera.com" => ExtensionStore::OperaAddons,
        _ => return None,
    };
    Some(StoreCompatibility {
        store,
        canonical_origin: url.origin().ascii_serialization(),
        user_agent: user_agent_mask(store),
    })
}

pub fn user_agent_mask(store: ExtensionStore) -> UserAgentMask {
    match store {
        ExtensionStore::MozillaAddons => UserAgentMask {
            product: "Firefox".into(),
            version_policy: UaVersionPolicy::CurrentStable,
            template: "Mozilla/5.0 ({platform}; rv:{productVersion}) Gecko/20100101 Firefox/{productVersion}".into(),
        },
        ExtensionStore::ChromeWebStore => UserAgentMask {
            product: "Chrome".into(),
            version_policy: UaVersionPolicy::FixedMajor(152),
            template: "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36".into(),
        },
        ExtensionStore::EdgeAddons => UserAgentMask {
            product: "Microsoft Edge".into(),
            version_policy: UaVersionPolicy::CurrentStable,
            template: "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chromiumVersion} Safari/537.36 Edg/{productVersion}".into(),
        },
        ExtensionStore::OperaAddons => UserAgentMask {
            product: "Opera".into(),
            version_policy: UaVersionPolicy::CurrentStable,
            template: "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chromiumVersion} Safari/537.36 OPR/{productVersion}".into(),
        },
    }
}

pub fn classify_package_download(
    store: ExtensionStore,
    download_url: &str,
    mime_type: Option<&str>,
    suggested_filename: Option<&str>,
) -> Option<PackageKind> {
    let url = parse_trusted_https(download_url)?;
    classify_download(store, &url, mime_type, suggested_filename)
}

fn classify_download(
    store: ExtensionStore,
    url: &Url,
    mime_type: Option<&str>,
    filename: Option<&str>,
) -> Option<PackageKind> {
    let host = url.host_str()?;
    let path = url.path();
    let mime = mime_type.and_then(normalize_mime);
    let file = filename.unwrap_or_default().to_ascii_lowercase();
    match store {
        ExtensionStore::MozillaAddons => {
            let endpoint = (host == "addons.mozilla.org" && path.starts_with("/firefox/downloads/file/"))
                || host == "addons.cdn.mozilla.net";
            (endpoint && (path.to_ascii_lowercase().ends_with(".xpi") || file.ends_with(".xpi")
                || mime.is_some_and(|value| value.eq_ignore_ascii_case("application/x-xpinstall"))))
                .then_some(PackageKind::FirefoxXpi)
        }
        ExtensionStore::ChromeWebStore => {
            let endpoint = (host == "clients2.google.com" && path == "/service/update2/crx")
                || (host == "clients2.googleusercontent.com" && path.starts_with("/crx/"));
            (endpoint && crx_signal(path, &file, mime)).then_some(PackageKind::ChromeCrx3)
        }
        ExtensionStore::EdgeAddons => {
            let endpoint = (host == "edge.microsoft.com" && path.starts_with("/extensionwebstorebase/v1/crx"))
                || host == "msedgeextensions.sf.tlu.dl.delivery.mp.microsoft.com";
            (endpoint && crx_signal(path, &file, mime)).then_some(PackageKind::ChromeCrx3)
        }
        ExtensionStore::OperaAddons => {
            let endpoint = host == "addons.opera.com" && path.contains("/extensions/download/");
            (endpoint && crx_signal(path, &file, mime)).then_some(PackageKind::ChromeCrx3)
        }
    }
}

fn crx_signal(path: &str, filename: &str, mime: Option<&str>) -> bool {
    let path = path.to_ascii_lowercase();
    path.ends_with(".crx") || path.ends_with("/crx") || path.contains("/crx/")
        || filename.ends_with(".crx") || mime.is_some_and(|value| {
            value.eq_ignore_ascii_case("application/x-chrome-extension")
                || value.eq_ignore_ascii_case("application/octet-stream")
        })
}

fn normalize_mime(value: &str) -> Option<&str> {
    let mime = value.split(';').next()?.trim();
    (!mime.is_empty()).then_some(mime)
}

fn parse_trusted_https(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        && url.port_or_known_default() == Some(443)).then_some(url)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
