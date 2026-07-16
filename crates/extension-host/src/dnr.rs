use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: u32,
    #[serde(default = "default_priority")]
    pub priority: u32,
    pub action: RuleAction,
    pub condition: RuleCondition,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuleAction {
    Block,
    Allow,
    AllowAllRequests,
    UpgradeScheme,
    Redirect { redirect: Redirect },
    ModifyHeaders,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Redirect {
    pub url: Option<String>,
    pub extension_path: Option<String>,
    pub regex_substitution: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleCondition {
    pub url_filter: Option<String>,
    pub regex_filter: Option<String>,
    #[serde(default)]
    pub resource_types: Vec<String>,
    #[serde(default)]
    pub excluded_resource_types: Vec<String>,
    #[serde(default)]
    pub request_domains: Vec<String>,
    #[serde(default)]
    pub excluded_request_domains: Vec<String>,
    #[serde(default)]
    pub initiator_domains: Vec<String>,
    #[serde(default)]
    pub excluded_initiator_domains: Vec<String>,
    pub domain_type: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompileWarning {
    pub rule_id: u32,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompiledRules {
    pub webkit_json: String,
    pub accepted_rule_ids: Vec<u32>,
    pub warnings: Vec<CompileWarning>,
}

fn default_priority() -> u32 { 1 }

pub fn compile(rules: &[Rule]) -> Result<CompiledRules, String> {
    let mut ordered = rules.to_vec();
    ordered.sort_by_key(|rule| (rule.priority, rule.id));
    let mut output = Vec::new();
    let mut accepted_rule_ids = Vec::new();
    let mut warnings = Vec::new();
    for rule in ordered {
        match compile_rule(&rule) {
            Ok(value) => {
                output.push(value);
                accepted_rule_ids.push(rule.id);
            }
            Err(message) => warnings.push(CompileWarning { rule_id: rule.id, message }),
        }
    }
    let webkit_json = serde_json::to_string(&output).map_err(|error| error.to_string())?;
    Ok(CompiledRules { webkit_json, accepted_rule_ids, warnings })
}

fn compile_rule(rule: &Rule) -> Result<Value, String> {
    let filter = match (&rule.condition.regex_filter, &rule.condition.url_filter) {
        (Some(regex), _) if !regex.is_empty() => regex.clone(),
        (_, Some(filter)) if !filter.is_empty() => url_filter_to_regex(filter),
        _ => ".*".into(),
    };
    let mut trigger = Map::from_iter([("url-filter".into(), Value::String(filter))]);
    add_strings(&mut trigger, "resource-type", map_resource_types(&rule.condition.resource_types));
    add_strings(&mut trigger, "unless-resource-type", map_resource_types(&rule.condition.excluded_resource_types));
    add_strings(&mut trigger, "if-domain", normalize_domains(&rule.condition.request_domains));
    add_strings(&mut trigger, "unless-domain", normalize_domains(&rule.condition.excluded_request_domains));
    add_strings(&mut trigger, "if-top-url", domains_to_top_urls(&rule.condition.initiator_domains));
    add_strings(&mut trigger, "unless-top-url", domains_to_top_urls(&rule.condition.excluded_initiator_domains));
    if let Some(kind) = &rule.condition.domain_type {
        let load_type = match kind.as_str() {
            "firstParty" => "first-party",
            "thirdParty" => "third-party",
            _ => return Err(format!("unknown domainType {kind}")),
        };
        trigger.insert("load-type".into(), json!([load_type]));
    }
    let action = match &rule.action {
        RuleAction::Block => json!({"type": "block"}),
        RuleAction::Allow | RuleAction::AllowAllRequests => json!({"type": "ignore-previous-rules"}),
        RuleAction::UpgradeScheme => json!({"type": "make-https"}),
        RuleAction::Redirect { redirect } => {
            if let Some(url) = &redirect.url {
                json!({"type": "redirect", "url": url})
            } else {
                return Err("extension-path and regex-substitution redirects require runtime interception".into());
            }
        }
        RuleAction::ModifyHeaders => return Err("modifyHeaders requires runtime interception".into()),
    };
    Ok(json!({"trigger": trigger, "action": action}))
}

fn add_strings(object: &mut Map<String, Value>, name: &str, values: Vec<String>) {
    if !values.is_empty() { object.insert(name.into(), json!(values)); }
}

fn map_resource_types(values: &[String]) -> Vec<String> {
    values.iter().filter_map(|value| Some(match value.as_str() {
        "main_frame" | "sub_frame" => "document",
        "stylesheet" => "style-sheet",
        "xmlhttprequest" | "ping" | "websocket" | "other" => "raw",
        "image" => "image",
        "script" => "script",
        "font" => "font",
        "media" => "media",
        "object" => "svg-document",
        _ => return None,
    }.to_string())).collect()
}

fn normalize_domains(values: &[String]) -> Vec<String> {
    values.iter().map(|domain| domain.trim_start_matches('.').to_ascii_lowercase()).collect()
}

fn domains_to_top_urls(values: &[String]) -> Vec<String> {
    normalize_domains(values).into_iter().flat_map(|domain| [
        format!("http://{domain}/*"), format!("https://{domain}/*"),
        format!("http://*.{domain}/*"), format!("https://*.{domain}/*"),
    ]).collect()
}

fn url_filter_to_regex(filter: &str) -> String {
    let domain_anchor = filter.starts_with("||");
    let start_anchor = !domain_anchor && filter.starts_with('|');
    let end_anchor = filter.ends_with('|') && !filter.ends_with("||");
    let body = filter
        .strip_prefix("||").or_else(|| filter.strip_prefix('|')).unwrap_or(filter)
        .strip_suffix('|').unwrap_or_else(|| filter.strip_prefix("||").or_else(|| filter.strip_prefix('|')).unwrap_or(filter));
    let mut output = if domain_anchor { "^https?://([^/]+\\.)?".into() }
        else if start_anchor { "^".into() } else { String::new() };
    for character in body.chars() {
        match character {
            '*' => output.push_str(".*"),
            '^' => output.push_str("(?:[^A-Za-z0-9_.%-]|$)"),
            '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '$' | '\\' => {
                output.push('\\'); output.push(character);
            }
            _ => output.push(character),
        }
    }
    if end_anchor { output.push('$'); }
    output
}
