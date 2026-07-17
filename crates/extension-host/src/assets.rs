use getrandom::fill;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread::{self, JoinHandle};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zip::ZipArchive;

#[derive(Clone)]
enum AssetSource { Directory(PathBuf), Archive(PathBuf) }

#[derive(Clone)]
struct AssetRoot {
    source: AssetSource,
    manifest: Value,
    messages: Value,
}

pub struct ExtensionAssetServer {
    address: SocketAddrV4,
    token: String,
    roots: Arc<Mutex<BTreeMap<String, AssetRoot>>>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ExtensionAssetServer {
    pub fn start() -> Result<Self, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| error.to_string())?;
        let address = match listener.local_addr().map_err(|error| error.to_string())? {
            std::net::SocketAddr::V4(value) => value,
            _ => return Err("extension asset server did not bind IPv4 loopback".into()),
        };
        let mut secret = [0u8; 32];
        fill(&mut secret).map_err(|error| error.to_string())?;
        let token = secret.iter().map(|byte| format!("{byte:02x}")).collect();
        let roots = Arc::new(Mutex::new(BTreeMap::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_roots = Arc::clone(&roots);
        let worker_stopping = Arc::clone(&stopping);
        let worker_token = token.clone();
        let thread = thread::Builder::new().name("ubar-extension-assets".into()).spawn(move || {
            for connection in listener.incoming() {
                if worker_stopping.load(Ordering::Acquire) { break; }
                if let Ok(stream) = connection {
                    serve(stream, &worker_token, &worker_roots);
                }
            }
        }).map_err(|error| error.to_string())?;
        Ok(Self { address, token, roots, stopping, thread: Some(thread) })
    }

    pub fn register_directory(&self, id: &str, root: PathBuf, manifest: Value) {
        self.register(id, AssetSource::Directory(root), manifest);
    }

    pub fn register_archive(&self, id: &str, archive: PathBuf, manifest: Value) {
        self.register(id, AssetSource::Archive(archive), manifest);
    }

    fn register(&self, id: &str, source: AssetSource, manifest: Value) {
        let messages = default_messages(&source, &manifest);
        self.roots.lock().unwrap_or_else(|error| error.into_inner()).insert(
            origin_key(id), AssetRoot { source, manifest, messages },
        );
    }

    pub fn unregister(&self, id: &str) {
        self.roots.lock().unwrap_or_else(|error| error.into_inner()).remove(&origin_key(id));
    }

    pub fn url(&self, id: &str, path: &str) -> Result<String, String> {
        validate_id(id)?;
        let path = validate_relative(path)?;
        Ok(format!(
            "http://{}.localhost:{}/{}/{}",
            origin_key(id), self.address.port(), self.token, path,
        ))
    }
}

impl Drop for ExtensionAssetServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

fn serve(mut stream: TcpStream, token: &str, roots: &Mutex<BTreeMap<String, AssetRoot>>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let Ok(reader_stream) = stream.try_clone() else { return };
    let mut reader = BufReader::new(reader_stream);
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() || first.len() > 8192 { return; }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if !matches!(method, "GET" | "HEAD") || !target.starts_with('/') {
        respond(&mut stream, method, 405, "text/plain", b"method not allowed");
        return;
    }
    let mut header_bytes = 0usize;
    let mut host = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() { return; }
        header_bytes += line.len();
        if header_bytes > 64 * 1024 { return; }
        if line == "\r\n" || line == "\n" { break; }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("host") { host = value.trim().to_ascii_lowercase(); }
        }
    }
    let path = target.split('?').next().unwrap_or_default().trim_start_matches('/');
    let mut segments = path.splitn(2, '/');
    if segments.next() != Some(token) {
        respond(&mut stream, method, 404, "text/plain", b"not found");
        return;
    }
    let relative = segments.next().unwrap_or_default();
    let expected_suffix = format!(
        ".localhost:{}", stream.local_addr().map(|value| value.port()).unwrap_or(0),
    );
    let Some(key) = host.strip_suffix(&expected_suffix) else {
        respond(&mut stream, method, 400, "text/plain", b"invalid host");
        return;
    };
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        respond(&mut stream, method, 400, "text/plain", b"invalid host");
        return;
    }
    let Ok(relative) = validate_relative(relative) else {
        respond(&mut stream, method, 400, "text/plain", b"invalid path");
        return;
    };
    let root = roots.lock().unwrap_or_else(|error| error.into_inner()).get(key).cloned();
    let Some(root) = root else {
        respond(&mut stream, method, 404, "text/plain", b"not found");
        return;
    };
    if relative == "__ubar_bridge.js" {
        let body = bridge_script(&root.manifest, &root.messages, &format!("/{token}/"));
        respond(&mut stream, method, 200, "text/javascript; charset=utf-8", body.as_bytes());
        return;
    }
    match read_asset(&root.source, relative) {
        Ok(mut body) => {
            if matches!(Path::new(relative).extension().and_then(|value| value.to_str()), Some("html" | "htm")) {
                body = inject_bridge(body, &format!("/{token}/"));
            }
            respond(&mut stream, method, 200, content_type(Path::new(relative)), &body)
        },
        Err(_) => respond(&mut stream, method, 404, "text/plain", b"not found"),
    }
}

