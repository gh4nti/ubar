#![deny(unsafe_op_in_unsafe_fn)]

mod backend;

use backend::WebKitBackend;
use std::collections::BTreeMap;
use core::ffi::c_void;
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use ubar_browser_core::BrowserCore;
use ubar_browser_core::hibernation::HibernationSnapshot;
use ubar_browser_core::secrets::NativeSecretStore;
use ubar_browser_core::services::{BrowserControlRequest, ProfileServices};
use ubar_drm_host::broker::{CdmBroker, CdmLaunchSpec, NativeSandboxLauncher};
use ubar_drm_host::widevine::{
    AuthorizedWidevine, CdmRegistrationBundle, Ed25519AuthorizationVerifier,
};
use ubar_engine_abi::*;
use ubar_extension_host::catalog::{ExtensionCatalog, ExtensionControlRequest};
use ubar_extension_host::bridge::{isolated_world_provider, trusted_context_provider};
use ubar_extension_host::dispatcher::{ApiRequest, BrowserProvider, ProviderError};
use ubar_extension_host::execution::{ExecutionAction, ExtensionExecutor};
use ubar_extension_host::runtime::{ContextId, ContextKind, ProfileScope};
use ubar_extension_host::worlds::{NavigationInjectionPlan, RunAt, WorldId};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeBridgeEnvelope {
    world: String,
    message: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeApiRequest {
    #[serde(default)]
    capability: Option<String>,
    #[serde(default)]
    request_id: u64,
    namespace: String,
    member: String,
    #[serde(default)]
    arguments: serde_json::Value,
    #[serde(default)]
    user_gesture: bool,
    #[serde(default)]
    reply_to: Option<u64>,
    #[serde(default)]
    result: serde_json::Value,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Clone, Default)]
struct EngineBrowserProvider {
    tabs: Arc<Mutex<BTreeMap<u64, ProviderTab>>>,
}

#[derive(Clone)]
struct ProviderTab { profile: ProfileScope, uri: String, active: bool }

impl BrowserProvider for EngineBrowserProvider {
    fn tabs_query(&self, profile: ProfileScope, query: &serde_json::Value) -> Result<serde_json::Value, String> {
        let active = query.get("active").and_then(serde_json::Value::as_bool);
        let tabs = self.tabs.lock().unwrap_or_else(|error| error.into_inner()).iter()
            .filter(|(_, tab)| tab.profile == profile && active.is_none_or(|value| tab.active == value))
            .map(|(id, tab)| serde_json::json!({"id": id, "url": tab.uri, "active": tab.active}))
            .collect::<Vec<_>>();
        Ok(serde_json::Value::Array(tabs))
    }
    fn tabs_create(&mut self, _profile: ProfileScope, _properties: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("tabs.create requires the native shell tab factory".into())
    }
    fn tabs_update(&mut self, _profile: ProfileScope, _tab_id: u64, _properties: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("tabs.update requires the native shell tab controller".into())
    }
    fn tabs_remove(&mut self, _profile: ProfileScope, _tab_ids: &[u64]) -> Result<(), String> {
        Err("tabs.remove requires the native shell tab controller".into())
    }
    fn api_call(&mut self, _profile: ProfileScope, namespace: &str, member: &str,
        _arguments: &serde_json::Value) -> Result<serde_json::Value, ProviderError> {
        Err(ProviderError::Unsupported(format!("{namespace}.{member} needs a native provider")))
    }
}

const DEFAULT_REQUEST_POLICY: &[u8] =
    include_bytes!("../../../config/default-content-blocker.json");

struct HostState {
    core: BrowserCore,
    zoom: BTreeMap<u64, f64>,
    cdms: BTreeMap<u64, CdmBroker>,
    request_policies: BTreeMap<u64, serde_json::Value>,
    extensions: ExtensionCatalog,
    extension_executor: ExtensionExecutor<EngineBrowserProvider>,
    extension_tabs: Arc<Mutex<BTreeMap<u64, ProviderTab>>>,
    pending_injections: BTreeMap<u64, Vec<NavigationInjectionPlan>>,
    navigation_serials: BTreeMap<u64, u64>,
    background_views: BTreeMap<u64, BackgroundView>,
    next_background_view: u64,
    profile_data: BTreeMap<u64, PathBuf>,
    services: BTreeMap<u64, ProfileServices>,
    views: BTreeMap<u64, StoredView>,
    backend: Option<WebKitBackend>,
}

#[derive(Clone, Copy)]
struct StoredViewConfig {
    native_parent: usize,
    width: u32,
    height: u32,
    device_scale: f64,
    initially_visible: bool,
}

impl StoredViewConfig {
    fn from_abi(value: &UbarViewConfigV1) -> Self {
        Self { native_parent: value.native_parent as usize, width: value.width, height: value.height,
            device_scale: value.device_scale, initially_visible: value.initially_visible }
    }
    fn to_abi(self, visible: bool) -> UbarViewConfigV1 {
        UbarViewConfigV1 { struct_size: std::mem::size_of::<UbarViewConfigV1>() as u32,
            native_parent: self.native_parent as *mut c_void, width: self.width, height: self.height,
            device_scale: self.device_scale, initially_visible: visible }
    }
}

#[derive(Clone, Copy)]
struct StoredCallbacks {
    user_data: usize,
    event: Option<UbarEventCallbackV1>,
}

impl StoredCallbacks {
    fn from_abi(value: &UbarCallbacksV1) -> Self { Self { user_data: value.user_data as usize, event: value.event } }
    fn to_abi(self) -> UbarCallbacksV1 { UbarCallbacksV1 { struct_size: std::mem::size_of::<UbarCallbacksV1>() as u32,
        user_data: self.user_data as *mut c_void, event: self.event } }
}

#[derive(Clone, Copy)]
struct StoredView {
    profile: u64,
    config: StoredViewConfig,
    callbacks: Option<StoredCallbacks>,
    hibernated: bool,
}

struct BackgroundView {
    context: ContextId,
    world: String,
    script: String,
}

impl Default for HostState {
    fn default() -> Self {
        let extension_tabs = Arc::new(Mutex::new(BTreeMap::new()));
        Self {
            core: BrowserCore::default(),
            zoom: BTreeMap::new(),
            cdms: BTreeMap::new(),
            request_policies: BTreeMap::new(),
            extensions: ExtensionCatalog::default(),
            extension_executor: ExtensionExecutor::new(EngineBrowserProvider { tabs: Arc::clone(&extension_tabs) }),
            extension_tabs,
            pending_injections: BTreeMap::new(),
            navigation_serials: BTreeMap::new(),
            background_views: BTreeMap::new(),
            next_background_view: 1u64 << 63,
            profile_data: BTreeMap::new(),
            services: BTreeMap::new(),
            views: BTreeMap::new(),
            backend: WebKitBackend::load_default().ok(),
        }
    }
}

fn host() -> &'static Mutex<HostState> {
    static HOST: OnceLock<Mutex<HostState>> = OnceLock::new();
    HOST.get_or_init(|| Mutex::new(HostState::default()))
}

