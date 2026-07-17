use crate::runtime::{
    ContextId, ExtensionId, MessageBus, ProfileScope, StorageArea, StorageService,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiRequest {
    pub request_id: u64,
    pub context: ContextId,
    pub namespace: String,
    pub member: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub user_gesture: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiResponse {
    pub request_id: u64,
    pub result: Result<Value, ApiError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    fn invalid(message: impl Into<String>) -> Self {
        Self { code: "INVALID_ARGUMENT".into(), message: message.into() }
    }

    fn denied(message: impl Into<String>) -> Self {
        Self { code: "PERMISSION_DENIED".into(), message: message.into() }
    }

    fn unsupported(namespace: &str, member: &str) -> Self {
        Self {
            code: "UNSUPPORTED_API".into(),
            message: format!("{namespace}.{member} is not implemented by this build"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct InstalledExtension {
    pub id: ExtensionId,
    pub profile: ProfileScope,
    pub base_url: String,
    pub manifest: Value,
    pub permissions: BTreeSet<String>,
}

pub trait BrowserProvider {
    fn tabs_query(&self, profile: ProfileScope, query: &Value) -> Result<Value, String>;
    fn tabs_create(&mut self, profile: ProfileScope, properties: &Value) -> Result<Value, String>;
    fn tabs_update(&mut self, profile: ProfileScope, tab_id: u64, properties: &Value) -> Result<Value, String>;
    fn tabs_remove(&mut self, profile: ProfileScope, tab_ids: &[u64]) -> Result<(), String>;

    fn api_call(
        &mut self,
        _profile: ProfileScope,
        namespace: &str,
        member: &str,
        _arguments: &Value,
    ) -> Result<Value, ProviderError> {
        Err(ProviderError::Unsupported(format!(
            "{namespace}.{member} has no browser provider"
        )))
    }
}

#[derive(Clone, Debug)]
pub enum ProviderError {
    Unsupported(String),
    Invalid(String),
    Denied(String),
    Failed(String),
}

impl ProviderError {
    fn into_api(self) -> ApiError {
        match self {
            Self::Unsupported(message) => ApiError { code: "UNSUPPORTED_API".into(), message },
            Self::Invalid(message) => ApiError::invalid(message),
            Self::Denied(message) => ApiError::denied(message),
            Self::Failed(message) => ApiError { code: "BROWSER_ERROR".into(), message },
        }
    }
}

pub struct ExtensionDispatcher<B> {
    pub browser: B,
    pub messages: MessageBus,
    pub storage: StorageService,
    extensions: BTreeMap<(ExtensionId, ProfileScope), InstalledExtension>,
}

impl<B: BrowserProvider> ExtensionDispatcher<B> {
    pub fn new(browser: B) -> Self {
        Self {
            browser,
            messages: MessageBus::default(),
            storage: StorageService::default(),
            extensions: BTreeMap::new(),
        }
    }

    pub fn install(&mut self, extension: InstalledExtension) {
        self.extensions.insert((extension.id.clone(), extension.profile), extension);
    }

    pub fn uninstall(&mut self, id: &ExtensionId, profile: ProfileScope) {
        self.extensions.remove(&(id.clone(), profile));
    }

    pub fn dispatch(&mut self, request: ApiRequest) -> ApiResponse {
        let result = self.dispatch_inner(&request);
        ApiResponse { request_id: request.request_id, result }
    }

    fn dispatch_inner(&mut self, request: &ApiRequest) -> Result<Value, ApiError> {
        let extension = self
            .extensions
            .get(&(request.context.extension.clone(), request.context.profile))
            .cloned()
            .ok_or_else(|| ApiError::denied("extension is not installed"))?;
        match (request.namespace.as_str(), request.member.as_str()) {
            ("runtime", "getManifest") => Ok(extension.manifest),
            ("runtime", "getURL") => {
                let path = request.arguments.get(0).and_then(Value::as_str).unwrap_or_default();
                Ok(Value::String(format!(
                    "{}{}",
                    extension.base_url,
                    path.trim_start_matches('/')
                )))
            }
            ("runtime", "sendMessage") => {
                let recipient = request
                    .arguments
                    .get("extensionId")
                    .and_then(Value::as_str)
                    .map(|value| ExtensionId(value.into()))
                    .unwrap_or_else(|| extension.id.clone());
                if recipient != extension.id {
                    let target = self.extensions.get(&(recipient.clone(), request.context.profile))
                        .ok_or_else(|| ApiError::invalid("receiving extension is not installed"))?;
                    if !accepts_external_message(&target.manifest, &extension.id) {
                        return Err(ApiError::denied("receiving extension does not accept this sender"));
                    }
                }
                let payload = request.arguments.get("message").cloned().unwrap_or(Value::Null);
                let id = self
                    .messages
                    .send_message(&request.context, recipient, payload)
                    .map_err(ApiError::invalid)?;
                Ok(json!({"requestId": id}))
            }
            ("runtime", "connect") => {
                let recipient = request.arguments.get("extensionId").and_then(Value::as_str)
                    .map(|value| ExtensionId(value.into())).unwrap_or_else(|| extension.id.clone());
                if recipient != extension.id {
                    let target = self.extensions.get(&(recipient.clone(), request.context.profile))
                        .ok_or_else(|| ApiError::invalid("receiving extension is not installed"))?;
                    if !accepts_external_message(&target.manifest, &extension.id) {
                        return Err(ApiError::denied("receiving extension does not accept this sender"));
                    }
                }
                let name = request.arguments.get("name").and_then(Value::as_str).unwrap_or_default();
                let port = self.messages.connect_extension(&request.context, recipient, name)
                    .map_err(ApiError::invalid)?;
                Ok(json!({"portId": port}))
            }
            ("runtime", "getPlatformInfo") => Ok(json!({
                "os": platform_os(),
                "arch": platform_arch(),
                "nacl_arch": platform_arch(),
            })),
            ("runtime", "getBrowserInfo") => Ok(json!({
                "name": "uBar",
                "vendor": "uBar",
                "version": env!("CARGO_PKG_VERSION"),
                "buildID": option_env!("UBAR_BUILD_ID").unwrap_or("development"),
            })),
            ("runtime", "getFrameId") => Ok(json!(match &request.context.kind {
                crate::runtime::ContextKind::ContentScript { frame_id, .. } => *frame_id,
                _ => 0,
            })),
            ("permissions", "contains") | ("permissions", "getAll")
            | ("permissions", "request") | ("permissions", "remove") => {
                self.dispatch_permissions(request)
            }
            ("storage", "local") | ("storage", "sync") | ("storage", "session") => {
                self.dispatch_storage(request, &extension)
            }
            ("tabs", "query") => {
                require_permission(&extension, "tabs")?;
                self.browser
                    .tabs_query(request.context.profile, &request.arguments)
                    .map_err(ApiError::invalid)
            }
            ("tabs", "create") => {
                require_permission(&extension, "tabs")?;
                self.browser
                    .tabs_create(request.context.profile, &request.arguments)
                    .map_err(ApiError::invalid)
            }
            ("tabs", "update") => {
                require_permission(&extension, "tabs")?;
                let tab_id = request
                    .arguments
                    .get("tabId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ApiError::invalid("tabs.update requires tabId"))?;
                self.browser
                    .tabs_update(request.context.profile, tab_id, &request.arguments)
                    .map_err(ApiError::invalid)
            }
            ("tabs", "remove") => {
                require_permission(&extension, "tabs")?;
                let ids = request
                    .arguments
                    .get("tabIds")
                    .and_then(Value::as_array)
                    .ok_or_else(|| ApiError::invalid("tabs.remove requires tabIds"))?
                    .iter()
                    .filter_map(Value::as_u64)
                    .collect::<Vec<_>>();
                self.browser
                    .tabs_remove(request.context.profile, &ids)
                    .map_err(ApiError::invalid)?;
                Ok(Value::Null)
            }
            ("tabs", "sendMessage") => {
                let tab_id = request.arguments.get("tabId").and_then(Value::as_u64)
                    .ok_or_else(|| ApiError::invalid("tabs.sendMessage requires tabId"))?;
                let frame_id = request.arguments.get("frameId").and_then(Value::as_u64);
                let payload = request.arguments.get("message").cloned().unwrap_or(Value::Null);
                let id = self.messages.send_tab_message(&request.context, tab_id, frame_id, payload)
                    .map_err(ApiError::invalid)?;
                Ok(json!({"requestId": id}))
            }
            (namespace, member) if provider_member(namespace, member) => {
                if let Some(permission) = provider_permission(namespace) {
                    require_permission(&extension, permission)?;
                }
                self.browser.api_call(
                    request.context.profile, namespace, member, &request.arguments,
                ).map_err(ProviderError::into_api)
            }
            _ => Err(ApiError::unsupported(&request.namespace, &request.member)),
        }
    }

    fn dispatch_storage(
        &mut self,
        request: &ApiRequest,
        extension: &InstalledExtension,
    ) -> Result<Value, ApiError> {
        require_permission(extension, "storage")?;
        let area = match request.member.as_str() {
            "local" => StorageArea::Local,
            "sync" => StorageArea::Sync,
            "session" => StorageArea::Session,
            _ => return Err(ApiError::unsupported("storage", &request.member)),
        };
        if matches!(request.context.profile, ProfileScope::Private(_)) && area == StorageArea::Sync {
            return Err(ApiError::denied("storage.sync is disabled in private profiles"));
        }
        let operation = request
            .arguments
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("storage request has no operation"))?;
        match operation {
            "get" => {
                let keys = request.arguments.get("keys").and_then(Value::as_array).map(|keys| {
                    keys.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()
                });
                serde_json::to_value(self.storage.get(
                    &extension.id,
                    request.context.profile,
                    area,
                    keys.as_deref(),
                ))
                .map_err(|error| ApiError::invalid(error.to_string()))
            }
            "set" => {
                let values: BTreeMap<String, Value> = serde_json::from_value(
                    request.arguments.get("items").cloned().unwrap_or_else(|| json!({})),
                )
                .map_err(|error| ApiError::invalid(error.to_string()))?;
                self.storage.set(&extension.id, request.context.profile, area, values);
                Ok(Value::Null)
            }
            "remove" => {
                let keys = request
                    .arguments
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| ApiError::invalid("storage.remove requires keys"))?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                self.storage.remove(&extension.id, request.context.profile, area, &keys);
                Ok(Value::Null)
            }
            "clear" => {
                self.storage.clear(&extension.id, request.context.profile, area);
                Ok(Value::Null)
            }
            _ => Err(ApiError::invalid(format!("unknown storage operation: {operation}"))),
        }
    }

    fn dispatch_permissions(&mut self, request: &ApiRequest) -> Result<Value, ApiError> {
        let extension = self.extensions.get_mut(&(
            request.context.extension.clone(), request.context.profile,
        ))
            .ok_or_else(|| ApiError::denied("extension is not installed"))?;
        let required = manifest_permissions(&extension.manifest, "permissions");
        let optional = manifest_permissions(&extension.manifest, "optional_permissions");
        match request.member.as_str() {
            "getAll" => {
                let (permissions, origins): (Vec<_>, Vec<_>) = extension.permissions.iter()
                    .cloned().partition(|permission| !is_origin_permission(permission));
                Ok(json!({"permissions": permissions, "origins": origins}))
            }
            "contains" => {
                let requested = requested_permissions(&request.arguments);
                Ok(json!(requested.iter().all(|item| extension.permissions.contains(item))))
            }
            "request" => {
                if !request.user_gesture {
                    return Err(ApiError::denied("permissions.request requires a user gesture"));
                }
                let requested = requested_permissions(&request.arguments);
                let optional_origins = manifest_permissions(&extension.manifest, "optional_host_permissions");
                if !requested.iter().all(|item| required.contains(item) || optional.contains(item)
                    || optional_origins.contains(item))
                {
                    return Err(ApiError::denied("permission was not declared in the manifest"));
                }
                extension.permissions.extend(requested);
                Ok(json!(true))
            }
            "remove" => {
                let requested = requested_permissions(&request.arguments);
                if requested.iter().any(|item| required.contains(item)) {
                    return Err(ApiError::denied("required permissions cannot be removed"));
                }
                for item in requested { extension.permissions.remove(&item); }
                Ok(json!(true))
            }
            _ => Err(ApiError::unsupported("permissions", &request.member)),
        }
    }
}

fn require_permission(extension: &InstalledExtension, permission: &str) -> Result<(), ApiError> {
    if extension.permissions.contains(permission) {
        Ok(())
    } else {
        Err(ApiError::denied(format!("missing {permission} permission")))
    }
}

fn requested_permissions(arguments: &Value) -> BTreeSet<String> {
    ["permissions", "origins"].into_iter()
        .filter_map(|field| arguments.get(field).and_then(Value::as_array))
        .flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn manifest_permissions(manifest: &Value, field: &str) -> BTreeSet<String> {
    manifest.get(field).and_then(Value::as_array).into_iter().flatten()
        .filter_map(Value::as_str).map(str::to_string).collect()
}

fn is_origin_permission(value: &str) -> bool {
    value == "<all_urls>" || value.contains("://")
}

fn platform_os() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win",
        "macos" => "mac",
        "linux" => "linux",
        "android" => "android",
        _ => "unknown",
    }
}

fn platform_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86-64",
        "aarch64" => "arm64",
        "x86" => "x86-32",
        "arm" => "arm",
        _ => "unknown",
    }
}

fn provider_permission(namespace: &str) -> Option<&'static str> {
    match namespace {
        "bookmarks" | "history" | "cookies" | "downloads" | "sessions"
        | "browsingData" | "webNavigation" | "notifications" | "management"
        | "contextualIdentities" | "declarativeNetRequest" | "webRequest" | "scripting" => Some(namespace),
        _ => None,
    }
}

fn provider_member(namespace: &str, member: &str) -> bool {
    match namespace {
        "windows" => matches!(member, "get" | "getCurrent" | "getLastFocused" | "getAll" | "create" | "update" | "remove"),
        "bookmarks" => matches!(member, "get" | "getChildren" | "getRecent" | "getSubTree" | "getTree" | "search" | "create" | "move" | "update" | "remove" | "removeTree"),
        "history" => matches!(member, "search" | "getVisits" | "addUrl" | "deleteUrl" | "deleteRange" | "deleteAll"),
        "cookies" => matches!(member, "get" | "getAll" | "set" | "remove" | "getAllCookieStores"),
        "downloads" => matches!(member, "download" | "search" | "pause" | "resume" | "cancel" | "getFileIcon" | "open" | "show" | "showDefaultFolder" | "erase" | "removeFile" | "acceptDanger"),
        "sessions" => matches!(member, "getRecentlyClosed" | "restore" | "getDevices" | "setTabValue" | "getTabValue" | "removeTabValue" | "setWindowValue" | "getWindowValue" | "removeWindowValue"),
        "browsingData" => matches!(member, "settings" | "remove" | "removeCache" | "removeCookies" | "removeDownloads" | "removeFormData" | "removeHistory" | "removeLocalStorage" | "removePasswords" | "removePluginData" | "removeServiceWorkers"),
        "webNavigation" => matches!(member, "getFrame" | "getAllFrames"),
        "notifications" => matches!(member, "create" | "update" | "clear" | "getAll"),
        "commands" => matches!(member, "getAll" | "reset" | "update"),
        "management" => matches!(member, "getAll" | "get" | "getSelf" | "install" | "uninstallSelf" | "setEnabled"),
        "contextualIdentities" => matches!(member, "get" | "query" | "create" | "update" | "remove"),
        "declarativeNetRequest" => matches!(member, "updateDynamicRules" | "getDynamicRules" | "updateSessionRules" | "getSessionRules" | "updateEnabledRulesets" | "getEnabledRulesets" | "isRegexSupported"),
        "webRequest" => member == "handlerBehaviorChanged",
        "topSites" => member == "get",
        "search" => matches!(member, "get" | "search"),
        "idle" => matches!(member, "queryState" | "setDetectionInterval"),
        "scripting" => matches!(member, "executeScript" | "insertCSS" | "removeCSS" | "registerContentScripts" | "unregisterContentScripts" | "getRegisteredContentScripts" | "updateContentScripts"),
        _ => false,
    }
}

fn accepts_external_message(manifest: &Value, sender: &ExtensionId) -> bool {
    manifest.pointer("/externally_connectable/ids").and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().filter_map(Value::as_str)
            .any(|id| id == "*" || id == sender.0))
}
