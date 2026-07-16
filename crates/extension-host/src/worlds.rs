use crate::permissions::PermissionStore;
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
}

impl ContentWorldManager {
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
    }
}

fn constant_time_equal(expected: &[u8; 32], supplied: &[u8]) -> bool {
    if supplied.len() != expected.len() { return false; }
    expected.iter().zip(supplied).fold(0u8, |difference, (left, right)| {
        difference | (left ^ right)
    }) == 0
}
