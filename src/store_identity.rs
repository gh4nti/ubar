#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreIdentity {
    Firefox,
    Chrome,
    Edge,
    Opera,
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
pub fn for_uri(uri: &str) -> Option<StoreIdentity> {
    let after_scheme = uri.split_once("://")?.1;
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let host = authority
        .split('@')
        .next_back()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let path = after_scheme
        .strip_prefix(authority)
        .unwrap_or_default()
        .split(['?', '#'])
        .next()
        .unwrap_or_default();

    match host.as_str() {
        "addons.mozilla.org" | "addons.mozilla.com" => Some(StoreIdentity::Firefox),
        "chromewebstore.google.com" => Some(StoreIdentity::Chrome),
        "chrome.google.com" if path == "/webstore" || path.starts_with("/webstore/") => {
            Some(StoreIdentity::Chrome)
        }
        "microsoftedge.microsoft.com" => Some(StoreIdentity::Edge),
        "addons.opera.com" => Some(StoreIdentity::Opera),
        _ => None,
    }
}

pub fn user_agent(identity: StoreIdentity) -> &'static str {
    #[cfg(target_os = "macos")]
    match identity {
        StoreIdentity::Firefox => {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0"
        }
        StoreIdentity::Chrome => {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36"
        }
        StoreIdentity::Edge => {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36 Edg/150.0.4078.65"
        }
        StoreIdentity::Opera => {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36 OPR/133.0.5932.34"
        }
    }

    #[cfg(target_os = "windows")]
    match identity {
        StoreIdentity::Firefox => {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0"
        }
        StoreIdentity::Chrome => {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36"
        }
        StoreIdentity::Edge => {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36 Edg/150.0.4078.65"
        }
        StoreIdentity::Opera => {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36 OPR/133.0.5932.34"
        }
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    match identity {
        StoreIdentity::Firefox => {
            "Mozilla/5.0 (X11; Linux x86_64; rv:152.0) Gecko/20100101 Firefox/152.0"
        }
        StoreIdentity::Chrome => {
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36"
        }
        StoreIdentity::Edge => {
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36 Edg/150.0.4078.65"
        }
        StoreIdentity::Opera => {
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36 OPR/133.0.5932.34"
        }
    }
}

pub fn navigator_override_script() -> String {
    let firefox = user_agent(StoreIdentity::Firefox);
    let chrome = user_agent(StoreIdentity::Chrome);
    let edge = user_agent(StoreIdentity::Edge);
    let opera = user_agent(StoreIdentity::Opera);
    format!(
        r#"
(() => {{
  const host = location.hostname.toLowerCase();
  let ua = null;
  if (host === 'addons.mozilla.org' || host === 'addons.mozilla.com') ua = {firefox:?};
  else if (host === 'chromewebstore.google.com' || (host === 'chrome.google.com' && (location.pathname === '/webstore' || location.pathname.startsWith('/webstore/')))) ua = {chrome:?};
  else if (host === 'microsoftedge.microsoft.com') ua = {edge:?};
  else if (host === 'addons.opera.com') ua = {opera:?};
  if (!ua) return;
  try {{ Object.defineProperty(Navigator.prototype, 'userAgent', {{get: () => ua}}); }} catch (_) {{}}
}})();
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_stores_only_by_host() {
        assert_eq!(
            for_uri("https://addons.mozilla.org/en-US/firefox/"),
            Some(StoreIdentity::Firefox)
        );
        assert_eq!(
            for_uri("https://microsoftedge.microsoft.com/addons"),
            Some(StoreIdentity::Edge)
        );
        assert_eq!(for_uri("https://addons.mozilla.org.evil.test"), None);
        assert_eq!(for_uri("https://chrome.google.com/maps"), None);
        assert_eq!(
            for_uri("https://chrome.google.com/webstore/category/extensions"),
            Some(StoreIdentity::Chrome)
        );
    }

    #[test]
    fn requested_chrome_store_identity_is_152() {
        assert!(user_agent(StoreIdentity::Chrome).contains("Chrome/152.0.0.0"));
    }
}
