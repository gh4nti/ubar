use crate::widevine::{AuthorizedWidevine, CdmRequest, CdmResponse};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub const MAX_CDM_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct CdmLaunchSpec {
    pub helper_executable: PathBuf,
    pub adapter_library: PathBuf,
    pub component: AuthorizedWidevine,
    pub host_version: u32,
}

#[derive(Clone, Debug)]
pub struct CdmSandboxPolicy {
    pub network_allowed: bool,
    pub child_processes_allowed: bool,
    pub writable_files_allowed: bool,
    pub memory_limit_bytes: u64,
    pub readable_component: PathBuf,
}

impl CdmSandboxPolicy {
    pub fn locked_down(component: PathBuf) -> Self {
        Self {
            network_allowed: false,
            child_processes_allowed: false,
            writable_files_allowed: false,
            memory_limit_bytes: 256 * 1024 * 1024,
            readable_component: component,
        }
    }
}

pub struct LaunchedCdm {
    pub child: Child,
    pub component_path: PathBuf,
}

pub trait SandboxedCdmLauncher {
    fn launch(&self, spec: &CdmLaunchSpec, policy: &CdmSandboxPolicy) -> Result<LaunchedCdm, String>;
}

pub struct NativeSandboxLauncher;

fn validate_launch(spec: &CdmLaunchSpec, policy: &CdmSandboxPolicy) -> Result<(), String> {
    for (name, path) in [
        ("CDM helper", &spec.helper_executable),
        ("licensed CDM adapter", &spec.adapter_library),
        ("Widevine component", &spec.component.library),
    ] {
        if !path.is_absolute() || !path.is_file() {
            return Err(format!("{name} path is invalid: {}", path.display()));
        }
    }
    if policy.network_allowed || policy.child_processes_allowed || policy.writable_files_allowed {
        return Err("CDM sandbox policy is not locked down".into());
    }
    if policy.memory_limit_bytes == 0 || policy.readable_component != spec.component.library {
        return Err("CDM sandbox policy does not match component".into());
    }
    Ok(())
}

fn piped(command: &mut Command) -> Result<Child, String> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
impl SandboxedCdmLauncher for NativeSandboxLauncher {
    fn launch(&self, spec: &CdmLaunchSpec, policy: &CdmSandboxPolicy) -> Result<LaunchedCdm, String> {
        validate_launch(spec, policy)?;
        let worker = Path::new("/app/ubar-cdm-worker");
        let adapter = Path::new("/app/widevine-adapter");
        let component = Path::new("/cdm/component");
        let mut command = Command::new("bwrap");
        command.args([
            "--die-with-parent", "--new-session", "--unshare-all", "--cap-drop", "ALL",
            "--clearenv", "--setenv", "HOME", "/nonexistent", "--setenv", "TMPDIR", "/tmp",
            "--tmpfs", "/tmp", "--proc", "/proc", "--dev", "/dev",
            "--dir", "/app", "--dir", "/cdm",
        ]);
        let memory_limit = policy.memory_limit_bytes.to_string();
        command.args(["--setenv", "UBAR_CDM_MEMORY_LIMIT_BYTES", memory_limit.as_str()]);
        for system in ["/usr", "/lib", "/lib64"] {
            if Path::new(system).exists() { command.args(["--ro-bind", system, system]); }
        }
        command
            .arg("--ro-bind").arg(&spec.helper_executable).arg(worker)
            .arg("--ro-bind").arg(&spec.adapter_library).arg(adapter)
            .arg("--ro-bind").arg(&spec.component.library).arg(component)
            .arg("--").arg(worker).arg(adapter);
        Ok(LaunchedCdm { child: piped(&mut command)?, component_path: component.into() })
    }
}

#[cfg(target_os = "macos")]
impl SandboxedCdmLauncher for NativeSandboxLauncher {
    fn launch(&self, spec: &CdmLaunchSpec, policy: &CdmSandboxPolicy) -> Result<LaunchedCdm, String> {
        validate_launch(spec, policy)?;
        fn seatbelt_path(path: &Path) -> String {
            path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
        }
        let profile = format!(
            "(version 1)(deny default)(allow process-info* sysctl-read)\
             (allow file-read* (subpath \"/System\") (subpath \"/usr/lib\")\
             (literal \"{}\") (literal \"{}\") (literal \"{}\"))",
            seatbelt_path(&spec.helper_executable),
            seatbelt_path(&spec.adapter_library),
            seatbelt_path(&spec.component.library),
        );
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .env_clear()
            .env("UBAR_CDM_MEMORY_LIMIT_BYTES", policy.memory_limit_bytes.to_string())
            .arg("-p").arg(profile)
            .arg(&spec.helper_executable).arg(&spec.adapter_library);
        Ok(LaunchedCdm {
            child: piped(&mut command)?,
            component_path: spec.component.library.clone(),
        })
    }
}