fn sync_runtime_extensions(host: &mut HostState, scope: ProfileScope) {
    for extension in host.extensions.runtime_extensions(scope) {
        if host.extension_executor.is_installed(&extension.id, scope) { continue; }
        if let Ok(actions) = host.extension_executor.install(
            extension.id, scope, extension.base_url, extension.manifest,
        ) {
            for action in actions { process_extension_action(host, action); }
        }
    }
}

fn profile_for_scope(host: &HostState, scope: ProfileScope) -> Option<u64> {
    match scope {
        ProfileScope::Private(id) => host.core.has_profile(id).then_some(id),
        ProfileScope::Normal => host.services.keys().copied()
            .find(|id| profile_scope(host, *id) == Some(ProfileScope::Normal)),
    }
}

fn build_background_script(
    host: &HostState,
    context: &ContextId,
    source: &ubar_extension_host::manifest::BackgroundSpec,
) -> Result<String, String> {
    if source.module { return Err("module service-worker backgrounds are not supported by WebKit evaluation".into()); }
    if source.page.is_some() && source.scripts.is_empty() {
        return Err("HTML background pages need a dedicated extension URL scheme".into());
    }
    let paths = source.service_worker.iter().chain(&source.scripts);
    let mut script = trusted_context_provider();
    for path in paths {
        let bytes = host.extensions.resource(context.profile, &context.extension, path)?;
        let source = String::from_utf8(bytes).map_err(|_| "background script is not UTF-8")?;
        script.push('\n');
        script.push_str(&source);
        script.push_str("\n//# sourceURL=ubar-extension://");
        script.push_str(&context.extension.0);
        script.push('/');
        script.push_str(path);
    }
    Ok(script)
}

fn destroy_background_view(host: &mut HostState, view: u64) {
    if host.background_views.remove(&view).is_some() {
        if let Some(backend) = host.backend.as_ref() { let _ = backend.destroy_view(view); }
    }
}

fn destroy_extension_backgrounds(host: &mut HostState, extension: &ubar_extension_host::runtime::ExtensionId,
                                 scope: ProfileScope) {
    let views = host.background_views.iter().filter_map(|(view, background)|
        (background.context.profile == scope && background.context.extension == *extension).then_some(*view))
        .collect::<Vec<_>>();
    for view in views { destroy_background_view(host, view); }
}

fn process_extension_action(host: &mut HostState, action: ExecutionAction) {
    match action {
        ExecutionAction::StopBackground { context } => {
            if let Some(view) = host.background_views.iter()
                .find_map(|(view, background)| (background.context == context).then_some(*view))
            {
                destroy_background_view(host, view);
            }
        }
        ExecutionAction::StartBackground { context, source } => {
            let Some(profile) = profile_for_scope(host, context.profile) else {
                host.extension_executor.background_crashed(&context);
                return;
            };
            let script = match build_background_script(host, &context, &source) {
                Ok(value) => value,
                Err(_) => {
                    host.extension_executor.background_crashed(&context);
                    return;
                }
            };
            let view = host.next_background_view;
            host.next_background_view = host.next_background_view.wrapping_add(1).max(1u64 << 63);
            let callbacks = UbarCallbacksV1 {
                struct_size: std::mem::size_of::<UbarCallbacksV1>() as u32,
                user_data: view as usize as *mut c_void,
                event: Some(engine_event),
            };
            let Some(backend) = host.backend.as_ref() else {
                host.extension_executor.background_crashed(&context);
                return;
            };
            if backend.create_headless_view(view, profile, &callbacks) != UbarResult::Ok {
                host.extension_executor.background_crashed(&context);
                return;
            }
            host.background_views.insert(view, BackgroundView {
                context, world: format!("ubar-background-{view}"), script,
            });
            let uri = b"about:blank";
            if backend.navigate(view, UbarBytes { data: uri.as_ptr(), len: uri.len() }) != UbarResult::Ok {
                if let Some(background) = host.background_views.get(&view) {
                    host.extension_executor.background_crashed(&background.context.clone());
                }
                destroy_background_view(host, view);
            }
        }
    }
}

fn drain_extension_actions(host: &mut HostState) {
    let actions = host.extension_executor.tick(Instant::now());
    for action in actions.into_iter().flatten() { process_extension_action(host, action); }
    while let Some(action) = host.extension_executor.next_action() {
        process_extension_action(host, action);
    }
}

fn evaluate_background(host: &HostState, view: u64, script: &str) -> UbarResult {
    let Some(background) = host.background_views.get(&view) else { return UbarResult::InvalidArgument };
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    backend.evaluate_extension_script(view,
        UbarBytes { data: background.world.as_ptr(), len: background.world.len() },
        UbarBytes { data: script.as_ptr(), len: script.len() })
}

fn drain_background_delivery(host: &mut HostState, view: u64) {
    let Some(context) = host.background_views.get(&view).map(|value| value.context.clone()) else { return };
    while let Some(event) = host.extension_executor.next_background_event(&context) {
        let delivery = serde_json::json!({"kind":"event", "namespace":event.namespace,
            "name":event.name, "arguments":event.arguments});
        let script = format!("globalThis.__ubarExtensionDeliver?.({delivery});");
        let _ = evaluate_background(host, view, &script);
    }
    while let Some(message) = host.extension_executor.next_message(&context) {
        let delivery = serde_json::json!({"kind":"message", "requestId":message.request_id,
            "payload":message.payload, "sender":message.sender});
        let script = format!("globalThis.__ubarExtensionDeliver?.({delivery});");
        let _ = evaluate_background(host, view, &script);
    }
    while let Some(reply) = host.extension_executor.next_reply(&context) {
        let delivery = match reply.result {
            Ok(result) => serde_json::json!({"kind":"response", "requestId":reply.request_id, "result":result}),
            Err(error) => serde_json::json!({"kind":"response", "requestId":reply.request_id,
                "error":{"code":"MESSAGE_ERROR", "message":error}}),
        };
        let script = format!("globalThis.__ubarExtensionDeliver?.({delivery});");
        let _ = evaluate_background(host, view, &script);
    }
}

fn injection_script(host: &HostState, plan: &NavigationInjectionPlan, run_at: RunAt) -> String {
    let mut script = isolated_world_provider(&plan.world);
    for injection in plan.injections.iter().filter(|injection| injection.run_at == run_at) {
        for path in &injection.css_files {
            if let Ok(bytes) = host.extensions.resource(plan.world.id.profile, &plan.world.id.extension, path) {
                if let Ok(value) = String::from_utf8(bytes) {
                    if let Ok(css) = serde_json::to_string(&value) {
                        script.push_str("\n{const s=document.createElement('style');s.textContent=");
                        script.push_str(&css);
                        script.push_str(";(document.head||document.documentElement).appendChild(s);}");
                    }
                }
            }
        }
        for css in &injection.css {
            if let Ok(css) = serde_json::to_string(css) {
                script.push_str("\n{const s=document.createElement('style');s.textContent=");
                script.push_str(&css);
                script.push_str(";(document.head||document.documentElement).appendChild(s);}");
            }
        }
        for path in &injection.files {
            if let Ok(bytes) = host.extensions.resource(plan.world.id.profile, &plan.world.id.extension, path) {
                if let Ok(source) = String::from_utf8(bytes) {
                    script.push('\n'); script.push_str(&source);
                    script.push_str("\n//# sourceURL=ubar-extension://");
                    script.push_str(&plan.world.id.extension.0); script.push('/'); script.push_str(path);
                }
            }
        }
        for source in &injection.code { script.push('\n'); script.push_str(source); }
    }
    script
}

