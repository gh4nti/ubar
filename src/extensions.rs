use serde::Deserialize;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const BUILTIN_BLOCKER_MANIFEST: &str = include_str!("../assets/extensions/ubar-blocker/manifest.json");
const BUILTIN_BLOCKER_RULES: &str = include_str!("../assets/extensions/ubar-blocker/rules.json");
const BUILTIN_BLOCKER_CSS: &str = include_str!("../assets/extensions/ubar-blocker/blocker.css");

#[derive(Deserialize, Default)]
struct ContentScript {
    #[serde(default)]
    matches: Vec<String>,
    #[serde(default)]
    js: Vec<String>,
    #[serde(default)]
    css: Vec<String>,
    #[serde(default)]
    run_at: String,
}

#[derive(Deserialize, Default)]
struct ActionDef {
    #[serde(default)]
    default_popup: String,
}

#[derive(Deserialize, Default)]
struct OptionsUi {
    #[serde(default)]
    page: String,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    content_scripts: Vec<ContentScript>,
    // The page a click on the extension should open: MV3 action popup, MV2
    // browser_action popup, or the options page — whichever it declares.
    #[serde(default)]
    action: ActionDef,
    #[serde(default)]
    browser_action: ActionDef,
    #[serde(default)]
    options_ui: OptionsUi,
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
pub struct ExtScript {
    pub source: String,
    pub allowlist: Vec<String>,
    pub at_start: bool,
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
impl ExtScript {
    pub fn initialization_source(&self) -> String {
        guarded_source(&self.source, &self.allowlist, self.at_start)
    }
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
pub struct ExtStyle {
    pub source: String,
    pub allowlist: Vec<String>,
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
impl ExtStyle {
    pub fn initialization_source(&self) -> String {
        let css = serde_json::to_string(&self.source).unwrap_or_else(|_| "\"\"".into());
        guarded_source(
            &format!(
                "const s=document.createElement('style');s.textContent={css};(document.head||document.documentElement).appendChild(s);"
            ),
            &self.allowlist,
            true,
        )
    }
}

#[derive(Default)]
pub struct Extensions {
    pub names: Vec<String>,
    pub dirs: Vec<PathBuf>,
    // file:// uri of each extension's popup/options page, "" if it has none.
    pub pages: Vec<String>,
    pub scripts: Vec<ExtScript>,
    pub styles: Vec<ExtStyle>,
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
fn guarded_source(source: &str, patterns: &[String], at_start: bool) -> String {
    let patterns = serde_json::to_string(patterns).unwrap_or_else(|_| "[]".into());
    let run = format!("()=>{{{source}}}");
    let execute = if at_start {
        format!("({run})()")
    } else {
        format!(
            "document.readyState==='loading'?document.addEventListener('DOMContentLoaded',{run},{{once:true}}):({run})()"
        )
    };
    format!(
        r#"(()=>{{
const patterns={patterns};
const escape=value=>value.replace(/[.+?^${{}}()|[\]\\]/g,'\\$&');
const matches=patterns.some(pattern=>{{
 const expression='^'+pattern.split('*').map(escape).join('.*')+'$';
 try{{return new RegExp(expression).test(location.href)}}catch(_error){{return false}}
}});
if(matches){{{execute};}}
}})();"#
    )
}

pub fn root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ubar")
        .join("extensions")
}

// CRX = zip with a signed header prefix; XPI = plain zip. Returns zip offset.
fn zip_start(bytes: &[u8]) -> usize {
    if bytes.len() > 16 && &bytes[0..4] == b"Cr24" {
        let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        if version == 2 {
            let pk = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
            let sig = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
            return 16 + pk + sig;
        }
        if version == 3 {
            let hlen = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
            return 12 + hlen;
        }
    }
    0
}

// Install a downloaded .xpi/.crx into the extensions dir. Returns extension name.
pub fn install_file(path: &Path) -> Result<String, String> {
    let archive_size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if archive_size > MAX_ARCHIVE_BYTES {
        return Err("extension archive is too large".into());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let start = zip_start(&bytes);
    if start >= bytes.len() {
        return Err("corrupt archive".into());
    }
    let cursor = std::io::Cursor::new(&bytes[start..]);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

    let manifest_text = {
        let mut file = archive.by_name("manifest.json").map_err(|e| e.to_string())?;
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(|e| e.to_string())?;
        text
    };
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_text).map_err(|e| e.to_string())?;
    let raw_name = manifest["name"].as_str().unwrap_or("extension");
    // ponytail: __MSG_*__ names need _locales lookup; fall back to file stem.
    let name = if raw_name.starts_with("__MSG_") {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "extension".into())
    } else {
        raw_name.to_string()
    };

