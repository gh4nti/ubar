use crate::runtime::{ExtensionId, ProfileScope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BackgroundKind {
    ManifestV2Page,
    ManifestV3Worker,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BackgroundState {
    Stopped,
    Starting,
    Running,
    Crashed,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtensionEvent {
    pub namespace: String,
    pub name: String,
    pub arguments: Value,
}

struct Background {
    kind: BackgroundKind,
    state: BackgroundState,
    queued: VecDeque<ExtensionEvent>,
    idle_deadline: Option<Instant>,
    restart_after: Option<Instant>,
    crashes: u32,
}

pub enum LifecycleAction {
    Start { extension: ExtensionId, profile: ProfileScope, kind: BackgroundKind },
    Stop { extension: ExtensionId, profile: ProfileScope },
}

pub struct LifecycleManager {
    entries: BTreeMap<(ExtensionId, ProfileScope), Background>,
    worker_idle: Duration,
}

impl Default for LifecycleManager {
    fn default() -> Self {
        Self { entries: BTreeMap::new(), worker_idle: Duration::from_secs(30) }
    }
}

impl LifecycleManager {
    pub fn register(&mut self, extension: ExtensionId, profile: ProfileScope, kind: BackgroundKind) {
        self.entries.insert((extension, profile), Background {
            kind,
            state: BackgroundState::Stopped,
            queued: VecDeque::new(),
            idle_deadline: None,
            restart_after: None,
            crashes: 0,
        });
    }

    pub fn dispatch(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
        event: ExtensionEvent,
    ) -> Result<Option<LifecycleAction>, String> {
        let background = self.entries.get_mut(&(extension.clone(), profile))
            .ok_or("extension has no registered background context")?;
        if background.state == BackgroundState::Disabled {
            return Err("extension background is disabled".into());
        }
        background.queued.push_back(event);
        if matches!(background.state, BackgroundState::Stopped | BackgroundState::Crashed) {
            background.state = BackgroundState::Starting;
            return Ok(Some(LifecycleAction::Start {
                extension: extension.clone(), profile, kind: background.kind,
            }));
        }
        Ok(None)
    }

    pub fn started(&mut self, extension: &ExtensionId, profile: ProfileScope) -> Result<(), String> {
        let background = self.entries.get_mut(&(extension.clone(), profile)).ok_or("unknown background")?;
        background.state = BackgroundState::Running;
        background.crashes = 0;
        background.restart_after = None;
        self.touch(extension, profile)
    }

    pub fn touch(&mut self, extension: &ExtensionId, profile: ProfileScope) -> Result<(), String> {
        let background = self.entries.get_mut(&(extension.clone(), profile)).ok_or("unknown background")?;
        background.idle_deadline = (background.kind == BackgroundKind::ManifestV3Worker)
            .then(|| Instant::now() + self.worker_idle);
        Ok(())
    }

    pub fn next_event(&mut self, extension: &ExtensionId, profile: ProfileScope) -> Option<ExtensionEvent> {
        self.entries.get_mut(&(extension.clone(), profile))?.queued.pop_front()
    }

    pub fn crashed(&mut self, extension: &ExtensionId, profile: ProfileScope) {
        if let Some(background) = self.entries.get_mut(&(extension.clone(), profile)) {
            background.state = BackgroundState::Crashed;
            background.crashes = background.crashes.saturating_add(1);
            let delay = 1u64 << background.crashes.min(6);
            background.restart_after = Some(Instant::now() + Duration::from_secs(delay));
        }
    }

    pub fn tick(&mut self, now: Instant) -> Vec<LifecycleAction> {
        let mut actions = Vec::new();
        for ((extension, profile), background) in &mut self.entries {
            if background.state == BackgroundState::Running
                && background.idle_deadline.is_some_and(|deadline| deadline <= now)
                && background.queued.is_empty()
            {
                background.state = BackgroundState::Stopped;
                background.idle_deadline = None;
                actions.push(LifecycleAction::Stop { extension: extension.clone(), profile: *profile });
            } else if background.state == BackgroundState::Crashed
                && background.restart_after.is_some_and(|deadline| deadline <= now)
                && !background.queued.is_empty()
            {
                background.state = BackgroundState::Starting;
                actions.push(LifecycleAction::Start {
                    extension: extension.clone(), profile: *profile, kind: background.kind,
                });
            }
        }
        actions
    }

    pub fn set_enabled(&mut self, extension: &ExtensionId, profile: ProfileScope, enabled: bool) {
        if let Some(background) = self.entries.get_mut(&(extension.clone(), profile)) {
            background.state = if enabled { BackgroundState::Stopped } else { BackgroundState::Disabled };
            background.queued.clear();
        }
    }

    pub fn drop_private_profile(&mut self, private_id: u64) {
        self.entries.retain(|(_, profile), _| *profile != ProfileScope::Private(private_id));
    }
}