fn inject_run_at(host: &mut HostState, view: u64, plans: &[NavigationInjectionPlan], run_at: RunAt) {
    for plan in plans {
        if !plan.injections.iter().any(|injection| injection.run_at == run_at) { continue; }
        let script = injection_script(host, plan, run_at);
        let world = serde_json::to_string(&plan.world.id).unwrap_or_default();
        let Some(backend) = host.backend.as_ref() else { continue };
        let status = backend.evaluate_extension_script(view,
            UbarBytes { data: world.as_ptr(), len: world.len() },
            UbarBytes { data: script.as_ptr(), len: script.len() });
        if status == UbarResult::Ok { host.extension_executor.activate_world(&plan.world); }
    }
}

fn handle_extension_message(host: &mut HostState, view: u64, text: &str) {
    const MAX_BRIDGE_MESSAGE: usize = 1024 * 1024;
    if text.len() > MAX_BRIDGE_MESSAGE { return; }
    let Ok(envelope) = serde_json::from_str::<NativeBridgeEnvelope>(text) else { return };
    let Ok(world) = serde_json::from_str::<WorldId>(&envelope.world) else { return };
    if world.tab_id != view || world.frame_id != 0 { return; }
    let Ok(incoming) = serde_json::from_str::<NativeApiRequest>(&envelope.message) else { return };
    let Some(capability) = incoming.capability.as_deref() else { return };
    let placeholder = ContextId {
        extension: world.extension.clone(), profile: world.profile, serial: 0,
        kind: ContextKind::ContentScript { tab_id: view, frame_id: 0 },
    };
    let response = host.extension_executor.dispatch_world_api_encoded(
        &world, capability,
        ApiRequest { request_id: incoming.request_id, context: placeholder,
            namespace: incoming.namespace, member: incoming.member,
            arguments: incoming.arguments, user_gesture: incoming.user_gesture },
    );
    let delivery = match response.result {
        Ok(result) => serde_json::json!({"kind":"response", "requestId":response.request_id, "result":result}),
        Err(error) => serde_json::json!({"kind":"response", "requestId":response.request_id, "error":error}),
    };
    let script = format!("globalThis.__ubarExtensionDeliver?.({delivery});");
    let Some(backend) = host.backend.as_ref() else { return };
    let _ = backend.evaluate_extension_script(view,
        UbarBytes { data: envelope.world.as_ptr(), len: envelope.world.len() },
        UbarBytes { data: script.as_ptr(), len: script.len() });
    drain_extension_actions(host);
}

fn handle_background_message(host: &mut HostState, view: u64, text: &str) {
    if text.len() > 1024 * 1024 { return; }
    let Ok(envelope) = serde_json::from_str::<NativeBridgeEnvelope>(text) else { return };
    let Some(background) = host.background_views.get(&view) else { return };
    if envelope.world != background.world { return; }
    let context = background.context.clone();
    let Ok(incoming) = serde_json::from_str::<NativeApiRequest>(&envelope.message) else { return };
    if incoming.capability.is_some() { return; }
    if let Some(request_id) = incoming.reply_to {
        let result = incoming.error.map_or_else(|| Ok(incoming.result), Err);
        let _ = host.extension_executor.reply_message(&context, request_id, result);
        return;
    }
    if incoming.request_id == 0 { return; }
    let response = host.extension_executor.dispatch_api(ApiRequest {
        request_id: incoming.request_id, context, namespace: incoming.namespace,
        member: incoming.member, arguments: incoming.arguments, user_gesture: incoming.user_gesture,
    });
    let delivery = match response.result {
        Ok(result) => serde_json::json!({"kind":"response", "requestId":response.request_id, "result":result}),
        Err(error) => serde_json::json!({"kind":"response", "requestId":response.request_id, "error":error}),
    };
    let script = format!("globalThis.__ubarExtensionDeliver?.({delivery});");
    let _ = evaluate_background(host, view, &script);
    drain_extension_actions(host);
    drain_background_delivery(host, view);
}

unsafe extern "C" fn engine_event(user_data: *mut c_void, event: *const UbarEventV1) {
    if event.is_null() { return; }
    let event = unsafe { *event };
    let view = user_data as usize as u64;
    let text = unsafe { bytes(event.text_utf8) }.ok()
        .and_then(|value| std::str::from_utf8(value).ok()).unwrap_or_default().to_owned();
    let callback = {
        let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
        if host.background_views.contains_key(&view) {
            if event.kind == UbarEventKind::ExtensionMessage {
                handle_background_message(&mut host, view, &text);
            } else if event.kind == UbarEventKind::NavigationFinished {
                let script = host.background_views.get(&view).map(|value| value.script.clone());
                if let Some(script) = script {
                    let context = host.background_views.get(&view).unwrap().context.clone();
                    if evaluate_background(&host, view, &script) == UbarResult::Ok
                        && host.extension_executor.background_started(&context).is_ok()
                    {
                        drain_background_delivery(&mut host, view);
                    } else {
                        host.extension_executor.background_crashed(&context);
                        destroy_background_view(&mut host, view);
                    }
                }
            } else if event.kind == UbarEventKind::RendererCrashed {
                if let Some(context) = host.background_views.get(&view).map(|value| value.context.clone()) {
                    host.extension_executor.background_crashed(&context);
                }
                destroy_background_view(&mut host, view);
            }
            drain_extension_actions(&mut host);
            return;
        }
        if event.kind == UbarEventKind::ExtensionMessage {
            handle_extension_message(&mut host, view, &text);
            return;
        } else if event.kind == UbarEventKind::NavigationCommitted {
            let serial = host.navigation_serials.entry(view).or_default();
            *serial = serial.wrapping_add(1).max(1);
            let navigation = *serial;
            if let Some(stored) = host.views.get(&view).copied() {
                if let Some(scope) = profile_scope(&host, stored.profile) {
                    let plans = host.extension_executor.plan_navigation(
                        scope, view, 0, navigation, &text, None,
                    ).unwrap_or_default();
                    inject_run_at(&mut host, view, &plans, RunAt::DocumentStart);
                    host.pending_injections.insert(view, plans);
                }
                if let Some(tab) = host.extension_tabs.lock().unwrap_or_else(|error| error.into_inner()).get_mut(&view) {
                    tab.uri = text.clone();
                }
            }
        } else if event.kind == UbarEventKind::NavigationFinished {
            if let Some(plans) = host.pending_injections.remove(&view) {
                inject_run_at(&mut host, view, &plans, RunAt::DocumentEnd);
                inject_run_at(&mut host, view, &plans, RunAt::DocumentIdle);
            }
        } else if event.kind == UbarEventKind::RendererCrashed {
            host.pending_injections.remove(&view);
        }
        drain_extension_actions(&mut host);
        host.views.get(&view).and_then(|stored| stored.callbacks)
    };
    if let Some(callbacks) = callback {
        if let Some(callback) = callbacks.event {
            let mut forwarded = event;
            forwarded.view = view;
            unsafe { callback(callbacks.user_data as *mut c_void, &forwarded) };
        }
    }
}

