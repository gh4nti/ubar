use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ubar_engine_abi::UbarProfileKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SandboxPolicy {
    Required,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StoragePolicy {
    PartitionThirdPartyByTopLevelSite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ThirdPartyCookiePolicy {
    Block,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicy {
    pub sandbox: SandboxPolicy,
    pub storage: StoragePolicy,
    pub third_party_cookies: ThirdPartyCookiePolicy,
    pub isolate_sites: bool,
    pub authenticate_privileged_ipc: bool,
    pub verify_extension_signatures: bool,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            sandbox: SandboxPolicy::Required,
            storage: StoragePolicy::PartitionThirdPartyByTopLevelSite,
            third_party_cookies: ThirdPartyCookiePolicy::Block,
            isolate_sites: true,
            authenticate_privileged_ipc: true,
            verify_extension_signatures: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile {
    pub id: u64,
    pub kind: UbarProfileKind,
    pub persistent: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewState {
    pub id: u64,
    pub profile_id: u64,
    pub uri: String,
    pub visible: bool,
    pub suspended: bool,
}

#[derive(Debug)]
pub struct BrowserCore {
    security: SecurityPolicy,
    next_profile: u64,
    next_view: u64,
    profiles: BTreeMap<u64, Profile>,
    views: BTreeMap<u64, ViewState>,
}

impl BrowserCore {
    pub fn new(security: SecurityPolicy) -> Self {
        Self {
            security,
            next_profile: 1,
            next_view: 1,
            profiles: BTreeMap::new(),
            views: BTreeMap::new(),
        }
    }

    pub fn security(&self) -> &SecurityPolicy {
        &self.security
    }

    pub fn has_profile(&self, profile_id: u64) -> bool {
        self.profiles.contains_key(&profile_id)
    }

    pub fn create_profile(&mut self, kind: UbarProfileKind) -> Profile {
        let profile = Profile {
            id: self.next_profile,
            kind,
            persistent: kind == UbarProfileKind::Normal,
        };
        self.next_profile += 1;
        self.profiles.insert(profile.id, profile.clone());
        profile
    }

    pub fn create_view(&mut self, profile_id: u64, uri: impl Into<String>) -> Option<ViewState> {
        self.profiles.get(&profile_id)?;
        let view = ViewState {
            id: self.next_view,
            profile_id,
            uri: uri.into(),
            visible: true,
            suspended: false,
        };
        self.next_view += 1;
        self.views.insert(view.id, view.clone());
        Some(view)
    }

    pub fn destroy_profile(&mut self, profile_id: u64) -> bool {
        if self.profiles.remove(&profile_id).is_none() {
            return false;
        }
        self.views.retain(|_, view| view.profile_id != profile_id);
        true
    }

    pub fn destroy_view(&mut self, view_id: u64) -> bool {
        self.views.remove(&view_id).is_some()
    }

    pub fn navigate(&mut self, view_id: u64, uri: impl Into<String>) -> bool {
        let Some(view) = self.views.get_mut(&view_id) else {
            return false;
        };
        view.uri = uri.into();
        view.suspended = false;
        true
    }

    pub fn set_visible(&mut self, view_id: u64, visible: bool) -> bool {
        let Some(view) = self.views.get_mut(&view_id) else {
            return false;
        };
        view.visible = visible;
        if visible {
            view.suspended = false;
        }
        true
    }

    pub fn suspend_hidden(&mut self) -> usize {
        self.views
            .values_mut()
            .filter(|view| !view.visible && !view.suspended)
            .map(|view| {
                view.suspended = true;
            })
            .count()
    }

    pub fn suspend(&mut self, view_id: u64) -> bool {
        let Some(view) = self.views.get_mut(&view_id) else {
            return false;
        };
        if view.visible {
            return false;
        }
        view.suspended = true;
        true
    }

    pub fn resume(&mut self, view_id: u64) -> bool {
        let Some(view) = self.views.get_mut(&view_id) else {
            return false;
        };
        view.suspended = false;
        true
    }

    pub fn persistent_session(&self) -> Vec<&ViewState> {
        self.views
            .values()
            .filter(|view| {
                self.profiles
                    .get(&view.profile_id)
                    .is_some_and(|profile| profile.persistent)
            })
            .collect()
    }
}

impl Default for BrowserCore {
    fn default() -> Self {
        Self::new(SecurityPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_views_never_enter_persistent_session() {
        let mut core = BrowserCore::default();
        let normal = core.create_profile(UbarProfileKind::Normal);
        let private = core.create_profile(UbarProfileKind::Private);
        core.create_view(normal.id, "https://example.com").unwrap();
        core.create_view(private.id, "https://secret.example").unwrap();
        let session = core.persistent_session();
        assert_eq!(session.len(), 1);
        assert_eq!(session[0].uri, "https://example.com");
    }

    #[test]
    fn hidden_views_suspend_and_resume_when_visible() {
        let mut core = BrowserCore::default();
        let profile = core.create_profile(UbarProfileKind::Normal);
        let view = core.create_view(profile.id, "https://example.com").unwrap();
        assert!(core.set_visible(view.id, false));
        assert_eq!(core.suspend_hidden(), 1);
        assert!(core.set_visible(view.id, true));
        assert_eq!(core.suspend_hidden(), 0);
    }

    #[test]
    fn secure_defaults_are_not_optional() {
        let core = BrowserCore::default();
        assert_eq!(core.security().sandbox, SandboxPolicy::Required);
        assert!(core.security().isolate_sites);
        assert!(core.security().authenticate_privileged_ipc);
        assert!(core.security().verify_extension_signatures);
    }
}
