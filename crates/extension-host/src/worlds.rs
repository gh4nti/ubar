use crate::manifest::{ContentScriptSpec, NormalizedManifest};
use crate::permissions::{MatchPattern, PermissionStore};
use crate::runtime::{ContextId, ContextKind, ExtensionId, ProfileScope};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RunAt {
    DocumentStart,
    DocumentEnd,
    DocumentIdle,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptInjection {
    pub files: Vec<String>,
    pub code: Vec<String>,
    pub css_files: Vec<String>,
    pub css: Vec<String>,
    pub run_at: RunAt,
    pub all_frames: bool,
    pub match_about_blank: bool,
}

#[derive(Clone, Debug)]
struct RegisteredScript {
    matches: Vec<MatchPattern>,
    excludes: Vec<MatchPattern>,
    spec: ContentScriptSpec,
}

#[derive(Clone, Debug)]
struct RegisteredExtension {
    base_url: String,
    scripts: Vec<RegisteredScript>,
}

#[derive(Clone, Debug)]
pub struct NavigationInjectionPlan {
    pub world: WorldHandle,
    pub base_url: String,
    pub injections: Vec<ScriptInjection>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct WorldId {
    pub extension: ExtensionId,
    pub profile: ProfileScope,
    pub tab_id: u64,
    pub frame_id: u64,
    pub navigation_id: u64,
    pub serial: u64,
}

#[derive(Clone, Debug)]
pub struct WorldHandle {
    pub id: WorldId,
    pub context: ContextId,
    pub page_url: String,
    capability: [u8; 32],
}

impl WorldHandle {
    /// Passed only through the engine's private isolated-world channel.
    pub fn capability(&self) -> [u8; 32] { self.capability }
}

#[derive(Default)]
pub struct ContentWorldManager {
    next_world: u64,
    next_context: u64,
    worlds: BTreeMap<WorldId, WorldHandle>,
    scripts: BTreeMap<(ExtensionId, ProfileScope), RegisteredExtension>,
}

impl ContentWorldManager {
    pub fn register_manifest(
        &mut self,
        extension: ExtensionId,
        profile: ProfileScope,
        base_url: &str,
        manifest: &NormalizedManifest,
    ) -> Result<(), String> {
        let scripts = manifest.content_scripts.iter().cloned().map(|spec| {
            let matches = spec.matches.iter().map(|value| MatchPattern::parse(value))
                .collect::<Result<_, _>>()?;
            let excludes = spec.exclude_matches.iter().map(|value| MatchPattern::parse(value))
                .collect::<Result<_, _>>()?;
            Ok(RegisteredScript { matches, excludes, spec })
        }).collect::<Result<_, String>>()?;
        self.scripts.insert((extension, profile), RegisteredExtension {
            base_url: base_url.into(), scripts,
        });
        Ok(())
    }

    pub fn plan_navigation(
        &mut self,
        profile: ProfileScope,
        tab_id: u64,
        frame_id: u64,
        navigation_id: u64,
        page_url: &str,
        inherited_origin_url: Option<&str>,
    ) -> Result<Vec<NavigationInjectionPlan>, String> {
        self.commit_navigation(tab_id, frame_id, navigation_id);
        let keys = self.scripts.keys()
            .filter(|(_, scope)| *scope == profile).cloned().collect::<Vec<_>>();
        let mut plans = Vec::new();
        for (extension, _) in keys {
            let Some(registered) = self.scripts.get(&(extension.clone(), profile)) else { continue };
            let effective_url = if is_blank_or_srcdoc(page_url) {
                inherited_origin_url.unwrap_or(page_url)
            } else {
                page_url
            };
            let injections = registered.scripts.iter().filter(|script| {
                (frame_id == 0 || script.spec.all_frames)
                    && (!is_blank_or_srcdoc(page_url) || script.spec.match_about_blank)
                    && script.matches.iter().any(|pattern| pattern.matches(effective_url))
                    && !script.excludes.iter().any(|pattern| pattern.matches(effective_url))
                    && glob_filters_match(&script.spec, effective_url)
            }).map(|script| ScriptInjection {
                files: script.spec.js.clone(),
                code: Vec::new(),
                css_files: script.spec.css.clone(),
                css: Vec::new(),
                run_at: script.spec.run_at,
                all_frames: script.spec.all_frames,
                match_about_blank: script.spec.match_about_blank,
            }).collect::<Vec<_>>();
            if injections.is_empty() { continue; }
            let base_url = registered.base_url.clone();
            let world = self.create_authorized(
                extension, profile, tab_id, frame_id, navigation_id, effective_url,
            )?;
            plans.push(NavigationInjectionPlan {
                world, base_url, injections,
            });
        }
        Ok(plans)
    }

    pub fn create(
        &mut self,
        permissions: &PermissionStore,
        extension: ExtensionId,
        profile: ProfileScope,
        tab_id: u64,
        frame_id: u64,
        navigation_id: u64,
        page_url: &str,
    ) -> Result<WorldHandle, String> {
        let grant = permissions.get(&extension, profile).ok_or("extension has no permission grant")?;
        if !grant.allows_url(page_url) {
            return Err("extension has no host permission for this frame".into());
        }
        self.create_authorized(extension, profile, tab_id, frame_id, navigation_id, page_url)
    }

    fn create_authorized(
        &mut self,
        extension: ExtensionId,
        profile: ProfileScope,
        tab_id: u64,
        frame_id: u64,
        navigation_id: u64,
        page_url: &str,
    ) -> Result<WorldHandle, String> {
        self.next_world = self.next_world.wrapping_add(1).max(1);
        self.next_context = self.next_context.wrapping_add(1).max(1);
        let id = WorldId {
            extension: extension.clone(), profile, tab_id, frame_id, navigation_id,
            serial: self.next_world,
        };
        let mut capability = [0u8; 32];
        getrandom::fill(&mut capability).map_err(|error| error.to_string())?;
        let handle = WorldHandle {
            id: id.clone(),
            context: ContextId {
                extension, profile, serial: self.next_context,
                kind: ContextKind::ContentScript { tab_id, frame_id },
            },
            page_url: page_url.into(),
            capability,
        };
        self.worlds.insert(id, handle.clone());
        Ok(handle)
    }

    pub fn authenticate(&self, id: &WorldId, supplied_capability: &[u8]) -> Option<&WorldHandle> {
        let world = self.worlds.get(id)?;
        constant_time_equal(&world.capability, supplied_capability).then_some(world)
    }

    pub fn commit_navigation(&mut self, tab_id: u64, frame_id: u64, navigation_id: u64) {
        self.worlds.retain(|id, _| {
            id.tab_id != tab_id || id.frame_id != frame_id || id.navigation_id == navigation_id
        });
    }

    pub fn destroy_tab(&mut self, tab_id: u64) {
        self.worlds.retain(|id, _| id.tab_id != tab_id);
    }

    pub fn drop_private_profile(&mut self, private_id: u64) {
        self.worlds.retain(|id, _| id.profile != ProfileScope::Private(private_id));
        self.scripts.retain(|(_, profile), _| *profile != ProfileScope::Private(private_id));
    }

    pub fn uninstall(&mut self, extension: &ExtensionId, profile: ProfileScope) {
        self.scripts.remove(&(extension.clone(), profile));
        self.worlds.retain(|id, _| id.extension != *extension || id.profile != profile);
    }
}

fn is_blank_or_srcdoc(url: &str) -> bool { matches!(url, "about:blank" | "about:srcdoc") }

fn glob_filters_match(script: &ContentScriptSpec, url: &str) -> bool {
    (script.include_globs.is_empty() || script.include_globs.iter().any(|glob| glob_match(glob, url)))
        && !script.exclude_globs.iter().any(|glob| glob_match(glob, url))
}

fn glob_match(pattern: &str, value: &str) -> bool {
    let (mut pattern, mut value) = (pattern.as_bytes(), value.as_bytes());
    let (mut star, mut retry) = (None, 0);
    while !value.is_empty() {
        if pattern.first().is_some_and(|byte| *byte == b'?' || *byte == value[0]) {
            pattern = &pattern[1..]; value = &value[1..];
        } else if pattern.first() == Some(&b'*') {
            pattern = &pattern[1..]; star = Some(pattern); retry = value.len();
        } else if let Some(after_star) = star {
            if retry == 0 { return false; }
            retry -= 1; value = &value[value.len() - retry..]; pattern = after_star;
        } else { return false; }
    }
    pattern.iter().all(|byte| *byte == b'*')
}

fn constant_time_equal(expected: &[u8; 32], supplied: &[u8]) -> bool {
    if supplied.len() != expected.len() { return false; }
    expected.iter().zip(supplied).fold(0u8, |difference, (left, right)| {
        difference | (left ^ right)
    }) == 0
}
