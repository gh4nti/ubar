use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

const MAX_QUEUED_MESSAGES: usize = 1024;
const MAX_OPEN_REQUESTS: usize = 4096;

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
pub struct RuntimeReply {
    pub request_id: u64,
    pub responder: ContextId,
    pub result: Result<Value, String>,
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
    pub second: Option<ContextId>,
    pub target_extension: ExtensionId,
    pub target_profile: ProfileScope,
    pub disconnected: bool,
    pending: VecDeque<PortMessage>,
}

#[derive(Default)]
pub struct MessageBus {
    next_request: u64,
    next_port: u64,
    contexts: BTreeMap<ContextId, VecDeque<RuntimeMessage>>,
    pending: BTreeMap<(ExtensionId, ProfileScope), VecDeque<RuntimeMessage>>,
    replies: BTreeMap<ContextId, VecDeque<RuntimeReply>>,
    requests: BTreeMap<u64, (ContextId, ExtensionId, ProfileScope)>,
    port_messages: BTreeMap<ContextId, VecDeque<PortMessage>>,
    ports: BTreeMap<u64, Port>,
}

impl MessageBus {
    pub fn register(&mut self, context: ContextId) {
        let queue = self.contexts.entry(context.clone()).or_default();
        if matches!(context.kind, ContextKind::Background | ContextKind::ServiceWorker) {
            if let Some(mut pending) = self.pending.remove(&(context.extension.clone(), context.profile)) {
                queue.append(&mut pending);
            }
        }
        self.replies.entry(context.clone()).or_default();
        self.port_messages.entry(context).or_default();
        let mut waiting = VecDeque::new();
        for port in self.ports.values_mut() {
            if port.second.is_none() && !port.disconnected
                && port.target_extension == context.extension && port.target_profile == context.profile
                && port.first != context
            {
                port.second = Some(context.clone());
                waiting.append(&mut port.pending);
            }
        }
        if let Some(queue) = self.port_messages.get_mut(&context) { queue.append(&mut waiting); }
    }

    pub fn unregister(&mut self, context: &ContextId) {
        self.contexts.remove(context);
        self.port_messages.remove(context);
        self.replies.remove(context);
        self.requests.retain(|_, (sender, _, _)| sender != context);
        for port in self.ports.values_mut() {
            if &port.first == context || port.second.as_ref() == Some(context) {
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
        if self.requests.len() >= MAX_OPEN_REQUESTS { return Err("too many open extension messages".into()); }
        self.next_request = self.next_request.wrapping_add(1).max(1);
        let message = RuntimeMessage {
            sender: sender.clone(),
            recipient_extension: recipient_extension.clone(),
            payload,
            request_id: self.next_request,
        };
        let request_id = message.request_id;
        self.requests.insert(message.request_id, (
            sender.clone(), recipient_extension.clone(), sender.profile,
        ));
        let mut delivered = 0;
        for (context, queue) in &mut self.contexts {
            if context.extension == recipient_extension && context.profile == sender.profile
                && context != sender
            {
                if queue.len() < MAX_QUEUED_MESSAGES {
                    queue.push_back(message.clone()); delivered += 1;
                }
            }
        }
        if delivered == 0 {
            if matches!(sender.kind, ContextKind::Background | ContextKind::ServiceWorker)
                && sender.extension == recipient_extension
            {
                self.requests.remove(&request_id);
                return Err("receiving extension has no other live context".into());
            }
            let queue = self.pending.entry((recipient_extension, sender.profile)).or_default();
            if queue.len() >= MAX_QUEUED_MESSAGES {
                self.requests.remove(&request_id);
                return Err("receiving extension message queue is full".into());
            }
            queue.push_back(message);
        }
        Ok(request_id)
    }

    pub fn send_tab_message(
        &mut self,
        sender: &ContextId,
        tab_id: u64,
        frame_id: Option<u64>,
        payload: Value,
    ) -> Result<u64, String> {
        if !self.contexts.contains_key(sender) { return Err("sender context is not registered".into()); }
        if self.requests.len() >= MAX_OPEN_REQUESTS { return Err("too many open extension messages".into()); }
        self.next_request = self.next_request.wrapping_add(1).max(1);
        let message = RuntimeMessage {
            sender: sender.clone(), recipient_extension: sender.extension.clone(), payload,
            request_id: self.next_request,
        };
        self.requests.insert(message.request_id, (
            sender.clone(), sender.extension.clone(), sender.profile,
        ));
        let mut delivered = 0;
        for (context, queue) in &mut self.contexts {
            if context.extension == sender.extension && context.profile == sender.profile
                && matches!(context.kind, ContextKind::ContentScript { tab_id: id, frame_id: frame }
                    if id == tab_id && frame_id.is_none_or(|requested| requested == frame))
            {
                if queue.len() < MAX_QUEUED_MESSAGES {
                    queue.push_back(message.clone()); delivered += 1;
                }
            }
        }
        if delivered == 0 {
            self.requests.remove(&message.request_id);
            return Err("tab has no matching content-script context".into());
        }
        Ok(message.request_id)
    }

    pub fn reply(
        &mut self,
        responder: &ContextId,
        request_id: u64,
        result: Result<Value, String>,
    ) -> Result<(), String> {
        if !self.contexts.contains_key(responder) { return Err("responder context is not registered".into()); }
        let (original, recipient, profile) = self.requests.get(&request_id).cloned()
            .ok_or("unknown message request")?;
        if responder.extension != recipient || responder.profile != profile {
            return Err("responder is not a recipient of this request".into());
        }
        let replies = self.replies.get_mut(&original).ok_or("message sender is no longer registered")?;
        if replies.len() >= MAX_QUEUED_MESSAGES { return Err("message reply queue is full".into()); }
        self.requests.remove(&request_id);
        replies.push_back(RuntimeReply { request_id, responder: responder.clone(), result });
        Ok(())
    }

    pub fn receive_reply(&mut self, context: &ContextId) -> Option<RuntimeReply> {
        self.replies.get_mut(context)?.pop_front()
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
                second: Some(second.clone()),
                target_extension: second.extension.clone(),
                target_profile: second.profile,
                disconnected: false,
                pending: VecDeque::new(),
            },
        );
        Ok(self.next_port)
    }

