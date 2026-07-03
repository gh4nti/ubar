use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

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

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    content_scripts: Vec<ContentScript>,
}

pub struct ExtScript {
    pub source: String,
    pub allowlist: Vec<String>,
    pub at_start: bool,
}

pub struct ExtStyle {
    pub source: String,
    pub allowlist: Vec<String>,
}

#[derive(Default)]
pub struct Extensions {
    pub names: Vec<String>,
    pub scripts: Vec<ExtScript>,
    pub styles: Vec<ExtStyle>,
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

    out.names.push(if manifest.name.is_empty() { id } else { manifest.name });
    Some(())
}

// Loads unpacked Firefox-style extensions from <data_dir>/ubar/extensions/<name>/.
pub fn load() -> Extensions {
    let mut out = Extensions::default();
    let root = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ubar")
        .join("extensions");
    let _ = fs::create_dir_all(&root);

    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = load_one(&path, &mut out);
            }
        }
    }

    for name in &out.names {
        println!("ubar: loaded extension: {name}");
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
    fn manifest_parses() {
        let m: Manifest = serde_json::from_str(
            r#"{"name":"x","manifest_version":2,"content_scripts":[{"matches":["<all_urls>"],"js":["a.js"],"run_at":"document_start"}]}"#,
        )
        .unwrap();
        assert_eq!(m.content_scripts.len(), 1);
        assert!(m.content_scripts[0].run_at == "document_start");
    }
}
