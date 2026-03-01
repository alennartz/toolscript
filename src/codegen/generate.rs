use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::Path;

use anyhow::Result;
use openapiv3::OpenAPI;

use super::{annotations, manifest::Manifest, parser};
use crate::config::SpecInput;

/// Run the full code generation pipeline: parse specs, build manifest,
/// write manifest.json and Lua annotation files to disk.
///
/// `global_frozen` contains parameter names/values applied to all APIs.
/// `per_api_frozen` maps API name to per-API overrides.
pub async fn generate<S1, S2, S3>(
    specs: &[SpecInput],
    output_dir: &Path,
    global_frozen: &HashMap<String, String, S1>,
    per_api_frozen: &HashMap<String, HashMap<String, String, S3>, S2>,
) -> Result<()>
where
    S1: BuildHasher + Sync,
    S2: BuildHasher + Sync,
    S3: BuildHasher + Sync,
{
    std::fs::create_dir_all(output_dir)?;
    let sdk_dir = output_dir.join("sdk");
    std::fs::create_dir_all(&sdk_dir)?;

    let mut combined = Manifest {
        apis: vec![],
        functions: vec![],
        schemas: vec![],
        mcp_servers: vec![],
    };

    for spec_input in specs {
        let spec = if spec_input.source.starts_with("http://")
            || spec_input.source.starts_with("https://")
        {
            parser::load_spec_from_url(&spec_input.source).await?
        } else {
            parser::load_spec_from_file(Path::new(&spec_input.source))?
        };
        let api_name = spec_input
            .name
            .clone()
            .unwrap_or_else(|| derive_api_name(&spec));
        let mut manifest = parser::spec_to_manifest(&spec, &api_name)?;

        // Apply frozen parameter values from config.
        // Build the merged map manually: start with global, then layer per-API on top.
        let mut api_frozen: HashMap<String, String> = global_frozen
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(per) = per_api_frozen.get(&api_name) {
            api_frozen.extend(per.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        if !api_frozen.is_empty() {
            for func in &mut manifest.functions {
                for param in &mut func.parameters {
                    if let Some(value) = api_frozen.get(&param.name) {
                        param.frozen_value = Some(value.clone());
                    }
                }
            }
        }

        combined.apis.extend(manifest.apis);
        combined.functions.extend(manifest.functions);
        combined.schemas.extend(manifest.schemas);
    }

    // Write manifest.json
    let manifest_json = serde_json::to_string_pretty(&combined)?;
    std::fs::write(output_dir.join("manifest.json"), manifest_json)?;

    // Write annotation files
    let files = annotations::generate_annotation_files(&combined);
    for (filename, content) in files {
        std::fs::write(sdk_dir.join(filename), content)?;
    }

    Ok(())
}

/// Derive an API name from the spec's title, converting to a safe
/// lowercase identifier with underscores. Consecutive underscores are
/// collapsed and leading/trailing underscores are trimmed.
fn derive_api_name(spec: &OpenAPI) -> String {
    let raw = spec
        .info
        .title
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric(), "_");
    let collapsed: String = raw.chars().fold(String::new(), |mut acc, c| {
        if !(c == '_' && acc.ends_with('_')) {
            acc.push(c);
        }
        acc
    });
    collapsed.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::config::SpecInput;

    #[test]
    fn test_derive_api_name() {
        let spec = parser::load_spec_from_file(Path::new("testdata/petstore.yaml")).unwrap();
        let name = derive_api_name(&spec);
        assert_eq!(name, "petstore");
    }

    #[test]
    fn test_derive_api_name_with_spaces() {
        // Create a minimal spec with a multi-word title
        let yaml = r#"
openapi: "3.0.3"
info:
  title: "My Cool API"
  version: "1.0.0"
paths: {}
"#;
        let spec: OpenAPI = serde_yaml::from_str(yaml).unwrap();
        let name = derive_api_name(&spec);
        assert_eq!(name, "my_cool_api");
    }

    #[tokio::test]
    async fn test_generate_creates_output() {
        let output_dir = tempfile::tempdir().unwrap();
        let no_per_api: HashMap<String, HashMap<String, String>> = HashMap::new();
        generate(
            &[SpecInput {
                name: None,
                source: "testdata/petstore.yaml".to_string(),
            }],
            output_dir.path(),
            &HashMap::new(),
            &no_per_api,
        )
        .await
        .unwrap();

        // manifest.json should exist
        let manifest_path = output_dir.path().join("manifest.json");
        assert!(manifest_path.exists(), "manifest.json not created");

        // sdk/ directory should exist
        let sdk_dir = output_dir.path().join("sdk");
        assert!(sdk_dir.exists(), "sdk/ directory not created");

        // Should have at least one .luau file
        let luau_files: Vec<_> = std::fs::read_dir(&sdk_dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "luau"))
            .collect();
        assert!(!luau_files.is_empty(), "No .luau files in sdk/");
    }

    #[tokio::test]
    async fn test_generate_with_explicit_name() {
        let output_dir = tempfile::tempdir().unwrap();
        let no_per_api: HashMap<String, HashMap<String, String>> = HashMap::new();
        generate(
            &[SpecInput {
                name: Some("mystore".to_string()),
                source: "testdata/petstore.yaml".to_string(),
            }],
            output_dir.path(),
            &HashMap::new(),
            &no_per_api,
        )
        .await
        .unwrap();

        let manifest_str =
            std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap();
        let manifest: Manifest = serde_json::from_str(&manifest_str).unwrap();
        assert_eq!(manifest.apis[0].name, "mystore");
        for func in &manifest.functions {
            assert_eq!(func.api, "mystore");
        }
    }

    #[tokio::test]
    async fn test_generate_with_frozen_params() {
        let output_dir = tempfile::tempdir().unwrap();
        let mut frozen = HashMap::new();
        frozen.insert("limit".to_string(), "10".to_string());

        let mut per_api = HashMap::new();
        per_api.insert("petstore".to_string(), frozen);

        generate(
            &[SpecInput {
                name: Some("petstore".to_string()),
                source: "testdata/petstore.yaml".to_string(),
            }],
            output_dir.path(),
            &HashMap::new(),
            &per_api,
        )
        .await
        .unwrap();

        let manifest: Manifest = serde_json::from_str(
            &std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap(),
        )
        .unwrap();

        let list_pets = manifest
            .functions
            .iter()
            .find(|f| f.name == "list_pets")
            .unwrap();
        let limit_param = list_pets
            .parameters
            .iter()
            .find(|p| p.name == "limit")
            .unwrap();
        assert_eq!(limit_param.frozen_value, Some("10".to_string()));

        for param in &list_pets.parameters {
            if param.name != "limit" {
                assert_eq!(
                    param.frozen_value, None,
                    "param {} should not be frozen",
                    param.name
                );
            }
        }
    }
}