unsafe fn bytes<'a>(value: UbarBytes) -> Result<&'a [u8], UbarResult> {
    if value.len == 0 {
        return Ok(&[]);
    }
    if value.data.is_null() {
        return Err(UbarResult::InvalidArgument);
    }
    Ok(unsafe { std::slice::from_raw_parts(value.data, value.len) })
}

unsafe fn text<'a>(value: UbarBytes) -> Result<&'a str, UbarResult> {
    std::str::from_utf8(unsafe { bytes(value)? }).map_err(|_| UbarResult::InvalidArgument)
}

unsafe extern "C" fn create_profile(
    config: *const UbarProfileConfigV1,
    profile_out: *mut u64,
) -> UbarResult {
    if config.is_null() || profile_out.is_null() {
        return UbarResult::InvalidArgument;
    }
    let config = unsafe { &*config };
    if config.struct_size < std::mem::size_of::<UbarProfileConfigV1>() as u32 {
        return UbarResult::AbiMismatch;
    }
    if !config.require_sandbox
        || !config.partition_third_party_storage
        || !config.block_third_party_cookies
        || config.memory_target_bytes == 0
        || config.memory_ceiling_bytes < config.memory_target_bytes
    {
        return UbarResult::PermissionDenied;
    }
    if config.kind == UbarProfileKind::Private
        && (config.data_directory_utf8.len != 0 || config.cache_directory_utf8.len != 0)
    {
        return UbarResult::PermissionDenied;
    }
    let profile_data = if config.kind == UbarProfileKind::Normal {
        match unsafe { text(config.data_directory_utf8) } {
            Ok(value) if !value.is_empty() => Some(PathBuf::from(value)),
            Ok(_) => default_profile_directory(),
            Err(error) => return error,
        }
    } else {
        None
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if host.backend.is_none() {
        return UbarResult::EngineFailure;
    }
    let profile = host.core.create_profile_with_memory(
        config.kind, config.memory_target_bytes, config.memory_ceiling_bytes,
    );
    let result = host.backend.as_ref().unwrap().create_profile(profile.id, config);
    if result != UbarResult::Ok {
        host.core.destroy_profile(profile.id);
        return result;
    }
    let default_policy = UbarBytes {
        data: DEFAULT_REQUEST_POLICY.as_ptr(),
        len: DEFAULT_REQUEST_POLICY.len(),
    };
    let result = host.backend.as_ref().unwrap().set_request_policy_json(profile.id, default_policy);
    if result != UbarResult::Ok {
        host.backend.as_ref().unwrap().destroy_profile(profile.id);
        host.core.destroy_profile(profile.id);
        return result;
    }
    if let Ok(policy) = serde_json::from_slice(DEFAULT_REQUEST_POLICY) {
        host.request_policies.insert(profile.id, policy);
    }
    let services = if config.kind == UbarProfileKind::Normal {
        let Some(directory) = profile_data.as_ref() else {
            host.backend.as_ref().unwrap().destroy_profile(profile.id);
            host.request_policies.remove(&profile.id);
            host.core.destroy_profile(profile.id);
            return UbarResult::EngineFailure;
        };
        match ProfileServices::open_persistent(directory, Arc::new(NativeSecretStore)) {
            Ok(value) => value,
            Err(_) => {
                host.backend.as_ref().unwrap().destroy_profile(profile.id);
                host.request_policies.remove(&profile.id);
                host.core.destroy_profile(profile.id);
                return UbarResult::EngineFailure;
            }
        }
    } else {
        ProfileServices::private(Arc::new(NativeSecretStore))
    };
    host.services.insert(profile.id, services);
    if let Some(directory) = profile_data {
        host.profile_data.insert(profile.id, directory.clone());
        if let Some((xpi_verifier, crx3_verifier)) = extension_verifiers() {
            let _ = host.extensions.load_installed(
                ProfileScope::Normal, &xpi_verifier, &crx3_verifier,
                &directory.join("extensions"),
            );
        }
    }
    if cfg!(feature = "developer-extensions") && config.kind == UbarProfileKind::Normal {
        if let Some(paths) = std::env::var_os("UBAR_UNPACKED_EXTENSIONS") {
            for path in std::env::split_paths(&paths) {
                let _ = host.extensions.load_unpacked(ProfileScope::Normal, &path, true);
            }
        }
    }
    let scope = profile_scope(&host, profile.id).unwrap();
    sync_runtime_extensions(&mut host, scope);
    unsafe { *profile_out = profile.id };
    UbarResult::Ok
}

unsafe extern "C" fn destroy_profile(profile: u64) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let scope = match host.core.profile(profile).map(|value| value.kind) {
        Some(UbarProfileKind::Normal) => ProfileScope::Normal,
        Some(UbarProfileKind::Private) => ProfileScope::Private(profile),
        None => return UbarResult::InvalidArgument,
    };
    let background_views = host.background_views.iter()
        .filter_map(|(view, background)| (background.context.profile == scope).then_some(*view))
        .collect::<Vec<_>>();
    for view in background_views { destroy_background_view(&mut host, view); }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.destroy_profile(profile);
    if result != UbarResult::Ok {
        return result;
    }
    host.cdms.remove(&profile);
    host.request_policies.remove(&profile);
    host.extension_executor.drop_profile(scope);
    host.extensions.drop_profile(scope);
    host.profile_data.remove(&profile);
    host.services.remove(&profile);
    if host.core.destroy_profile(profile) {
        UbarResult::Ok
    } else {
        UbarResult::InvalidArgument
    }
}

