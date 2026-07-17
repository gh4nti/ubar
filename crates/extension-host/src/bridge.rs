use crate::worlds::WorldHandle;
use base64::Engine;

/// JavaScript provider installed only in a WebKit isolated content world.
/// The native port owns `__ubarExtensionBridge`; page scripts cannot reach it.
pub fn isolated_world_provider(world: &WorldHandle) -> String {
    let capability = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(world.capability());
    provider(&format!("{capability:?}"))
}

/// Provider for background/popup documents whose native host already binds a
/// trusted context; no page-visible capability is needed.
pub fn trusted_context_provider() -> String { provider("null") }

fn provider(capability_literal: &str) -> String {
    format!(r#"(() => {{
  'use strict';
  const capability = {capability_literal};
  if (globalThis.__ubarExtensionProviderReady === capability) return;
  let sequence = 0;
  const pending = new Map(), events = new Map();
  const event = key => Object.freeze({{
    addListener(fn) {{ if (typeof fn === 'function') (events.get(key) || events.set(key, new Set()).get(key)).add(fn); }},
    removeListener(fn) {{ events.get(key)?.delete(fn); }},
    hasListener(fn) {{ return events.get(key)?.has(fn) || false; }},
    hasListeners() {{ return (events.get(key)?.size || 0) !== 0; }}
  }});
  const call = (namespace, member, args) => new Promise((resolve, reject) => {{
    const requestId = ++sequence;
    pending.set(requestId, {{resolve, reject}});
    globalThis.__ubarExtensionBridge.postMessage(JSON.stringify({{
      capability, requestId, namespace, member,
      arguments: normalize(namespace, member, args),
      userGesture: navigator.userActivation?.isActive === true
    }}));
  }});
  const normalize = (namespace, member, args) => {{
    if (namespace === 'runtime' && member === 'sendMessage') {{
      const external = typeof args[0] === 'string';
      return {{extensionId: external ? args[0] : undefined, message: args[external ? 1 : 0]}};
    }}
    if (namespace === 'runtime' && member === 'connect') {{
      const external = typeof args[0] === 'string';
      return {{extensionId: external ? args[0] : undefined, ...args[external ? 1 : 0]}};
    }}
    if (namespace === 'tabs' && member === 'sendMessage')
      return {{tabId: args[0], message: args[1], frameId: args[2]?.frameId}};
    if (namespace === 'tabs' && member === 'update')
      return {{tabId: args[0], ...(args[1] || {{}})}};
    if (namespace === 'tabs' && member === 'remove')
      return {{tabIds: Array.isArray(args[0]) ? args[0] : [args[0]]}};
    if (namespace === 'tabs') return args[0] || {{}};
    if (namespace === 'permissions') return args[0] || {{}};
    return args;
  }};
  const namespace = name => new Proxy({{}}, {{get(_, member) {{
    if (String(member).startsWith('on')) return event(`${{name}}.${{String(member)}}`);
    return (...args) => call(name, String(member), args);
  }}}});
  const storageArea = area => Object.freeze({{
    get: keys => call('storage', area, {{operation:'get', keys: keys == null ? undefined : (Array.isArray(keys) ? keys : typeof keys === 'string' ? [keys] : Object.keys(keys))}}),
    set: items => call('storage', area, {{operation:'set', items}}),
    remove: keys => call('storage', area, {{operation:'remove', keys: Array.isArray(keys) ? keys : [keys]}}),
    clear: () => call('storage', area, {{operation:'clear'}})
  }});
  const browser = new Proxy({{storage: Object.freeze({{local:storageArea('local'), sync:storageArea('sync'), session:storageArea('session')}})}}, {{
    get(target, name) {{ return target[name] || namespace(String(name)); }}
  }});
  const chrome = new Proxy(browser, {{get(target, name) {{
    const value = target[name];
    if (typeof value !== 'object' || name === 'storage') return value;
    return new Proxy(value, {{get(ns, member) {{
      const fn = ns[member]; if (typeof fn !== 'function') return fn;
      return (...args) => {{ const cb = typeof args.at(-1) === 'function' ? args.pop() : null;
        const promise = fn(...args); if (cb) promise.then(value => cb(value), () => cb()); return promise; }};
    }}});
  }}}});
  Object.defineProperties(globalThis, {{browser:{{value:browser}}, chrome:{{value:chrome}}}});
  Object.defineProperty(globalThis, '__ubarExtensionProviderReady', {{value:capability}});
  Object.defineProperty(globalThis, '__ubarExtensionDeliver', {{value: message => {{
    if (message.kind === 'response') {{ const waiter = pending.get(message.requestId); if (!waiter) return;
      pending.delete(message.requestId); message.error ? waiter.reject(new Error(message.error.message || String(message.error))) : waiter.resolve(message.result); return; }}
    if (message.kind === 'event') for (const listener of events.get(`${{message.namespace}}.${{message.name}}`) || []) listener(...(message.arguments || []));
    if (message.kind === 'message') for (const listener of events.get('runtime.onMessage') || []) listener(message.payload, message.sender, value =>
      globalThis.__ubarExtensionBridge.postMessage(JSON.stringify({{capability, replyTo:message.requestId, result:value}})));
  }}, configurable:false}});
}})();"#)
}
