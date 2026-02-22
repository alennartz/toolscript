use std::path::Path;

use anyhow::Result;
use openapiv3::OpenAPI;

use super::{annotations, manifest::Manifest, parser};

/// Run the full code generation pipeline: parse specs, build manifest,
/// write manifest.json and Lua annotation files to disk.
pub async fn generate(specs: &[String], output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let sdk_dir = output_dir.join("sdk");
    std::fs::create_dir_all(&sdk_dir)?;

    let mut combined = Manifest {
        apis: vec![],
        functions: vec![],
        schemas: vec![],
    };

    for spec_source in specs {
        let spec = if spec_source.starts_with("http://") || spec_source.starts_with("https://") {
            parser::load_spec_from_url(spec_source).await?
        } else {
            parser::load_spec_from_file(Path::new(spec_source))?
        };
        let api_name = derive_api_name(&spec);
        let manifest = parser::spec_to_manifest(&spec, &api_name)?;
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
        if c == '_' && acc.ends_with('_') {
            acc
        } else {
            acc.push(c);
            acc
        }
    });
    collapsed.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        generate(
            &["testdata/petstore.yaml".to_string()],
            output_dir.path(),
        )
        .await
        .unwrap();

        // manifest.json should exist
        let manifest_path = output_dir.path().join("manifest.json");
        assert!(manifest_path.exists(), "manifest.json not created");

        // sdk/ directory should exist
        let sdk_dir = output_dir.path().join("sdk");
        assert!(sdk_dir.exists(), "sdk/ directory not created");

        // Should have at least one .lua file
        let lua_files: Vec<_> = std::fs::read_dir(&sdk_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "lua")
                    .unwrap_or(false)
            })
            .collect();
        assert!(!lua_files.is_empty(), "No .lua files in sdk/");
    }
}
