use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub mod runtime;
pub mod dispatcher;
pub mod lifecycle;
pub mod permissions;
pub mod manifest;
pub mod package;
pub mod worlds;
pub mod dnr;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum ApiMemberKind {
    Function,
    Event,
    Property,
    Type,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ApiMember {
    pub namespace: String,
    pub name: String,
    pub kind: ApiMemberKind,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchemaRegistry {
    pub members: BTreeSet<ApiMember>,
    pub source_files: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CapabilityRegistry {
    pub implemented: BTreeSet<ApiMember>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CoverageReport {
    pub required: usize,
    pub implemented: usize,
    pub coverage_percent: f64,
    pub missing: Vec<ApiMember>,
    pub extra: Vec<ApiMember>,
}

impl SchemaRegistry {
    pub fn load_tree(root: &Path) -> Result<Self, String> {
        let mut files = Vec::new();
        collect_json(root, &mut files).map_err(|error| error.to_string())?;
        files.sort();
        let mut registry = Self::default();
        for path in files {
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let value: Value = serde_json::from_str(&strip_json_comments(&text))
                .map_err(|error| format!("{}: {error}", path.display()))?;
            registry.parse_schema(&value);
            registry.source_files.push(path);
        }
        Ok(registry)
    }

    fn parse_schema(&mut self, value: &Value) {
        let Some(namespaces) = value.as_array() else {
            return;
        };
        for schema in namespaces {
            let Some(namespace) = schema.get("namespace").and_then(Value::as_str) else {
                continue;
            };
            for (field, kind) in [
                ("functions", ApiMemberKind::Function),
                ("events", ApiMemberKind::Event),
                ("types", ApiMemberKind::Type),
            ] {
                if let Some(items) = schema.get(field).and_then(Value::as_array) {
                    for item in items {
                        let name = item
                            .get("name")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str);
                        if let Some(name) = name {
                            self.members.insert(ApiMember {
                                namespace: namespace.into(),
                                name: name.into(),
                                kind: kind.clone(),
                            });
                        }
                    }
                }
            }
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for name in properties.keys() {
                    self.members.insert(ApiMember {
                        namespace: namespace.into(),
                        name: name.into(),
                        kind: ApiMemberKind::Property,
                    });
                }
            }
        }
    }

    pub fn coverage(&self, capabilities: &CapabilityRegistry) -> CoverageReport {
        let missing = self
            .members
            .difference(&capabilities.implemented)
            .cloned()
            .collect::<Vec<_>>();
        let extra = capabilities
            .implemented
            .difference(&self.members)
            .cloned()
            .collect::<Vec<_>>();
        let implemented = self.members.len().saturating_sub(missing.len());
        CoverageReport {
            required: self.members.len(),
            implemented,
            coverage_percent: if self.members.is_empty() {
                100.0
            } else {
                implemented as f64 * 100.0 / self.members.len() as f64
            },
            missing,
            extra,
        }
    }
}

fn strip_json_comments(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            output.push(byte as char);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            output.push('"');
            index += 1;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len()
                && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
            {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
        } else {
            output.push(byte as char);
            index += 1;
        }
    }
    output
}

impl CapabilityRegistry {
    pub fn from_json(path: &Path) -> Result<Self, String> {
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())
    }
}

fn collect_json(root: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if root.is_file() {
        if root.extension().is_some_and(|extension| extension == "json") {
            output.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_json(&path, output)?;
        } else if path.extension().is_some_and(|extension| extension == "json") {
            output.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_firefox_schema_shape_and_reports_missing_members() {
        let schema = serde_json::json!([{
            "namespace": "tabs",
            "functions": [{"name": "create"}],
            "events": [{"name": "onUpdated"}],
            "properties": {"TAB_ID_NONE": {"value": -1}},
            "types": [{"id": "Tab", "type": "object"}]
        }]);
        let mut registry = SchemaRegistry::default();
        registry.parse_schema(&schema);
        assert_eq!(registry.members.len(), 4);
        let report = registry.coverage(&CapabilityRegistry::default());
        assert_eq!(report.required, 4);
        assert_eq!(report.implemented, 0);
        assert_eq!(report.missing.len(), 4);
    }

    #[test]
    fn exact_capabilities_reach_full_coverage() {
        let member = ApiMember {
            namespace: "runtime".into(),
            name: "getManifest".into(),
            kind: ApiMemberKind::Function,
        };
        let schema = SchemaRegistry {
            members: BTreeSet::from([member.clone()]),
            source_files: Vec::new(),
        };
        let capabilities = CapabilityRegistry {
            implemented: BTreeSet::from([member]),
        };
        assert_eq!(schema.coverage(&capabilities).coverage_percent, 100.0);
    }

    #[test]
    fn strips_mozilla_json_comments_without_touching_urls() {
        let input = r#"// schema note
[{"namespace":"x","description":"https://example.com/*","functions":[]}]"#;
        let value: Value = serde_json::from_str(&strip_json_comments(input)).unwrap();
        assert_eq!(value[0]["description"], "https://example.com/*");
    }
}