unsafe extern "C" fn create_view(
    profile: u64,
    config: *const UbarViewConfigV1,
    callbacks: *const UbarCallbacksV1,
    view_out: *mut u64,
) -> UbarResult {
    if config.is_null() || view_out.is_null() {
        return UbarResult::InvalidArgument;
    }
    let config = unsafe { &*config };
    if config.struct_size < std::mem::size_of::<UbarViewConfigV1>() as u32 {
        return UbarResult::AbiMismatch;
    }
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if host.backend.is_none() {
        return UbarResult::EngineFailure;
    }
    let Some(view) = host.core.create_view(profile, "about:blank") else {
        return UbarResult::InvalidArgument;
    };
    host.core.set_visible(view.id, config.initially_visible);
    if !callbacks.is_null()
        && unsafe { (*callbacks).struct_size } < std::mem::size_of::<UbarCallbacksV1>() as u32
    {
        host.core.destroy_view(view.id);
        return UbarResult::AbiMismatch;
    }
    let internal_callbacks = UbarCallbacksV1 {
        struct_size: std::mem::size_of::<UbarCallbacksV1>() as u32,
        user_data: view.id as usize as *mut c_void,
        event: Some(engine_event),
    };
    let result = host.backend.as_ref().unwrap().create_view(view.id, profile, config, &internal_callbacks);
    if result != UbarResult::Ok {
        host.core.destroy_view(view.id);
        return result;
    }
    host.zoom.insert(view.id, 1.0);
    host.views.insert(view.id, StoredView {
        profile,
        config: StoredViewConfig::from_abi(config),
        callbacks: if callbacks.is_null() { None } else { Some(StoredCallbacks::from_abi(unsafe { &*callbacks })) },
        hibernated: false,
    });
    let scope = profile_scope(&host, profile).unwrap();
    host.extension_tabs.lock().unwrap_or_else(|error| error.into_inner()).insert(view.id,
        ProviderTab { profile: scope, uri: "about:blank".into(), active: config.initially_visible });
    unsafe { *view_out = view.id };
    UbarResult::Ok
}

unsafe extern "C" fn destroy_view(view: u64) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(stored) = host.views.get(&view).copied() else { return UbarResult::InvalidArgument };
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    if !stored.hibernated {
        let result = backend.destroy_view(view);
        if result != UbarResult::Ok { return result; }
    }
    if let Some(services) = host.services.get_mut(&stored.profile) {
        let _ = services.control(BrowserControlRequest::RemoveHibernation { view_id: view });
    }
    host.zoom.remove(&view);
    host.pending_injections.remove(&view);
    host.navigation_serials.remove(&view);
    host.extension_executor.destroy_tab(view);
    host.extension_tabs.lock().unwrap_or_else(|error| error.into_inner()).remove(&view);
    host.views.remove(&view);
    if host.core.destroy_view(view) {
        UbarResult::Ok
    } else {
        UbarResult::InvalidArgument
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |value| value.as_millis() as u64)
}

fn hibernate_view_locked(host: &mut HostState, view: u64) -> UbarResult {
    let Some(state) = host.core.view(view).cloned() else { return UbarResult::InvalidArgument };
    let snapshot = HibernationSnapshot {
        view_id: view, uri: state.uri.clone(), title: String::new(), history: vec![state.uri],
        history_index: 0, scroll_x: 0, scroll_y: 0, page_state_base64: String::new(), created_at_ms: now_ms(),
    };
    hibernate_snapshot_locked(host, snapshot)
}

fn hibernate_snapshot_locked(host: &mut HostState, snapshot: HibernationSnapshot) -> UbarResult {
    let view = snapshot.view_id;
    let Some(stored) = host.views.get(&view).copied() else { return UbarResult::InvalidArgument };
    if stored.hibernated { return UbarResult::Ok; }
    let Some(state) = host.core.view(view).cloned() else { return UbarResult::InvalidArgument };
    if state.visible { return UbarResult::PermissionDenied; }
    if snapshot.uri != state.uri { return UbarResult::InvalidArgument; }
    let Some(services) = host.services.get_mut(&stored.profile) else { return UbarResult::EngineFailure };
    if services.control(BrowserControlRequest::Hibernate { snapshot }).is_err() {
        return UbarResult::EngineFailure;
    }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.destroy_view(view);
    if result != UbarResult::Ok {
        let _ = host.services.get_mut(&stored.profile)
            .and_then(|services| services.control(BrowserControlRequest::RemoveHibernation { view_id: view }).ok());
        return result;
    }
    if !host.core.mark_hibernated(view) { return UbarResult::EngineFailure; }
    if let Some(runtime) = host.views.get_mut(&view) { runtime.hibernated = true; }
    UbarResult::Ok
}

fn restore_view_locked(host: &mut HostState, view: u64) -> UbarResult {
    let Some(stored) = host.views.get(&view).copied() else { return UbarResult::InvalidArgument };
    if !stored.hibernated { return UbarResult::Ok; }
    let value = match host.services.get_mut(&stored.profile)
        .and_then(|services| services.control(BrowserControlRequest::RestoreHibernation { view_id: view }).ok())
    {
        Some(value) => value,
        None => return UbarResult::EngineFailure,
    };
    let snapshot: HibernationSnapshot = match serde_json::from_value(value) {
        Ok(value) => value,
        Err(_) => return UbarResult::EngineFailure,
    };
    let config = stored.config.to_abi(false);
    let callbacks = UbarCallbacksV1 {
        struct_size: std::mem::size_of::<UbarCallbacksV1>() as u32,
        user_data: view as usize as *mut c_void,
        event: Some(engine_event),
    };
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.create_view(view, stored.profile, &config, &callbacks);
    if result != UbarResult::Ok {
        let _ = host.services.get_mut(&stored.profile)
            .and_then(|services| services.control(BrowserControlRequest::Hibernate { snapshot }).ok());
        return result;
    }
    let uri = UbarBytes { data: snapshot.uri.as_ptr(), len: snapshot.uri.len() };
    let result = backend.navigate(view, uri);
    if result != UbarResult::Ok {
        let _ = backend.destroy_view(view);
        let _ = host.services.get_mut(&stored.profile)
            .and_then(|services| services.control(BrowserControlRequest::Hibernate { snapshot }).ok());
        return result;
    }
    host.core.navigate(view, snapshot.uri);
    host.core.mark_restored(view);
    if let Some(runtime) = host.views.get_mut(&view) { runtime.hibernated = false; }
    UbarResult::Ok
}

fn hibernate_hidden_under_pressure(host: &mut HostState, profile: u64, resident_bytes: u64) -> usize {
    let Some(limit) = host.core.profile(profile).map(|value| value.memory_ceiling_bytes) else { return 0 };
    if resident_bytes < limit { return 0; }
    let candidates = host.core.hidden_views(profile);
    candidates.into_iter().filter(|view| hibernate_view_locked(host, *view) == UbarResult::Ok).count()
}

#[cfg(target_os = "linux")]
fn resident_tree_bytes() -> Option<u64> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let pid: u32 = entry.file_name().to_string_lossy().parse().ok().unwrap_or(0);
        if pid == 0 { continue; }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        let Some((_, tail)) = stat.rsplit_once(')') else { continue };
        let tail = tail.trim_start();
        let mut fields = tail.split_whitespace();
        if fields.next().is_none() { continue; }
        let Some(parent) = fields.next().and_then(|value| value.parse::<u32>().ok()) else { continue };
        let status = std::fs::read_to_string(entry.path().join("status")).unwrap_or_default();
        let rss_kb = status.lines().find_map(|line| line.strip_prefix("VmRSS:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())).unwrap_or(0);
        rows.push((pid, parent, rss_kb * 1024));
    }
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let memory: BTreeMap<u32, u64> = rows.iter().map(|(pid, _, rss)| (*pid, *rss)).collect();
    for (pid, parent, _) in rows { children.entry(parent).or_default().push(pid); }
    let mut pending = vec![std::process::id()];
    let mut seen = BTreeSet::new();
    while let Some(pid) = pending.pop() {
        if seen.insert(pid) { pending.extend(children.get(&pid).into_iter().flatten().copied()); }
    }
    Some(seen.into_iter().filter_map(|pid| memory.get(&pid)).sum())
}

