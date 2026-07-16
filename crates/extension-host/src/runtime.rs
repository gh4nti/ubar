use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ExtensionId(pub String);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum ProfileScope {
    Normal,
    Private(u64),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum ContextKind {
    Background,
    ServiceWorker,
    ContentScript { tab_id: u64, frame_id: u64 },
    Popup,
    Options,
    DevTools,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ContextId {
    pub extension: ExtensionId,
    pub profile: ProfileScope,
    pub serial: u64,
    pub kind: ContextKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeMessage {
    pub sender: ContextId,
    pub recipient_extension: ExtensionId,
    pub payload: Value,
    pub request_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortMessage {
    pub port_id: u64,
    pub sender: ContextId,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub struct Port {
    pub id: u64,
    pub name: String,
    pub first: ContextId,
    pub second: ContextId,
    pub disconnected: bool,
}

#[derive(Default)]
pub struct MessageBus {
    next_request: u64,
    next_port: u64,
    contexts: BTreeMap<ContextId, VecDeque<RuntimeMessage>>,
    port_messages: BTreeMap<ContextId, VecDeque<PortMessage>>,
    ports: BTreeMap<u64, Port>,
}

impl MessageBus {
    pub fn register(&mut self, context: ContextId) {
        self.contexts.entry(context.clone()).or_default();
        self.port_messages.entry(context).or_default();
    }

    pub fn unregister(&mut self, context: &ContextId) {
        self.contexts.remove(context);
        self.port_messages.remove(context);
        for port in self.ports.values_mut() {
            if &port.first == context || &port.second == context {
                port.disconnected = true;
            }
        }
    }

    pub fn context_for_extension(
        &self,
        extension: &ExtensionId,
        profile: ProfileScope,
        excluding: Option<&ContextId>,
    ) -> Option<ContextId> {
        self.contexts
            .keys()
            .find(|context| {
                &context.extension == extension
                    && context.profile == profile
                    && excluding != Some(*context)
            })
            .cloned()
    }

    pub fn send_message(
        &mut self,
        sender: &ContextId,
        recipient_extension: ExtensionId,
        payload: Value,
    ) -> Result<u64, String> {
        if !self.contexts.contains_key(sender) {
            return Err("sender context is not registered".into());
        }
        self.next_request = self.next_request.wrapping_add(1).max(1);
        let message = RuntimeMessage {
            sender: sender.clone(),
            recipient_extension: recipient_extension.clone(),
            payload,
            request_id: self.next_request,
        };
        let mut delivered = 0;
        for (context, queue) in &mut self.contexts {
            if context.extension == recipient_extension && context.profile == sender.profile {
                queue.push_back(message.clone());
                delivered += 1;
            }
        }
        if delivered == 0 {
            return Err("receiving extension has no live context".into());
        }
        Ok(message.request_id)
    }

    pub fn receive(&mut self, context: &ContextId) -> Option<RuntimeMessage> {
        self.contexts.get_mut(context)?.pop_front()
    }

    pub fn connect(
        &mut self,
        first: &ContextId,
        second: &ContextId,
        name: impl Into<String>,
    ) -> Result<u64, String> {
        if first.profile != second.profile {
            return Err("ports cannot cross normal/private profile boundaries".into());
        }
        if !self.contexts.contains_key(first) || !self.contexts.contains_key(second) {
            return Err("both port contexts must be registered".into());
        }
        self.next_port = self.next_port.wrapping_add(1).max(1);
        self.ports.insert(
            self.next_port,
            Port {
                id: self.next_port,
                name: name.into(),
                first: first.clone(),
                second: second.clone(),
                disconnected: false,
            },
        );
        Ok(self.next_port)
    }

    pub fn post_port(
        &mut self,
        port_id: u64,
        sender: &ContextId,
        payload: Value,
    ) -> Result<(), String> {
        let port = self.ports.get(&port_id).ok_or("unknown port")?;
        if port.disconnected {
            return Err("port is disconnected".into());
        }
        let recipient = if &port.first == sender {
            &port.second
        } else if &port.second == sender {
            &port.first
        } else {
            return Err("sender is not attached to the port".into());
        };
        self.port_messages
            .get_mut(recipient)
            .ok_or("recipient context is not registered")?
            .push_back(PortMessage {
                port_id,
                sender: sender.clone(),
                payload,
            });
        Ok(())
    }

    pub fn receive_port(&mut self, context: &ContextId) -> Option<PortMessage> {
        self.port_messages.get_mut(context)?.pop_front()
    }

    pub fn disconnect(&mut self, port_id: u64) -> bool {
        let Some(port) = self.ports.get_mut(&port_id) else {
            return false;
        };
        port.disconnected = true;
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum StorageArea {
    Local,
    Sync,
    Session,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct StorageKey {
    extension: ExtensionId,
    profile: ProfileScope,
    area: StorageArea,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageChange {
    pub key: String,
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
}

#[derive(Default)]
pub struct StorageService {
    values: BTreeMap<StorageKey, BTreeMap<String, Value>>,
    changes: BTreeMap<(ExtensionId, ProfileScope), VecDeque<(StorageArea, Vec<StorageChange>)>>,
}

impl StorageService {
    fn key(extension: &ExtensionId, profile: ProfileScope, area: StorageArea) -> StorageKey {
        StorageKey { extension: extension.clone(), profile, area }
    }

    pub fn get(
        &self,
        extension: &ExtensionId,
        profile: ProfileScope,
        area: StorageArea,
        keys: Option<&[String]>,
    ) -> BTreeMap<String, Value> {
        let Some(values) = self.values.get(&Self::key(extension, profile, area)) else {
            return BTreeMap::new();
        };
        match keys {
            None => values.clone(),
            Some(keys) => keys
                .iter()
                .filter_map(|key| values.get(key).cloned().map(|value| (key.clone(), value)))
                .collect(),
        }
    }

    pub fn set(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
        area: StorageArea,
        additions: BTreeMap<String, Value>,
    ) -> Vec<StorageChange> {
        let values = self.values.entry(Self::key(extension, profile, area)).or_default();
        let changes = additions
            .into_iter()
            .filter_map(|(key, new_value)| {
                let old_value = values.insert(key.clone(), new_value.clone());
                (old_value.as_ref() != Some(&new_value)).then_some(StorageChange {
                    key,
                    old_value,
                    new_value: Some(new_value),
                })
            })
            .collect::<Vec<_>>();
        self.queue_changes(extension, profile, area, changes.clone());
        changes
    }

    pub fn remove(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
        area: StorageArea,
        keys: &[String],
    ) -> Vec<StorageChange> {
        let values = self.values.entry(Self::key(extension, profile, area)).or_default();
        let changes = keys
            .iter()
            .filter_map(|key| values.remove(key).map(|old_value| StorageChange {
                key: key.clone(), old_value: Some(old_value), new_value: None,
            }))
            .collect::<Vec<_>>();
        self.queue_changes(extension, profile, area, changes.clone());
        changes
    }

    pub fn clear(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
        area: StorageArea,
    ) -> Vec<StorageChange> {
        let values = self.values.remove(&Self::key(extension, profile, area)).unwrap_or_default();
        let changes = values
            .into_iter()
            .map(|(key, old_value)| StorageChange {
                key, old_value: Some(old_value), new_value: None,
            })
            .collect::<Vec<_>>();
        self.queue_changes(extension, profile, area, changes.clone());
        changes
    }

    fn queue_changes(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
        area: StorageArea,
        changes: Vec<StorageChange>,
    ) {
        if !changes.is_empty() {
            self.changes
                .entry((extension.clone(), profile))
                .or_default()
                .push_back((area, changes));
        }
    }

    pub fn receive_changes(
        &mut self,
        extension: &ExtensionId,
        profile: ProfileScope,
    ) -> Option<(StorageArea, Vec<StorageChange>)> {
        self.changes.get_mut(&(extension.clone(), profile))?.pop_front()
    }

    pub fn drop_private_profile(&mut self, private_id: u64) {
        self.values.retain(|key, _| key.profile != ProfileScope::Private(private_id));
        self.changes.retain(|(_, profile), _| *profile != ProfileScope::Private(private_id));
    }
}
