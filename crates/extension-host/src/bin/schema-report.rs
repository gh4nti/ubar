use std::path::Path;
use ubar_extension_host::{CapabilityRegistry, SchemaRegistry};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let schemas = args
        .next()
        .ok_or("usage: schema-report <schema-directory> <capabilities.json>")?;
    let capabilities = args
        .next()
        .ok_or("usage: schema-report <schema-directory> <capabilities.json>")?;
    let summary_only = args.any(|argument| argument == "--summary");
    let registry = SchemaRegistry::load_tree(Path::new(&schemas))?;
    let capabilities = CapabilityRegistry::from_json(Path::new(&capabilities))?;
    let report = registry.coverage(&capabilities);
    if summary_only {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "required": report.required,
                "implemented": report.implemented,
                "coverage_percent": report.coverage_percent,
                "missing": report.missing.len(),
                "extra": report.extra.len(),
            }))
            .map_err(|error| error.to_string())?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    }
    if report.missing.is_empty() {
        Ok(())
    } else {
        Err(format!("{} Firefox API members are missing", report.missing.len()))
    }
}
