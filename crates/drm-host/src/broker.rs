use crate::widevine::{AuthorizedWidevine, CdmRequest, CdmResponse};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout};

pub const MAX_CDM_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct CdmLaunchSpec {
    pub helper_executable: PathBuf,
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

/// Platform shells implement this with AppContainer, sandbox-exec/seatbelt, or
/// Linux namespaces+seccomp. There is intentionally no direct unsandboxed launcher.
pub trait SandboxedCdmLauncher {
    fn launch(&self, spec: &CdmLaunchSpec, policy: &CdmSandboxPolicy) -> Result<Child, String>;
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
        let policy = CdmSandboxPolicy::locked_down(spec.component.library.clone());
        let mut child = launcher.launch(&spec, &policy)?;
        let stdin = child.stdin.take().ok_or("CDM helper stdin is not piped")?;
        let stdout = child.stdout.take().ok_or("CDM helper stdout is not piped")?;
        let mut broker = Self { child, channel: FramedJson::new(stdout, stdin) };
        broker.channel.send(&CdmRequest::Initialize {
            key_system: "com.widevine.alpha".into(),
            component_path: spec.component.library,
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
