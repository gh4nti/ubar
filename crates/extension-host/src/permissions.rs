use crate::runtime::{ExtensionId, ProfileScope};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct MatchPattern {
    scheme: String,
    host: String,
    path: String,
}

impl MatchPattern {
    pub fn parse(pattern: &str) -> Result<Self, String> {
        if pattern == "<all_urls>" {
            return Ok(Self { scheme: "*".into(), host: "*".into(), path: "/*".into() });
        }
        let (scheme, remainder) = pattern
            .split_once("://")
            .ok_or("match pattern must contain ://")?;
        if !matches!(scheme, "*" | "http" | "https" | "file" | "ftp") {
            return Err("unsupported match-pattern scheme".into());
        }
        let (host, path) = remainder.split_once('/').unwrap_or((remainder, ""));
        if host.is_empty() && scheme != "file" {
            return Err("match pattern has no host".into());
        }
        if host.contains('*') && host != "*" && !host.starts_with("*.") {
            return Err("host wildcard is only allowed as * or *.<domain>".into());
        }
        Ok(Self {
            scheme: scheme.into(),
            host: host.to_ascii_lowercase(),
            path: format!("/{path}"),
        })
    }

    pub fn matches(&self, url: &str) -> bool {
        let Some((scheme, remainder)) = url.split_once("://") else { return false };
        if self.scheme != "*" && self.scheme != scheme {
            return false;
        }
        if self.scheme == "*" && !matches!(scheme, "http" | "https") {
            return false;
        }
        let (authority, path) = remainder.split_once('/').unwrap_or((remainder, ""));
        let host = authority
            .rsplit_once('@').map_or(authority, |(_, value)| value)
            .split(':').next().unwrap_or("")
            .to_ascii_lowercase();
        let host_matches = self.host == "*"
            || host == self.host
            || self.host.strip_prefix("*.").is_some_and(|suffix| {
                host == suffix || host.ends_with(&format!(".{suffix}"))
            });
        host_matches && wildcard_matches(&self.path, &format!("/{path}"))
    }
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let mut rest = value;
    let mut first = true;
    for part in pattern.split('*') {
        if part.is_empty() { continue; }
        let Some(index) = rest.find(part) else { return false };
        if first && !pattern.starts_with('*') && index != 0 { return false; }
        rest = &rest[index + part.len()..];
        first = false;
    }
    pattern.ends_with('*') || rest.is_empty()
}

#[derive(Clone, Debug, Default)]
pub struct PermissionGrant {
    pub required_apis: BTreeSet<String>,
    pub optional_apis: BTreeSet<String>,
    pub required_origins: BTreeSet<MatchPattern>,
    pub optional_origins: BTreeSet<MatchPattern>,
}

impl PermissionGrant {
    pub fn allows_api(&self, permission: &str) -> bool {
        self.required_apis.contains(permission) || self.optional_apis.contains(permission)
    }

    pub fn allows_url(&self, url: &str) -> bool {
        self.required_origins.iter().chain(&self.optional_origins).any(|p| p.matches(url))
    }
}

#[derive(Default)]
pub struct PermissionStore {
    grants: BTreeMap<(ExtensionId, ProfileScope), PermissionGrant>,
}

impl PermissionStore {
    pub fn install(
        &mut self,
        extension: ExtensionId,
        profile: ProfileScope,
        required_apis: impl IntoIterator<Item = String>,
        required_origins: impl IntoIterator<Item = String>,
    ) -> Result<(), String> {
        let origins = required_origins
            .into_iter().map(|value| MatchPattern::parse(&value)).collect::<Result<_, _>>()?;
        self.grants.insert((extension, profile), PermissionGrant {
            required_apis: required_apis.into_iter().collect(),
            required_origins: origins,
            ..PermissionGrant::default()
        });
        Ok(())
    }

    pub fn grant_optional_api(&mut self, extension: &ExtensionId, profile: ProfileScope, api: String) {
        self.grants.entry((extension.clone(), profile)).or_default().optional_apis.insert(api);
    }

    pub fn grant_optional_origin(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
        origin: &str,
    ) -> Result<(), String> {
        self.grants.entry((extension.clone(), profile)).or_default()
            .optional_origins.insert(MatchPattern::parse(origin)?);
        Ok(())
    }

    pub fn revoke_optional(&mut self, extension: &ExtensionId, profile: ProfileScope) {
        if let Some(grant) = self.grants.get_mut(&(extension.clone(), profile)) {
            grant.optional_apis.clear();
            grant.optional_origins.clear();
        }
    }

    pub fn get(&self, extension: &ExtensionId, profile: ProfileScope) -> Option<&PermissionGrant> {
        self.grants.get(&(extension.clone(), profile))
    }

    pub fn drop_private_profile(&mut self, private_id: u64) {
        self.grants.retain(|(_, profile), _| *profile != ProfileScope::Private(private_id));
    }
}