#[cfg(target_os = "windows")]
impl SandboxedCdmLauncher for NativeSandboxLauncher {
    fn launch(&self, spec: &CdmLaunchSpec, policy: &CdmSandboxPolicy) -> Result<LaunchedCdm, String> {
        validate_launch(spec, policy)?;
        let broker = spec.helper_executable.parent().ok_or("CDM helper has no parent directory")?
            .join("ubar-cdm-appcontainer.exe");
        if !broker.is_file() {
            return Err("signed AppContainer CDM broker is missing".into());
        }
        let mut command = Command::new(broker);
        command
            .arg("--worker").arg(&spec.helper_executable)
            .arg("--adapter").arg(&spec.adapter_library)
            .arg("--component").arg(&spec.component.library)
            .arg("--memory-limit").arg(policy.memory_limit_bytes.to_string());
        Ok(LaunchedCdm {
            child: piped(&mut command)?,
            component_path: spec.component.library.clone(),
        })
    }
}

pub struct FramedJson<R, W> {
    reader: R,
    writer: W,
    max_frame_bytes: usize,
}

impl<R: Read, W: Write> FramedJson<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer, max_frame_bytes: MAX_CDM_FRAME_BYTES }
    }

    pub fn send<T: Serialize>(&mut self, value: &T) -> Result<(), String> {
        let payload = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        if payload.len() > self.max_frame_bytes {
            return Err("CDM IPC frame exceeds limit".into());
        }
        let length = u32::try_from(payload.len()).map_err(|_| "CDM IPC frame is too large")?;
        self.writer.write_all(&length.to_le_bytes()).map_err(|error| error.to_string())?;
        self.writer.write_all(&payload).map_err(|error| error.to_string())?;
        self.writer.flush().map_err(|error| error.to_string())
    }

    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<T, String> {
        let mut length = [0; 4];
        self.reader.read_exact(&mut length).map_err(|error| error.to_string())?;
        let length = u32::from_le_bytes(length) as usize;
        if length > self.max_frame_bytes {
            return Err("CDM IPC frame exceeds limit".into());
        }
        let mut payload = vec![0; length];
        self.reader.read_exact(&mut payload).map_err(|error| error.to_string())?;
        serde_json::from_slice(&payload).map_err(|error| error.to_string())
    }
}

pub struct CdmBroker {
    child: Child,
    channel: FramedJson<ChildStdout, ChildStdin>,
}

impl CdmBroker {
    pub fn launch(spec: CdmLaunchSpec, launcher: &dyn SandboxedCdmLauncher) -> Result<Self, String> {
        if !spec.helper_executable.is_file() {
            return Err("CDM helper executable is missing".into());
        }
        if !spec.adapter_library.is_file() {
            return Err("licensed CDM adapter is missing".into());
        }
        let policy = CdmSandboxPolicy::locked_down(spec.component.library.clone());
        let launched = launcher.launch(&spec, &policy)?;
        let mut child = launched.child;
        let stdin = child.stdin.take().ok_or("CDM helper stdin is not piped")?;
        let stdout = child.stdout.take().ok_or("CDM helper stdout is not piped")?;
        let mut broker = Self { child, channel: FramedJson::new(stdout, stdin) };
        broker.channel.send(&CdmRequest::Initialize {
            key_system: "com.widevine.alpha".into(),
            component_path: launched.component_path,
            host_version: spec.host_version,
        })?;
        match broker.channel.receive::<CdmResponse>()? {
            CdmResponse::Initialized { .. } => Ok(broker),
            CdmResponse::Error { reason, .. } => Err(reason),
            _ => Err("CDM helper returned an invalid initialization response".into()),
        }
    }

    pub fn transact(&mut self, request: &CdmRequest) -> Result<CdmResponse, String> {
        self.channel.send(request)?;
        self.channel.receive()
    }

    pub fn shutdown(mut self) -> Result<(), String> {
        self.channel.send(&CdmRequest::Shutdown)?;
        self.child.wait().map_err(|error| error.to_string()).map(|_| ())
    }
}

impl Drop for CdmBroker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