#[cfg(target_os = "macos")]
fn resident_tree_bytes() -> Option<u64> {
    use std::collections::{BTreeMap, BTreeSet};
    let output = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,rss="]).output().ok()?;
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let rows: Vec<(u32, u32, u64)> = text.lines().filter_map(|line| {
        let mut field = line.split_whitespace();
        Some((field.next()?.parse().ok()?, field.next()?.parse().ok()?, field.next()?.parse::<u64>().ok()? * 1024))
    }).collect();
    let memory: BTreeMap<u32, u64> = rows.iter().map(|(pid, _, rss)| (*pid, *rss)).collect();
    let mut pending = vec![std::process::id()];
    let mut seen = BTreeSet::new();
    while let Some(pid) = pending.pop() {
        if seen.insert(pid) { pending.extend(rows.iter().filter(|(_, parent, _)| *parent == pid).map(|(child, _, _)| *child)); }
    }
    Some(seen.into_iter().filter_map(|pid| memory.get(&pid)).sum())
}

#[cfg(target_os = "windows")]
fn resident_tree_bytes() -> Option<u64> {
    use std::collections::{BTreeMap, BTreeSet};
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_VM_READ: u32 = 0x0010;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    #[repr(C)] struct Counters { cb: u32, faults: u32, peak_working_set: usize, working_set: usize,
        peak_paged_pool: usize, paged_pool: usize, peak_nonpaged_pool: usize, nonpaged_pool: usize,
        pagefile_usage: usize, peak_pagefile_usage: usize }
    #[repr(C)] struct ProcessEntry { size: u32, usage: u32, pid: u32, heap: usize,
        module: u32, threads: u32, parent: u32, priority: i32, flags: u32, executable: [u16; 260] }
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    #[link(name = "Psapi")]
    unsafe extern "system" { fn GetProcessMemoryInfo(process: *mut c_void, counters: *mut Counters, size: u32) -> i32; }
    fn working_set(pid: u32) -> Option<u64> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if process.is_null() { return None; }
        let mut counters = Counters { cb: std::mem::size_of::<Counters>() as u32, faults: 0,
            peak_working_set: 0, working_set: 0, peak_paged_pool: 0, paged_pool: 0,
            peak_nonpaged_pool: 0, nonpaged_pool: 0, pagefile_usage: 0, peak_pagefile_usage: 0 };
        let ok = unsafe { GetProcessMemoryInfo(process, &mut counters, counters.cb) } != 0;
        unsafe { CloseHandle(process) };
        ok.then_some(counters.working_set as u64)
    }
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot as isize == -1 { return None; }
    let mut entry = ProcessEntry { size: std::mem::size_of::<ProcessEntry>() as u32, usage: 0,
        pid: 0, heap: 0, module: 0, threads: 0, parent: 0, priority: 0, flags: 0,
        executable: [0; 260] };
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if unsafe { Process32FirstW(snapshot, &mut entry) } != 0 {
        loop {
            children.entry(entry.parent).or_default().push(entry.pid);
            if unsafe { Process32NextW(snapshot, &mut entry) } == 0 { break; }
        }
    }
    unsafe { CloseHandle(snapshot) };
    let mut pending = vec![std::process::id()];
    let mut seen = BTreeSet::new();
    while let Some(pid) = pending.pop() {
        if seen.insert(pid) { pending.extend(children.get(&pid).into_iter().flatten().copied()); }
    }
    Some(seen.into_iter().filter_map(working_set).sum())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn resident_tree_bytes() -> Option<u64> { None }

unsafe extern "C" fn navigate(view: u64, value: UbarBytes) -> UbarResult {
    let uri = match unsafe { text(value) } {
        Ok(uri) if uri.starts_with("http://") || uri.starts_with("https://") || uri == "about:blank" => uri,
        _ => return UbarResult::InvalidArgument,
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if restore_view_locked(&mut host, view) != UbarResult::Ok { return UbarResult::EngineFailure; }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.navigate(view, value);
    if result != UbarResult::Ok {
        return result;
    }
    if !host.core.navigate(view, uri) {
        return UbarResult::InvalidArgument;
    }
    if let Some(tab) = host.extension_tabs.lock().unwrap_or_else(|error| error.into_inner()).get_mut(&view) {
        tab.uri = uri.to_owned();
    }
    if let Some(profile) = host.core.view(view).map(|state| state.profile_id) {
        let _ = host.services.get_mut(&profile).map(|services| services.control(
            BrowserControlRequest::RecordHistory {
                title: String::new(), url: uri.to_owned(), visited_at_ms: now_ms(), transition: "typed".into(),
            },
        ));
    }
    UbarResult::Ok
}

unsafe extern "C" fn set_visible(view: u64, visible: bool) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if visible {
        let restored = restore_view_locked(&mut host, view);
        if restored != UbarResult::Ok { return restored; }
    }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.set_visible(view, visible);
    if result != UbarResult::Ok { return result; }
    if !host.core.set_visible(view, visible) { return UbarResult::InvalidArgument; }
    {
        let mut tabs = host.extension_tabs.lock().unwrap_or_else(|error| error.into_inner());
        if visible {
            for tab in tabs.values_mut() { if tab.profile == profile_scope(&host, host.views[&view].profile).unwrap() { tab.active = false; } }
        }
        if let Some(tab) = tabs.get_mut(&view) { tab.active = visible; }
    }
    if !visible {
        if let (Some(profile), Some(resident)) = (host.core.view(view).map(|value| value.profile_id), resident_tree_bytes()) {
            hibernate_hidden_under_pressure(&mut host, profile, resident);
        }
    }
    UbarResult::Ok
}

unsafe extern "C" fn set_zoom(view: u64, zoom: f64) -> UbarResult {
    if !(0.5..=5.0).contains(&zoom) {
        return UbarResult::InvalidArgument;
    }
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if !host.zoom.contains_key(&view) {
        return UbarResult::InvalidArgument;
    }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.set_zoom(view, zoom);
    if result != UbarResult::Ok { return result; }
    host.zoom.insert(view, zoom);
    UbarResult::Ok
}

unsafe extern "C" fn suspend(view: u64) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    hibernate_view_locked(&mut host, view)
}

unsafe extern "C" fn resume(view: u64) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let restored = restore_view_locked(&mut host, view);
    if restored != UbarResult::Ok { return restored; }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.resume(view);
    if result != UbarResult::Ok { return result; }
    if host.core.resume(view) && host.core.set_visible(view, true) {
        UbarResult::Ok
    } else {
        UbarResult::InvalidArgument
    }
}

