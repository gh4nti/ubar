use crate::lifecycle::BackgroundKind;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackgroundSpec {
    pub kind: BackgroundKind,
    pub scripts: Vec<String>,
    pub page: Option<String>,
    pub service_worker: Option<String>,
    pub module: bool,
    pub persistent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionSpec {
    pub default_title: Option<String>,
    pub default_popup: Option<String>,
    pub default_icon: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormalizedManifest {
    pub manifest_version: u64,
    pub name: String,
    pub version: String,
    pub extension_id: Option<String>,
    pub permissions: BTreeSet<String>,
    pub host_permissions: BTreeSet<String>,
    pub optional_permissions: BTreeSet<String>,
    pub optional_host_permissions: BTreeSet<String>,
    pub background: Option<BackgroundSpec>,
    pub action: Option<ActionSpec>,
    pub content_scripts: Vec<Value>,
    pub web_accessible_resources: Vec<Value>,
    pub content_security_policy: Value,
    pub chrome_compatibility_required: bool,
    pub warnings: Vec<String>,
    pub raw: Value,
}

pub fn normalize(value: Value) -> Result<NormalizedManifest, String> {
    let object = value.as_object().ok_or("manifest root must be an object")?;
    let manifest_version = object.get("manifest_version").and_then(Value::as_u64)
        .ok_or("manifest_version is required")?;
    if !matches!(manifest_version, 2 | 3) {
        return Err(format!("unsupported manifest_version {manifest_version}"));
    }
    let name = required_string(object, "name")?;
    let version = required_string(object, "version")?;
    let extension_id = value.pointer("/browser_specific_settings/gecko/id")
        .or_else(|| value.pointer("/applications/gecko/id"))
        .and_then(Value::as_str).map(str::to_string);
    let mut warnings = Vec::new();
    let permissions = strings(object.get("permissions"));
    let mut host_permissions = strings(object.get("host_permissions"));
    let optional_permissions = strings(object.get("optional_permissions"));
    let mut optional_host_permissions = strings(object.get("optional_host_permissions"));
    if manifest_version == 2 {
        host_permissions.extend(permissions.iter().filter(|value| is_origin(value)).cloned());
        optional_host_permissions.extend(optional_permissions.iter().filter(|value| is_origin(value)).cloned());
    }
    let background = normalize_background(object, manifest_version, &mut warnings)?;
    let action_value = object.get("action")
        .or_else(|| object.get("browser_action"))
        .or_else(|| object.get("page_action"));
    let action = action_value.and_then(Value::as_object).map(|action| ActionSpec {
        default_title: action.get("default_title").and_then(Value::as_str).map(str::to_string),
        default_popup: action.get("default_popup").and_then(Value::as_str).map(str::to_string),
        default_icon: action.get("default_icon").cloned().unwrap_or(Value::Null),
    });
    if object.contains_key("browser_action") || object.contains_key("page_action") {
        warnings.push("MV2 action translated to the shared action model".into());
    }
    let web_accessible_resources = normalize_resources(object, manifest_version)?;
    let content_security_policy = match object.get("content_security_policy") {
        Some(Value::String(value)) => json!({"extension_pages": value}),
        Some(value @ Value::Object(_)) => value.clone(),
        Some(_) => return Err("content_security_policy must be a string or object".into()),
        None => json!({}),
    };
    let chrome_compatibility_required = object.contains_key("minimum_chrome_version")
        || object.get("update_url").and_then(Value::as_str)
            .is_some_and(|url| url.contains("google.com/service/update2"));
    Ok(NormalizedManifest {
        manifest_version,
        name,
        version,
        extension_id,
        permissions,
        host_permissions,
        optional_permissions,
        optional_host_permissions,
        background,
        action,
        content_scripts: array(object.get("content_scripts")),
        web_accessible_resources,
        content_security_policy,
        chrome_compatibility_required,
        warnings,
        raw: value,
    })
}

fn normalize_background(
    object: &Map<String, Value>,
    manifest_version: u64,
    warnings: &mut Vec<String>,
) -> Result<Option<BackgroundSpec>, String> {
    let Some(background) = object.get("background").and_then(Value::as_object) else {
        return Ok(None);
    };
    if let Some(worker) = background.get("service_worker").and_then(Value::as_str) {
        return Ok(Some(BackgroundSpec {
            kind: BackgroundKind::ManifestV3Worker,
            scripts: Vec::new(),
            page: None,
            service_worker: Some(worker.into()),
            module: background.get("type").and_then(Value::as_str) == Some("module"),
            persistent: false,
        }));
    }
    if manifest_version == 3 {
        return Err("MV3 background requires service_worker".into());
    }
    let scripts = strings_vec(background.get("scripts"));
    let page = background.get("page").and_then(Value::as_str).map(str::to_string);
    if scripts.is_empty() && page.is_none() {
        return Err("MV2 background requires scripts or page".into());
    }
    warnings.push("MV2 background page retained with Firefox lifecycle semantics".into());
    Ok(Some(BackgroundSpec {
        kind: BackgroundKind::ManifestV2Page,
        scripts,
        page,
        service_worker: None,
        module: false,
        persistent: background.get("persistent").and_then(Value::as_bool).unwrap_or(true),
    }))
}

fn normalize_resources(object: &Map<String, Value>, manifest_version: u64) -> Result<Vec<Value>, String> {
    let Some(resources) = object.get("web_accessible_resources").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if manifest_version == 3 {
        if resources.iter().any(|value| !value.is_object()) {
            return Err("MV3 web_accessible_resources entries must be objects".into());
        }
        return Ok(resources.clone());
    }
    Ok(vec![json!({"resources": resources, "matches": ["<all_urls>"]})])
}

fn required_string(object: &Map<String, Value>, field: &str) -> Result<String, String> {
    object.get(field).and_then(Value::as_str).filter(|value| !value.trim().is_empty())
        .map(str::to_string).ok_or_else(|| format!("{field} is required"))
}

fn strings(value: Option<&Value>) -> BTreeSet<String> {
    strings_vec(value).into_iter().collect()
}

fn strings_vec(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).into_iter().flatten()
        .filter_map(Value::as_str).map(str::to_string).collect()
}

fn array(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

fn is_origin(value: &str) -> bool {
    value == "<all_urls>" || value.contains("://")
}