    let dir_name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if dir_name.is_empty() {
        return Err("bad extension name".into());
    }
    let target = root().join(dir_name);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let staging = root().join(format!(".install-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    let install_result = (|| -> Result<(), String> {
        if archive.len() > MAX_FILES {
            return Err("extension contains too many files".into());
        }
        let unpacked = (0..archive.len()).try_fold(0u64, |total, index| {
            let file = archive.by_index(index).map_err(|e| e.to_string())?;
            total
                .checked_add(file.size())
                .filter(|size| *size <= MAX_UNPACKED_BYTES)
                .ok_or_else(|| "extension expands beyond the size limit".to_string())
        })?;
        if unpacked == 0 {
            return Err("empty extension archive".into());
        }

        for index in 0..archive.len() {
            let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
            let relative = file
                .enclosed_name()
                .ok_or_else(|| "extension contains an unsafe path".to_string())?;
            if file.is_symlink() {
                return Err("extension contains a symbolic link".into());
            }
            let output = staging.join(relative);
            if file.is_dir() {
                fs::create_dir_all(&output).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut destination = fs::File::create(&output).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut destination).map_err(|e| e.to_string())?;
            destination.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    })();

    if let Err(error) = install_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if target.exists() {
        fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    fs::rename(&staging, &target).map_err(|e| {
        let _ = fs::remove_dir_all(&staging);
        e.to_string()
    })?;
    Ok(name)
}

pub fn ensure_builtins() -> Result<(), String> {
    let directory = root().join("ubar-blocker");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    for (name, contents) in [
        ("manifest.json", BUILTIN_BLOCKER_MANIFEST),
        ("rules.json", BUILTIN_BLOCKER_RULES),
        ("blocker.css", BUILTIN_BLOCKER_CSS),
    ] {
        let path = directory.join(name);
        if fs::read_to_string(&path).ok().as_deref() != Some(contents) {
            fs::write(path, contents).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn uninstall(name: &str, extensions: &Extensions) -> bool {
    if let Some(index) = extensions.names.iter().position(|n| n == name)
        && let Some(dir) = extensions.dirs.get(index)
        && dir.starts_with(root())
    {
        return fs::remove_dir_all(dir).is_ok();
    }
    false
}

// Firefox match patterns -> WebKit user content allowlist patterns.
fn to_webkit_patterns(matches: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for m in matches {
        if m == "<all_urls>" {
            out.push("http://*/*".into());
            out.push("https://*/*".into());
        } else if let Some(rest) = m.strip_prefix("*://") {
            out.push(format!("http://{rest}"));
            out.push(format!("https://{rest}"));
        } else {
            out.push(m.clone());
        }
    }
    out
}

// Minimal browser.* shim: storage.local (localStorage-backed, per-origin),
// runtime.getURL/getManifest, no-op messaging. Enough for content-script-only
// extensions (dark readers, redirectors, style/script tweaks).
// ponytail: no background pages, no webRequest, no cross-origin storage.
const POLYFILL: &str = r#";(function(){
const __k='__ubar_ext_%ID%';
const __read=()=>{try{return JSON.parse(localStorage.getItem(__k)||'{}')}catch(e){return{}}};
const __write=(a)=>{try{localStorage.setItem(__k,JSON.stringify(a))}catch(e){}};
const __area={
 get:(keys)=>{const a=__read();if(keys==null)return Promise.resolve(a);
  if(typeof keys==='string')keys=[keys];
  if(Array.isArray(keys)){const r={};for(const k of keys)if(k in a)r[k]=a[k];return Promise.resolve(r)}
  const r={};for(const k in keys)r[k]=(k in a)?a[k]:keys[k];return Promise.resolve(r)},
 set:(items)=>{const a=__read();Object.assign(a,items);__write(a);return Promise.resolve()},
 remove:(keys)=>{const a=__read();for(const k of [].concat(keys))delete a[k];__write(a);return Promise.resolve()},
 clear:()=>{try{localStorage.removeItem(__k)}catch(e){}return Promise.resolve()},
 onChanged:{addListener(){},removeListener(){}}
};
const browser={
 runtime:{
  id:'%ID%',
  getURL:(p)=>'%BASE%'+String(p).replace(/^\//,''),
  getManifest:()=>(%MANIFEST%),
  sendMessage:()=>Promise.resolve(undefined),
  onMessage:{addListener(){},removeListener(){},hasListener:()=>false},
  connect:()=>({postMessage(){},disconnect(){},onMessage:{addListener(){}},onDisconnect:{addListener(){}}})
 },
 storage:{local:__area,sync:__area,onChanged:{addListener(){},removeListener(){}}},
 i18n:{getMessage:()=>''}
};
const chrome=browser;
%SCRIPTS%
})();"#;

fn read_all(dir: &Path, files: &[String]) -> String {
    files
        .iter()
        .filter_map(|f| fs::read_to_string(dir.join(f)).ok())
        .collect::<Vec<_>>()
        .join("\n;\n")
}

fn load_one(dir: &Path, out: &mut Extensions) -> Option<()> {
    let manifest_text = fs::read_to_string(dir.join("manifest.json")).ok()?;
    let manifest: Manifest = serde_json::from_str(&manifest_text).ok()?;
    let id = dir
        .file_name()?
        .to_string_lossy()
        .replace(['\'', '\\', '%'], "_");
    let base = format!("file://{}/", dir.to_string_lossy().replace('\\', "/"));

    for cs in &manifest.content_scripts {
        let allowlist = to_webkit_patterns(&cs.matches);
        if allowlist.is_empty() {
            continue;
        }
        if !cs.js.is_empty() {
            let scripts = read_all(dir, &cs.js);
            if !scripts.is_empty() {
                let source = POLYFILL
                    .replace("%ID%", &id)
                    .replace("%BASE%", &base)
                    .replace("%MANIFEST%", manifest_text.trim())
                    .replace("%SCRIPTS%", &scripts);
                out.scripts.push(ExtScript {
                    source,
                    allowlist: allowlist.clone(),
                    at_start: cs.run_at == "document_start",
                });
            }
        }
        if !cs.css.is_empty() {
            let css = read_all(dir, &cs.css);
            if !css.is_empty() {
                out.styles.push(ExtStyle { source: css, allowlist });
            }
        }
    }

    let page = [
        &manifest.action.default_popup,
        &manifest.browser_action.default_popup,
        &manifest.options_ui.page,
    ]
    .into_iter()
    .find(|p| !p.is_empty())
    .map(|p| format!("{base}{}", p.trim_start_matches('/')))
    .unwrap_or_default();

    out.names.push(if manifest.name.is_empty() { id } else { manifest.name });
    out.dirs.push(dir.to_path_buf());
    out.pages.push(page);
    Some(())
}

// Loads unpacked Firefox/Chrome-style extensions from <data_dir>/ubar/extensions/<name>/.
pub fn load() -> Extensions {
    let mut out = Extensions::default();
    let root = root();
    let _ = fs::create_dir_all(&root);

    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = load_one(&path, &mut out);
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_convert() {
        let out = to_webkit_patterns(&["<all_urls>".into(), "*://example.com/*".into()]);
        assert_eq!(
            out,
            vec![
                "http://*/*",
                "https://*/*",
                "http://example.com/*",
                "https://example.com/*"
            ]
        );
    }

    #[test]
    fn crx_header_skipped() {
        let mut crx3 = b"Cr24".to_vec();
        crx3.extend(3u32.to_le_bytes());
        crx3.extend(5u32.to_le_bytes());
        crx3.extend([0u8; 5]);
        crx3.extend(b"PKzip");
        assert_eq!(zip_start(&crx3), 17);
        assert_eq!(zip_start(b"PK\x03\x04 plain zip content here"), 0);
    }

    #[test]
    fn manifest_parses() {
        let m: Manifest = serde_json::from_str(
            r#"{"name":"x","manifest_version":2,"content_scripts":[{"matches":["<all_urls>"],"js":["a.js"],"run_at":"document_start"}]}"#,
        )
        .unwrap();
        assert_eq!(m.content_scripts.len(), 1);
        assert!(m.content_scripts[0].run_at == "document_start");
    }
}