fn backend_view_command(view: u64, command: impl FnOnce(&WebKitBackend, u64) -> UbarResult) -> UbarResult {
    let host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    command(backend, view)
}

unsafe extern "C" fn go_back(view: u64) -> UbarResult {
    backend_view_command(view, WebKitBackend::go_back)
}

unsafe extern "C" fn go_forward(view: u64) -> UbarResult {
    backend_view_command(view, WebKitBackend::go_forward)
}

unsafe extern "C" fn reload(view: u64) -> UbarResult {
    backend_view_command(view, WebKitBackend::reload)
}

unsafe extern "C" fn stop(view: u64) -> UbarResult {
    backend_view_command(view, WebKitBackend::stop)
}

unsafe extern "C" fn set_request_policy_json(profile: u64, value: UbarBytes) -> UbarResult {
    let text = match unsafe { text(value) } {
        Ok(text) => text,
        Err(error) => return error,
    };
    let policy = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value @ serde_json::Value::Array(_)) => value,
        _ => return UbarResult::InvalidArgument,
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if !host.core.has_profile(profile) { return UbarResult::InvalidArgument; }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.set_request_policy_json(profile, value);
    if result == UbarResult::Ok { host.request_policies.insert(profile, policy); }
    result
}

unsafe extern "C" fn register_cdm(
    profile: u64,
    key_system: UbarBytes,
    library_path: UbarBytes,
    manifest_json: UbarBytes,
) -> UbarResult {
    let key_system = match unsafe { text(key_system) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let library_path = match unsafe { text(library_path) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let bundle_json = match unsafe { bytes(manifest_json) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    if profile == 0
        || key_system != "com.widevine.alpha"
        || library_path.is_empty()
    {
        return UbarResult::InvalidArgument;
    }
    let bundle: CdmRegistrationBundle = match serde_json::from_slice(bundle_json) {
        Ok(value) => value,
        Err(_) => return UbarResult::InvalidArgument,
    };
    let Some(public_key) = option_env!("UBAR_WIDEVINE_AUTH_PUBLIC_KEY_BASE64") else {
        return UbarResult::PermissionDenied;
    };
    let verifier = match Ed25519AuthorizationVerifier::from_base64(public_key) {
        Ok(value) => value,
        Err(_) => return UbarResult::EngineFailure,
    };
    let manifest = match serde_json::to_vec(&bundle.manifest) {
        Ok(value) => value,
        Err(_) => return UbarResult::InvalidArgument,
    };
    let authorization = match serde_json::to_vec(&bundle.authorization) {
        Ok(value) => value,
        Err(_) => return UbarResult::InvalidArgument,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64);
    let library_path = match PathBuf::from(library_path).canonicalize() {
        Ok(value) => value,
        Err(_) => return UbarResult::InvalidArgument,
    };
    let cdm = match AuthorizedWidevine::open(
        &library_path,
        &manifest,
        &authorization,
        now,
        &verifier,
    ) {
        Ok(value) => value,
        Err(_) => return UbarResult::PermissionDenied,
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if !host.core.has_profile(profile) {
        return UbarResult::InvalidArgument;
    }
    let executable = match std::env::current_exe() {
        Ok(value) => value,
        Err(_) => return UbarResult::EngineFailure,
    };
    let Some(directory) = executable.parent() else { return UbarResult::EngineFailure };
    let worker_name = if cfg!(target_os = "windows") {
        "ubar-cdm-worker.exe"
    } else {
        "ubar-cdm-worker"
    };
    let adapter_name = if cfg!(target_os = "windows") {
        "ubar_widevine_adapter.dll"
    } else if cfg!(target_os = "macos") {
        "libubar_widevine_adapter.dylib"
    } else {
        "libubar_widevine_adapter.so"
    };
    let worker = directory.join(worker_name);
    let mut adapter = std::env::var_os("UBAR_WIDEVINE_ADAPTER_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| directory.join(adapter_name));
    if cfg!(target_os = "macos") && !adapter.is_file() {
        if let Some(contents) = directory.parent() {
            adapter = contents.join("Frameworks").join(adapter_name);
        }
    }
    let worker = match worker.canonicalize() {
        Ok(value) => value,
        Err(_) => return UbarResult::EngineFailure,
    };
    let adapter = match adapter.canonicalize() {
        Ok(value) => value,
        Err(_) => return UbarResult::PermissionDenied,
    };
    let broker = match CdmBroker::launch(
        CdmLaunchSpec {
            helper_executable: worker,
            adapter_library: adapter,
            component: cdm,
            host_version: 1,
        },
        &NativeSandboxLauncher,
    ) {
        Ok(value) => value,
        Err(_) => return UbarResult::EngineFailure,
    };
    host.cdms.insert(profile, broker);
    UbarResult::Ok
}

unsafe extern "C" fn free_bytes(value: UbarOwnedBytes) {
    if !value.data.is_null() && value.capacity >= value.len {
        drop(unsafe { Vec::from_raw_parts(value.data, value.len, value.capacity) });
    }
}

fn profile_scope(host: &HostState, profile: u64) -> Option<ProfileScope> {
    match host.core.profile(profile)?.kind {
        UbarProfileKind::Normal => Some(ProfileScope::Normal),
        UbarProfileKind::Private => Some(ProfileScope::Private(profile)),
    }
}

fn default_profile_directory() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            .map(|path| path.join("uBar").join("Default"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(PathBuf::from)
            .map(|path| path.join("Library").join("Application Support").join("uBar").join("Default"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
            std::env::var_os("HOME").map(PathBuf::from)
                .map(|path| path.join(".local").join("share"))
        }).map(|path| path.join("ubar").join("default"))
    }
}

fn runtime_sibling(name: &str) -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|directory| directory.join(name))
}

fn extension_verifiers() -> Option<(PathBuf, PathBuf)> {
    Some((
        runtime_sibling(if cfg!(target_os = "windows") {
            "ubar-xpi-verifier.exe"
        } else {
            "ubar-xpi-verifier"
        })?,
        runtime_sibling(if cfg!(target_os = "windows") {
            "ubar-crx3-verifier.exe"
        } else {
            "ubar-crx3-verifier"
        })?,
    ))
}

unsafe extern "C" fn extension_control_json(
    profile: u64,
    request_json: UbarBytes,
    response_out: *mut UbarOwnedBytes,
) -> UbarResult {
    if response_out.is_null() { return UbarResult::InvalidArgument; }
    unsafe { *response_out = UbarOwnedBytes::default() };
    let request = match unsafe { bytes(request_json) }
        .ok()
        .and_then(|value| serde_json::from_slice::<ExtensionControlRequest>(value).ok())
    {
        Some(value) => value,
        None => return UbarResult::InvalidArgument,
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(scope) = profile_scope(&host, profile) else { return UbarResult::InvalidArgument };
    let result: Result<serde_json::Value, String> = match request {
        ExtensionControlRequest::List => serde_json::to_value(host.extensions.list(scope))
            .map_err(|error| error.to_string()),
        ExtensionControlRequest::LoadUnpacked { directory } => {
            let result = host.extensions
                .load_unpacked(scope, &directory, cfg!(feature = "developer-extensions"));
            if result.is_ok() { sync_runtime_extensions(&mut host, scope); }
            result.and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
        }
        ExtensionControlRequest::InstallPackage { path } => (|| {
            let data = host.profile_data.get(&profile).cloned()
                .ok_or("normal profile has no data directory")?;
            let (xpi_verifier, crx3_verifier) = extension_verifiers()
                .ok_or("engine executable has no parent")?;
            let installed = host.extensions.install_package(
                scope, &path, &xpi_verifier, &crx3_verifier, &data.join("extensions"),
            )?;
            sync_runtime_extensions(&mut host, scope);
            serde_json::to_value(installed).map_err(|error| error.to_string())
        })(),
        ExtensionControlRequest::GetAction { id } => host.extensions.action(scope, &id),
        ExtensionControlRequest::Remove { id } => {
            let removed = host.extensions.remove(scope, &id);
            if removed {
                destroy_extension_backgrounds(&mut host, &id, scope);
                host.extension_executor.uninstall(&id, scope);
            }
            Ok(serde_json::json!({"removed": removed}))
        }
        ExtensionControlRequest::DispatchWorldApi { world, capability, request_id,
            namespace, member, arguments, user_gesture } => {
            if world.profile != scope { Err("extension world belongs to another profile".into()) }
            else {
                let context = ContextId {
                    extension: world.extension.clone(), profile: world.profile, serial: world.serial,
                    kind: ContextKind::ContentScript { tab_id: world.tab_id, frame_id: world.frame_id },
                };
                serde_json::to_value(host.extension_executor.dispatch_world_api_encoded(
                    &world, &capability, ApiRequest {
                        request_id, context, namespace, member, arguments, user_gesture,
                    },
                )).map_err(|error| error.to_string())
            }
        }
    };
    let Ok(value) = result else { return UbarResult::PermissionDenied };
    let Ok(mut encoded) = serde_json::to_vec(&value) else { return UbarResult::EngineFailure };
    let output = UbarOwnedBytes {
        data: encoded.as_mut_ptr(), len: encoded.len(), capacity: encoded.capacity(),
    };
    std::mem::forget(encoded);
    unsafe { *response_out = output };
    UbarResult::Ok
}

unsafe extern "C" fn browser_control_json(
    profile: u64,
    request_json: UbarBytes,
    response_out: *mut UbarOwnedBytes,
) -> UbarResult {
    if response_out.is_null() || request_json.len > 1024 * 1024 { return UbarResult::InvalidArgument; }
    unsafe { *response_out = UbarOwnedBytes::default() };
    let request = match unsafe { bytes(request_json) }
        .ok().and_then(|value| serde_json::from_slice::<BrowserControlRequest>(value).ok())
    {
        Some(value) => value,
        None => return UbarResult::InvalidArgument,
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if !host.core.has_profile(profile) { return UbarResult::InvalidArgument; }
    let result: Result<serde_json::Value, String> = match request {
        BrowserControlRequest::Hibernate { snapshot } => {
            if host.core.view(snapshot.view_id).is_none_or(|view| view.profile_id != profile) {
                Err("view does not belong to profile".into())
            } else {
                match hibernate_snapshot_locked(&mut host, snapshot) {
                    UbarResult::Ok => Ok(serde_json::json!({"stored": true})),
                    _ => Err("view cannot be hibernated".into()),
                }
            }
        }
        BrowserControlRequest::RestoreHibernation { view_id } => {
            if host.core.view(view_id).is_none_or(|view| view.profile_id != profile) {
                Err("view does not belong to profile".into())
            } else {
                match restore_view_locked(&mut host, view_id) {
                    UbarResult::Ok => Ok(serde_json::json!({"restored": true})),
                    _ => Err("view cannot be restored".into()),
                }
            }
        }
        BrowserControlRequest::ReportMemory { resident_bytes } => {
            let observed = resident_tree_bytes().map_or(resident_bytes, |measured| measured.max(resident_bytes));
            let count = hibernate_hidden_under_pressure(&mut host, profile, observed);
            Ok(serde_json::json!({"residentBytes": observed, "hibernated": count}))
        }
        request => match host.services.get_mut(&profile) {
            Some(services) => services.control(request),
            None => Err("profile services unavailable".into()),
        },
    };
    let Ok(value) = result else { return UbarResult::PermissionDenied };
    let Ok(mut encoded) = serde_json::to_vec(&value) else { return UbarResult::EngineFailure };
    let output = UbarOwnedBytes { data: encoded.as_mut_ptr(), len: encoded.len(), capacity: encoded.capacity() };
    std::mem::forget(encoded);
    unsafe { *response_out = output };
    UbarResult::Ok
}

static ENGINE_NAME: &CStr = c"uBar shared engine host";

static API: UbarEngineApiV1 = UbarEngineApiV1 {
    abi_version: UBAR_ENGINE_ABI_V1,
    struct_size: std::mem::size_of::<UbarEngineApiV1>() as u32,
    engine_name: ENGINE_NAME.as_ptr(),
    create_profile,
    destroy_profile,
    create_view,
    destroy_view,
    navigate,
    set_visible,
    set_zoom,
    suspend,
    resume,
    set_request_policy_json,
    register_cdm,
    free_bytes,
    go_back,
    go_forward,
    reload,
    stop,
    extension_control_json,
    browser_control_json,
};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ubar_get_engine_api(
    requested_abi: u32,
    api_out: *mut *const UbarEngineApiV1,
) -> UbarResult {
    if api_out.is_null() {
        return UbarResult::InvalidArgument;
    }
    if requested_abi != UBAR_ENGINE_ABI_V1 {
        return UbarResult::AbiMismatch;
    }
    unsafe { *api_out = &API };
    UbarResult::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_v1_and_rejects_unsandboxed_profiles() {
        let mut api = std::ptr::null();
        assert_eq!(unsafe { ubar_get_engine_api(1, &mut api) }, UbarResult::Ok);
        assert!(!api.is_null());
        let mut config = UbarProfileConfigV1::normal();
        config.require_sandbox = false;
        let mut profile = 0;
        assert_eq!(
            unsafe { ((*api).create_profile)(&config, &mut profile) },
            UbarResult::PermissionDenied
        );
    }

    #[test]
    fn private_profile_rejects_persistent_paths() {
        let data = b"profile";
        let mut config = UbarProfileConfigV1::private();
        config.data_directory_utf8 = UbarBytes {
            data: data.as_ptr(),
            len: data.len(),
        };
        let mut profile = 0;
        assert_eq!(
            unsafe { create_profile(&config, &mut profile) },
            UbarResult::PermissionDenied
        );
    }
}