    pub fn connect_extension(
        &mut self,
        first: &ContextId,
        extension: ExtensionId,
        name: impl Into<String>,
    ) -> Result<u64, String> {
        if !self.contexts.contains_key(first) { return Err("sender context is not registered".into()); }
        if let Some(second) = self.context_for_extension(&extension, first.profile, Some(first)) {
            return self.connect(first, &second, name);
        }
        self.next_port = self.next_port.wrapping_add(1).max(1);
        self.ports.insert(self.next_port, Port {
            id: self.next_port, name: name.into(), first: first.clone(), second: None,
            target_extension: extension, target_profile: first.profile,
            disconnected: false, pending: VecDeque::new(),
        });
        Ok(self.next_port)
    }

    pub fn post_port(
        &mut self,
        port_id: u64,
        sender: &ContextId,
        payload: Value,
    ) -> Result<(), String> {
        let port = self.ports.get_mut(&port_id).ok_or("unknown port")?;
        if port.disconnected {
            return Err("port is disconnected".into());
        }
        let recipient = if &port.first == sender {
            if let Some(second) = &port.second { second } else {
                if port.pending.len() >= MAX_QUEUED_MESSAGES { return Err("pending port queue is full".into()); }
                port.pending.push_back(PortMessage { port_id, sender: sender.clone(), payload });
                return Ok(());
            }
        } else if port.second.as_ref() == Some(sender) {
            &port.first
        } else {
            return Err("sender is not attached to the port".into());
        };
        let queue = self.port_messages.get_mut(recipient)
            .ok_or("recipient context is not registered")?;
        if queue.len() >= MAX_QUEUED_MESSAGES { return Err("port message queue is full".into()); }
        queue.push_back(PortMessage {
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

    pub fn drop_profile(&mut self, profile: ProfileScope) {
        let contexts = self.contexts.keys().filter(|context| context.profile == profile)
            .cloned().collect::<Vec<_>>();
        for context in contexts { self.unregister(&context); }
        self.pending.retain(|(_, scope), _| *scope != profile);
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
    pub fn drop_profile(&mut self, profile: ProfileScope) {
        self.values.retain(|key, _| key.profile != profile);
        self.changes.retain(|(_, scope), _| *scope != profile);
    }

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