fn respond(stream: &mut TcpStream, method: &str, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status { 200 => "OK", 400 => "Bad Request", 403 => "Forbidden", 404 => "Not Found", _ => "Method Not Allowed" };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nX-Content-Type-Options: nosniff\r\nCross-Origin-Resource-Policy: same-origin\r\nContent-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; object-src 'none'; base-uri 'none'\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    let _ = stream.write_all(header.as_bytes());
    if method != "HEAD" { let _ = stream.write_all(body); }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-@{}".contains(&byte)) {
        return Err("extension ID cannot be used in an asset URL".into());
    }
    Ok(())
}

fn validate_relative(path: &str) -> Result<&str, String> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains('%') ||
        path.starts_with(".ubar-") ||
        path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err("extension asset path is invalid".into());
    }
    Ok(path)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn inject_bridge(body: Vec<u8>, base: &str) -> Vec<u8> {
    let text = match String::from_utf8(body) {
        Ok(value) => value,
        Err(error) => return error.into_bytes(),
    };
    let tag = format!("<script src=\"{base}__ubar_bridge.js\"></script>");
    let lower = text.to_ascii_lowercase();
    if let Some(index) = lower.find("</head>") {
        let mut output = String::with_capacity(text.len() + tag.len());
        output.push_str(&text[..index]);
        output.push_str(tag);
        output.push_str(&text[index..]);
        output.into_bytes()
    } else {
        format!("{tag}{text}").into_bytes()
    }
}

fn default_messages(source: &AssetSource, manifest: &Value) -> Value {
    let Some(locale) = manifest.get("default_locale").and_then(Value::as_str) else {
        return serde_json::json!({});
    };
    let path = format!("_locales/{locale}/messages.json");
    read_asset(source, &path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn read_asset(source: &AssetSource, relative: &str) -> Result<Vec<u8>, String> {
    const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;
    match source {
        AssetSource::Directory(root) => {
            let file = root.join(relative).canonicalize().map_err(|error| error.to_string())?;
            if !file.starts_with(root) || !file.is_file() { return Err("forbidden asset path".into()); }
            let metadata = fs::metadata(&file).map_err(|error| error.to_string())?;
            if metadata.len() > MAX_ASSET_BYTES { return Err("extension asset is too large".into()); }
            fs::read(file).map_err(|error| error.to_string())
        }
        AssetSource::Archive(path) => {
            let file = fs::File::open(path).map_err(|error| error.to_string())?;
            let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
            let mut entry = archive.by_name(relative).map_err(|error| error.to_string())?;
            if entry.is_dir() || entry.size() > MAX_ASSET_BYTES {
                return Err("extension asset is missing or too large".into());
            }
            let mut bytes = Vec::with_capacity(entry.size().min(16 * 1024 * 1024) as usize);
            entry.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
            Ok(bytes)
        }
    }
}

fn bridge_script(manifest: &Value, messages: &Value, base: &str) -> String {
    let manifest = serde_json::to_string(manifest).unwrap_or_else(|_| "{}".into());
    let messages = serde_json::to_string(messages).unwrap_or_else(|_| "{}".into());
    let base = serde_json::to_string(base).unwrap_or_else(|_| "\"/\"".into());
    format!(r#"'use strict';
(() => {{
  const manifest = {manifest};
  const messages = {messages};
  const extensionRoot = new URL({base}, location.origin);
  const finish = (value, callback) => typeof callback === 'function'
    ? queueMicrotask(() => callback(value)) : Promise.resolve(value);
  const keys = value => value == null ? null : Array.isArray(value) ? value
    : typeof value === 'string' ? [value] : Object.keys(value);
  const storage = {{
    get(value, callback) {{
      const result = {{}};
      const requested = keys(value);
      const names = requested || Object.keys(localStorage);
      for (const name of names) {{
        const stored = localStorage.getItem(name);
        if (stored !== null) {{ try {{ result[name] = JSON.parse(stored); }} catch {{}} }}
        else if (value && !Array.isArray(value) && typeof value === 'object' && name in value)
          result[name] = value[name];
      }}
      return finish(result, callback);
    }},
    set(items, callback) {{
      for (const [name, value] of Object.entries(items || {{}}))
        localStorage.setItem(name, JSON.stringify(value));
      return finish(undefined, callback);
    }},
    remove(value, callback) {{
      for (const name of keys(value) || []) localStorage.removeItem(name);
      return finish(undefined, callback);
    }},
    clear(callback) {{ localStorage.clear(); return finish(undefined, callback); }}
  }};
  const browser = {{
    runtime: {{
      getManifest: () => structuredClone(manifest),
      getURL: path => new URL(String(path).replace(/^\/+/, ''), extensionRoot).href,
      getPlatformInfo: callback => finish({{os: navigator.platform, arch: 'unknown'}}, callback),
      getBrowserInfo: callback => finish({{name: 'uBar', vendor: 'uBar'}}, callback)
    }},
    storage: {{local: storage}},
    i18n: {{
      getMessage(name, substitutions) {{
        let value = messages[name]?.message || '';
        const args = Array.isArray(substitutions) ? substitutions : [substitutions];
        args.forEach((item, index) => value = value.replaceAll(`$${{index + 1}}`, item ?? ''));
        return value;
      }},
      getUILanguage: () => navigator.language
    }}
  }};
  Object.defineProperty(globalThis, 'browser', {{value: browser, configurable: false}});
  if (!globalThis.chrome) Object.defineProperty(globalThis, 'chrome', {{value: browser}});
}})();"#)
}

fn origin_key(id: &str) -> String {
    format!("{:x}", Sha256::digest(id.as_bytes()))
}
