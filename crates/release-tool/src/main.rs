use serde::Serialize;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct Artifact {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Serialize)]
struct ReleaseManifest {
    schema: u32,
    product: &'static str,
    version: String,
    target: String,
    source_commit: String,
    source_date_epoch: String,
    webkit_commit: String,
    firefox_schema_commit: String,
    signed: bool,
    artifacts: Vec<Artifact>,
}

#[derive(Serialize)]
struct HardwareEvidence {
    ram_bytes: u64,
    cpu: String,
    gpu: String,
    gpu_driver: String,
}

#[derive(Serialize)]
struct SecurityEvidence {
    renderer_escape: String,
    cross_site_storage: String,
    private_persistence: String,
    privileged_ipc: String,
}

#[derive(Serialize)]
struct PerformanceEvidence {
    five_tab_rss_bytes: u64,
    five_tab_target_bytes: u64,
    target_met: bool,
}

#[derive(Serialize)]
struct MediaEvidence {
    youtube_720p_average_fps: f64,
    youtube_720p_dropped_frames_percent: f64,
    hardware_decode: bool,
    low_memory_target_met: bool,
}

#[derive(Serialize)]
struct CertificationEvidence {
    schema: u32,
    product: &'static str,
    target: String,
    source_commit: String,
    artifact_manifest_sha256: String,
    hardware: HardwareEvidence,
    security: SecurityEvidence,
    performance: PerformanceEvidence,
    media: MediaEvidence,
    passed: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ubar-release-tool: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("manifest") => manifest(parse_options(arguments)?),
        Some("certify") => certify(parse_options(arguments)?),
        _ => Err("usage: ubar-release-tool <manifest|certify> --key value ...".into()),
    }
}

fn parse_options(arguments: impl Iterator<Item = String>) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut output = std::collections::BTreeMap::new();
    let mut arguments = arguments.peekable();
    while let Some(key) = arguments.next() {
        if !key.starts_with("--") { return Err(format!("unexpected argument {key}")); }
        let value = arguments.next().ok_or_else(|| format!("{key} requires a value"))?;
        output.insert(key.trim_start_matches("--").into(), value);
    }
    Ok(output)
}

fn required(options: &std::collections::BTreeMap<String, String>, key: &str) -> Result<String, String> {
    options.get(key).cloned().ok_or_else(|| format!("--{key} is required"))
}

fn manifest(options: std::collections::BTreeMap<String, String>) -> Result<(), String> {
    let root = PathBuf::from(required(&options, "artifacts")?);
    let output = PathBuf::from(required(&options, "output")?);
    if !root.is_dir() { return Err("artifact directory is missing".into()); }
    let mut files = Vec::new();
    collect_files(&root, &root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = ReleaseManifest {
        schema: 1,
        product: "uBar",
        version: env_or("UBAR_VERSION", env!("CARGO_PKG_VERSION")),
        target: required(&options, "target")?,
        source_commit: env_or("GITHUB_SHA", "unknown"),
        source_date_epoch: env_or("SOURCE_DATE_EPOCH", "unknown"),
        webkit_commit: env_or("UBAR_WEBKIT_COMMIT", "3af9d4073d75a9b44668cebc03a41223dbef5745"),
        firefox_schema_commit: env_or("UBAR_FIREFOX_SCHEMA_COMMIT", "79223b6429f6d8844435cfdbf68ad699367bfec5"),
        signed: env::var("UBAR_ARTIFACT_SIGNED").as_deref() == Ok("true"),
        artifacts: files,
    };
    write_json(&output, &manifest)
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<Artifact>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_files(root, &path, output)?;
        } else if path.is_file() {
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            output.push(Artifact {
                path: path.strip_prefix(root).map_err(|error| error.to_string())?
                    .to_string_lossy().replace('\\', "/"),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            });
        }
    }
    Ok(())
}

fn certify(options: std::collections::BTreeMap<String, String>) -> Result<(), String> {
    let manifest_path = PathBuf::from(required(&options, "manifest")?);
    let output = PathBuf::from(required(&options, "output")?);
    let manifest = fs::read(&manifest_path).map_err(|error| error.to_string())?;
    serde_json::from_slice::<serde_json::Value>(&manifest).map_err(|error| error.to_string())?;
    let ram = env_u64("UBAR_CERT_RAM_BYTES")?;
    let five_tab_rss = env_u64("UBAR_CERT_FIVE_TAB_RSS_BYTES")?;
    let fps = env_f64("UBAR_CERT_YOUTUBE_720P_AVG_FPS")?;
    let dropped = env_f64("UBAR_CERT_YOUTUBE_720P_DROPPED_PERCENT")?;
    let hardware_decode = env_bool("UBAR_CERT_HARDWARE_DECODE")?;
    let security = SecurityEvidence {
        renderer_escape: env_required("UBAR_CERT_RENDERER_ESCAPE")?,
        cross_site_storage: env_required("UBAR_CERT_CROSS_SITE_STORAGE")?,
        private_persistence: env_required("UBAR_CERT_PRIVATE_PERSISTENCE")?,
        privileged_ipc: env_required("UBAR_CERT_PRIVILEGED_IPC")?,
    };
    let security_passed = security.renderer_escape == "blocked"
        && security.cross_site_storage == "isolated"
        && security.private_persistence == "none"
        && security.privileged_ipc == "authenticated";
    let five_tab_target = 1024 * 1024 * 1024;
    let normal_memory_passed = five_tab_rss <= five_tab_target;
    let low_memory_passed = ram > 600 * 1024 * 1024
        || (fps >= 24.0 && dropped <= 10.0 && hardware_decode && five_tab_rss <= ram * 9 / 10);
    let passed = security_passed && normal_memory_passed && low_memory_passed;
    let evidence = CertificationEvidence {
        schema: 1,
        product: "uBar",
        target: required(&options, "target")?,
        source_commit: env_or("GITHUB_SHA", "unknown"),
        artifact_manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
        hardware: HardwareEvidence {
            ram_bytes: ram,
            cpu: env_required("UBAR_CERT_CPU")?,
            gpu: env_required("UBAR_CERT_GPU")?,
            gpu_driver: env_required("UBAR_CERT_GPU_DRIVER")?,
        },
        security,
        performance: PerformanceEvidence {
            five_tab_rss_bytes: five_tab_rss,
            five_tab_target_bytes: five_tab_target,
            target_met: normal_memory_passed,
        },
        media: MediaEvidence {
            youtube_720p_average_fps: fps,
            youtube_720p_dropped_frames_percent: dropped,
            hardware_decode,
            low_memory_target_met: low_memory_passed,
        },
        passed,
    };
    write_json(&output, &evidence)?;
    if passed { Ok(()) } else { Err("certification gates failed; evidence was written".into()) }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn env_or(key: &str, fallback: &str) -> String { env::var(key).unwrap_or_else(|_| fallback.into()) }
fn env_required(key: &str) -> Result<String, String> { env::var(key).map_err(|_| format!("{key} is required")) }
fn env_u64(key: &str) -> Result<u64, String> { env_required(key)?.parse().map_err(|_| format!("{key} must be u64")) }
fn env_f64(key: &str) -> Result<f64, String> { env_required(key)?.parse().map_err(|_| format!("{key} must be f64")) }
fn env_bool(key: &str) -> Result<bool, String> { env_required(key)?.parse().map_err(|_| format!("{key} must be true or false")) }
