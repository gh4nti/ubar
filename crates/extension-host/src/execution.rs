use crate::dispatcher::{ApiRequest, ApiResponse, BrowserProvider, ExtensionDispatcher, InstalledExtension};
use crate::lifecycle::{BackgroundKind, ExtensionEvent, LifecycleAction, LifecycleManager};
use crate::manifest::{BackgroundSpec, NormalizedManifest};
use crate::permissions::PermissionStore;
use crate::runtime::{ContextId, ContextKind, ExtensionId, ProfileScope, RuntimeMessage, RuntimeReply};
use crate::worlds::{ContentWorldManager, NavigationInjectionPlan, WorldHandle, WorldId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;
use base64::Engine;

#[derive(Clone, Debug)]
pub enum ExecutionAction {
    StartBackground {
        context: ContextId,
        source: BackgroundSpec,
    },
    StopBackground {
        context: ContextId,
    },
}

/// Shared extension execution state. Native WebKit ports execute the returned
/// scripts/background sources and route their private bridge calls back here.
pub struct ExtensionExecutor<B> {
    pub dispatcher: ExtensionDispatcher<B>,
    pub permissions: PermissionStore,
    pub worlds: ContentWorldManager,
    pub lifecycle: LifecycleManager,
    manifests: BTreeMap<(ExtensionId, ProfileScope), NormalizedManifest>,
    contexts: BTreeSet<ContextId>,
    backgrounds: BTreeMap<(ExtensionId, ProfileScope), ContextId>,
    actions: VecDeque<ExecutionAction>,
    next_context: u64,
}

impl<B: BrowserProvider> ExtensionExecutor<B> {
    pub fn new(browser: B) -> Self {
        Self {
            dispatcher: ExtensionDispatcher::new(browser),
            permissions: PermissionStore::default(),
            worlds: ContentWorldManager::default(),
            lifecycle: LifecycleManager::default(),
            manifests: BTreeMap::new(),
            contexts: BTreeSet::new(),
            backgrounds: BTreeMap::new(),
            actions: VecDeque::new(),
            next_context: 0,
        }
    }

    pub fn is_installed(&self, extension: &ExtensionId, profile: ProfileScope) -> bool {
        self.manifests.contains_key(&(extension.clone(), profile))
    }

    pub fn install(
        &mut self,
        extension: ExtensionId,
        profile: ProfileScope,
        base_url: String,
        manifest: NormalizedManifest,
    ) -> Result<Vec<ExecutionAction>, String> {
        if manifest.extension_id.as_deref().is_some_and(|id| id != extension.0.as_str()) {
            return Err("verified extension ID differs from manifest ID".into());
        }
        self.permissions.install_manifest(extension.clone(), profile, &manifest)?;
        if let Err(error) = self.worlds.register_manifest(
            extension.clone(), profile, &base_url, &manifest,
        ) {
            self.permissions.uninstall(&extension, profile);
            return Err(error);
        }
        self.dispatcher.install(InstalledExtension {
            id: extension.clone(), profile, base_url, manifest: manifest.raw.clone(),
            permissions: manifest.permissions.iter().chain(&manifest.host_permissions).cloned().collect(),
        });
        let background = manifest.background.clone();
        self.manifests.insert((extension.clone(), profile), manifest);
        let Some(background) = background else { return Ok(Vec::new()) };
        self.lifecycle.register(extension.clone(), profile, background.kind);
        if background.kind == BackgroundKind::ManifestV2Page && background.persistent {
            let action = self.lifecycle.dispatch(&extension, profile, ExtensionEvent {
                namespace: "runtime".into(), name: "onStartup".into(),
                arguments: serde_json::Value::Array(Vec::new()),
            })?;
            return action.map(|action| self.materialize(action)).transpose()
                .map(|action| action.into_iter().collect());
        }
        Ok(Vec::new())
    }

    pub fn uninstall(&mut self, extension: &ExtensionId, profile: ProfileScope) {
        if let Some(context) = self.backgrounds.remove(&(extension.clone(), profile)) {
            self.unregister_context(&context);
        }
        self.manifests.remove(&(extension.clone(), profile));
        self.permissions.uninstall(extension, profile);
        self.worlds.uninstall(extension, profile);
        self.lifecycle.set_enabled(extension, profile, false);
        self.dispatcher.uninstall(extension, profile);
    }

    pub fn plan_navigation(
        &mut self,
        profile: ProfileScope,
        tab_id: u64,
        frame_id: u64,
        navigation_id: u64,
        url: &str,
        inherited_origin_url: Option<&str>,
    ) -> Result<Vec<NavigationInjectionPlan>, String> {
        let stale = self.contexts.iter().filter(|context| {
            matches!(context.kind, ContextKind::ContentScript { tab_id: id, frame_id: frame }
                if id == tab_id && frame == frame_id)
        }).cloned().collect::<Vec<_>>();
        for context in stale { self.unregister_context(&context); }
        self.worlds.plan_navigation(
            profile, tab_id, frame_id, navigation_id, url,
            inherited_origin_url,
        )
    }

    pub fn activate_world(&mut self, world: &WorldHandle) {
        self.register_context(world.context.clone());
    }

    pub fn open_context(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
        kind: ContextKind,
    ) -> Result<ContextId, String> {
        if !self.manifests.contains_key(&(extension.clone(), profile)) {
            return Err("extension is not installed in this profile".into());
        }
        let context = self.context(extension.clone(), profile, kind);
        self.register_context(context.clone());
        Ok(context)
    }

    pub fn close_context(&mut self, context: &ContextId) { self.unregister_context(context); }

    pub fn dispatch_api(&mut self, request: ApiRequest) -> ApiResponse {
        if !self.contexts.contains(&request.context) {
            return ApiResponse {
                request_id: request.request_id,
                result: Err(crate::dispatcher::ApiError {
                    code: "INVALID_CONTEXT".into(),
                    message: "extension context is not active".into(),
                }),
            };
        }
        let permission_change = (request.namespace == "permissions"
            && matches!(request.member.as_str(), "request" | "remove"))
            .then(|| (request.context.extension.clone(), request.context.profile,
                request.member.clone(), requested_permissions(&request.arguments)));
        let wake = if request.namespace == "runtime"
            && matches!(request.member.as_str(), "sendMessage" | "connect")
        {
            Some((request.arguments.get("extensionId").and_then(serde_json::Value::as_str)
                .map(|id| ExtensionId(id.into())).unwrap_or_else(|| request.context.extension.clone()),
                if request.member == "connect" { "onConnect" } else { "onMessage" }))
        } else { None };
        let profile = request.context.profile;
        if let Some((target, _)) = &wake {
            let live = self.dispatcher.messages.context_for_extension(
                target, profile, Some(&request.context),
            ).is_some();
            let has_background = self.manifests.get(&(target.clone(), profile))
                .and_then(|manifest| manifest.background.as_ref()).is_some();
            if !live && !has_background {
                return ApiResponse {
                    request_id: request.request_id,
                    result: Err(crate::dispatcher::ApiError {
                        code: "NO_RECEIVER".into(), message: "receiving extension has no listener context".into(),
                    }),
                };
            }
        }
        let response = self.dispatcher.dispatch(request);
        if response.result.is_ok() {
            if let Some((extension, profile, operation, permissions)) = permission_change {
                for permission in permissions {
                    let origin = permission == "<all_urls>" || permission.contains("://");
                    if operation == "request" {
                        let already_granted = self.permissions.get(&extension, profile)
                            .is_some_and(|grant| if origin { grant.contains_origin(&permission) } else { grant.allows_api(&permission) });
                        if !already_granted {
                            let _ = if origin {
                                self.permissions.grant_optional_origin(&extension, profile, &permission)
                            } else {
                                self.permissions.grant_optional_api(&extension, profile, permission.clone())
                            };
                        }
                    } else if origin {
                        self.permissions.revoke_optional_origin(&extension, profile, &permission);
                    } else {
                        self.permissions.revoke_optional_api(&extension, profile, &permission);
                    }
                }
            }
            if let Some((extension, event_name)) = wake {
                let event = ExtensionEvent {
                    namespace: "runtime".into(), name: event_name.into(),
                    arguments: serde_json::Value::Array(Vec::new()),
                };
                if let Ok(Some(action)) = self.lifecycle.dispatch(&extension, profile, event) {
                    if let Ok(action) = self.materialize(action) { self.actions.push_back(action); }
                }
            }
        }
        response
    }

    pub fn dispatch_world_api(
        &mut self,
        world: &WorldId,
        capability: &[u8],
        mut request: ApiRequest,
    ) -> ApiResponse {
        let Some(context) = self.worlds.authenticate(world, capability)
            .map(|handle| handle.context.clone()) else {
            return ApiResponse {
                request_id: request.request_id,
                result: Err(crate::dispatcher::ApiError {
                    code: "INVALID_CONTEXT".into(), message: "isolated-world authentication failed".into(),
                }),
            };
        };
        request.context = context;
        self.dispatch_api(request)
    }

    pub fn dispatch_world_api_encoded(
        &mut self,
        world: &WorldId,
        capability: &str,
        request: ApiRequest,
    ) -> ApiResponse {
        let capability = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(capability).unwrap_or_default();
        self.dispatch_world_api(world, &capability, request)
    }

    pub fn next_action(&mut self) -> Option<ExecutionAction> { self.actions.pop_front() }

    pub fn emit_event(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
        event: ExtensionEvent,
    ) -> Result<Vec<ExecutionAction>, String> {
        let grant = self.permissions.get(extension, profile).ok_or("extension is not installed")?;
        if event_needs_permission(&event.namespace) && !grant.allows_api(&event.namespace) {
            return Ok(Vec::new());
        }
        if self.manifests.get(&(extension.clone(), profile))
            .and_then(|manifest| manifest.background.as_ref()).is_none()
        {
            return Ok(Vec::new());
        }
        let action = self.lifecycle.dispatch(extension, profile, event)?;
        action.map(|action| self.materialize(action)).transpose()
            .map(|action| action.into_iter().collect())
    }

    pub fn emit_event_all(
        &mut self,
        profile: ProfileScope,
        event: ExtensionEvent,
    ) -> Vec<Result<ExecutionAction, String>> {
        let extensions = self.manifests.keys().filter(|(_, scope)| *scope == profile)
            .map(|(extension, _)| extension.clone()).collect::<Vec<_>>();
        let mut results = Vec::new();
        for extension in extensions {
            match self.emit_event(&extension, profile, event.clone()) {
                Ok(actions) => results.extend(actions.into_iter().map(Ok)),
                Err(error) => results.push(Err(error)),
            }
        }
        results
    }

    pub fn background_started(&mut self, context: &ContextId) -> Result<(), String> {
        if self.backgrounds.get(&(context.extension.clone(), context.profile)) != Some(context) {
            return Err("background context is not current".into());
        }
        self.register_context(context.clone());
        self.lifecycle.started(&context.extension, context.profile)
    }

    pub fn next_background_event(&mut self, context: &ContextId) -> Option<ExtensionEvent> {
        self.lifecycle.next_event(&context.extension, context.profile)
    }

    pub fn next_message(&mut self, context: &ContextId) -> Option<RuntimeMessage> {
        self.dispatcher.messages.receive(context)
    }

    pub fn reply_message(
        &mut self,
        context: &ContextId,
        request_id: u64,
        result: Result<serde_json::Value, String>,
    ) -> Result<(), String> {
        self.dispatcher.messages.reply(context, request_id, result)
    }

    pub fn next_reply(&mut self, context: &ContextId) -> Option<RuntimeReply> {
        self.dispatcher.messages.receive_reply(context)
    }

    pub fn background_crashed(&mut self, context: &ContextId) {
        self.unregister_context(context);
        self.lifecycle.crashed(&context.extension, context.profile);
    }

    pub fn tick(&mut self, now: Instant) -> Vec<Result<ExecutionAction, String>> {
        self.lifecycle.tick(now).into_iter().map(|action| self.materialize(action)).collect()
    }

    pub fn destroy_tab(&mut self, tab_id: u64) {
        let contexts = self.contexts.iter().filter(|context| {
            matches!(context.kind, ContextKind::ContentScript { tab_id: id, .. } if id == tab_id)
        }).cloned().collect::<Vec<_>>();
        for context in contexts { self.unregister_context(&context); }
        self.worlds.destroy_tab(tab_id);
    }

    pub fn drop_profile(&mut self, profile: ProfileScope) {
        let extensions = self.manifests.keys().filter(|(_, scope)| *scope == profile)
            .map(|(extension, _)| extension.clone()).collect::<Vec<_>>();
        for extension in extensions { self.uninstall(&extension, profile); }
        self.dispatcher.messages.drop_profile(profile);
        self.dispatcher.storage.drop_profile(profile);
        if let ProfileScope::Private(id) = profile {
            self.permissions.drop_private_profile(id);
            self.worlds.drop_private_profile(id);
            self.lifecycle.drop_private_profile(id);
        }
    }

    fn materialize(&mut self, action: LifecycleAction) -> Result<ExecutionAction, String> {
        match action {
            LifecycleAction::Start { extension, profile, kind } => {
                if let Some(old) = self.backgrounds.remove(&(extension.clone(), profile)) {
                    self.unregister_context(&old);
                }
                let context = self.context(extension.clone(), profile, match kind {
                    BackgroundKind::ManifestV2Page => ContextKind::Background,
                    BackgroundKind::ManifestV3Worker => ContextKind::ServiceWorker,
                });
                let source = self.manifests.get(&(extension.clone(), profile))
                    .and_then(|manifest| manifest.background.clone())
                    .ok_or("background source is missing")?;
                self.backgrounds.insert((extension, profile), context.clone());
                Ok(ExecutionAction::StartBackground { context, source })
            }
            LifecycleAction::Stop { extension, profile } => {
                let context = self.backgrounds.remove(&(extension, profile))
                    .ok_or("background context is missing")?;
                self.unregister_context(&context);
                Ok(ExecutionAction::StopBackground { context })
            }
        }
    }

    fn context(&mut self, extension: ExtensionId, profile: ProfileScope, kind: ContextKind) -> ContextId {
        self.next_context = self.next_context.wrapping_add(1).max(1);
        ContextId { extension, profile, serial: self.next_context, kind }
    }

    fn register_context(&mut self, context: ContextId) {
        self.dispatcher.messages.register(context.clone());
        self.contexts.insert(context);
    }

    fn unregister_context(&mut self, context: &ContextId) {
        self.contexts.remove(context);
        self.dispatcher.messages.unregister(context);
    }
}

fn event_needs_permission(namespace: &str) -> bool {
    matches!(namespace, "bookmarks" | "browsingData" | "cookies" | "downloads" | "history"
        | "management" | "notifications" | "sessions" | "webNavigation" | "webRequest")
}

fn requested_permissions(arguments: &serde_json::Value) -> Vec<String> {
    ["permissions", "origins"].into_iter()
        .filter_map(|field| arguments.get(field).and_then(serde_json::Value::as_array))
        .flatten().filter_map(serde_json::Value::as_str).map(str::to_string).collect()
}
