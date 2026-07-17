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

pub fn install_helper_script() -> &'static str {
    r#"
document.addEventListener('DOMContentLoaded', function () {
  function installHref() {
    var host = location.hostname.toLowerCase();
    if (host === 'addons.mozilla.org' || host === 'addons.mozilla.com') {
      var link = document.querySelector('a[href*="/downloads/file/"]');
      return link ? link.href : '';
    } else if (host === 'chromewebstore.google.com') {
      var match = location.pathname.match(/\/detail\/[^/]+\/([a-p]{32})/);
      return match ? 'https://clients2.google.com/service/update2/crx?response=redirect&prodversion=152.0.0.0&acceptformat=crx3&x=id%3D' + match[1] + '%26uc' : '';
    }
    return '';
  }

  function updateUbarInstallButton() {
    var href = installHref();
    var button = document.getElementById('ubar-install');
    if (!href) { if (button) button.remove(); return; }
    if (!button) {
      button = document.createElement('button');
      button.id = 'ubar-install';
      button.textContent = 'Install in ubar';
      button.style.cssText = 'position:fixed;bottom:20px;right:20px;z-index:2147483647;padding:12px 20px;border:0;border-radius:999px;background:#1a73e8;color:#fff;font:600 14px system-ui;cursor:pointer;box-shadow:0 4px 12px rgba(0,0,0,.3)';
      button.onclick = function () { location.href = button.dataset.href; };
      document.body.appendChild(button);
    }
    button.dataset.href = href;
  }
  updateUbarInstallButton();
  new MutationObserver(updateUbarInstallButton).observe(document.documentElement, {childList:true, subtree:true});

  document.addEventListener('click', function (event) {
    var control = event.target.closest && event.target.closest('button,[role="button"]');
    if (!control) return;
    var label = (control.textContent || '').trim().toLowerCase();
    if (label.indexOf('add to chrome') < 0 && label.indexOf('add to firefox') < 0) return;
    var href = installHref();
    if (!href) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    location.href = href;
  }, true);
});
"#
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
