#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use toolscript::codegen::manifest::FieldType;
use toolscript::config::SpecInput;

#[tokio::test]
async fn test_generate_from_petstore() {
    let output_dir = tempfile::tempdir().unwrap();
    let no_frozen: HashMap<String, HashMap<String, String>> = HashMap::new();
    toolscript::codegen::generate::generate(
        &[SpecInput {
            name: None,
            source: "testdata/petstore.yaml".to_string(),
        }],
        output_dir.path(),
        &HashMap::new(),
        &no_frozen,
    )
    .await
    .unwrap();

    // manifest.json exists and is valid JSON
    let manifest_path = output_dir.path().join("manifest.json");
    assert!(manifest_path.exists());
    let manifest: toolscript::codegen::manifest::Manifest =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert!(!manifest.functions.is_empty());
    assert!(!manifest.schemas.is_empty());

    // sdk/ directory has .luau files
    let sdk_dir = output_dir.path().join("sdk");
    assert!(sdk_dir.exists());
    let luau_files: Vec<_> = std::fs::read_dir(&sdk_dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "luau"))
        .collect();
    assert!(!luau_files.is_empty());

    // Verify luau files contain proper annotations (skip _meta.luau which is metadata-only)
    for entry in &luau_files {
        let content = std::fs::read_to_string(entry.path()).unwrap();
        let filename = entry.file_name();
        let filename_str = filename.to_string_lossy();
        if filename_str == "_meta.luau" {
            // Meta file should contain API metadata
            assert!(
                content.contains("-- API:") || content.contains("-- Version:"),
                "File {} doesn't contain metadata",
                entry.path().display()
            );
        } else {
            assert!(
                content.contains("function sdk.") || content.contains("export type"),
                "File {} doesn't contain annotations",
                entry.path().display()
            );
        }
    }
}

#[tokio::test]
async fn test_generate_from_advanced() {
    let output_dir = tempfile::tempdir().unwrap();
    let no_frozen: HashMap<String, HashMap<String, String>> = HashMap::new();
    toolscript::codegen::generate::generate(
        &[SpecInput {
            name: None,
            source: "testdata/advanced.yaml".to_string(),
        }],
        output_dir.path(),
        &HashMap::new(),
        &no_frozen,
    )
    .await
    .unwrap();

    // manifest.json exists and is valid JSON
    let manifest_path = output_dir.path().join("manifest.json");
    assert!(manifest_path.exists(), "manifest.json should exist");
    let manifest: toolscript::codegen::manifest::Manifest =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();

    // Verify Resource schema exists with allOf-merged fields
    let resource = manifest
        .schemas
        .iter()
        .find(|s| s.name == "Resource")
        .expect("Resource schema should exist in manifest");
    let field_names: Vec<&str> = resource.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(
        field_names.contains(&"id"),
        "Resource should have id from BaseResource. Got: {field_names:?}"
    );
    assert!(
        field_names.contains(&"created_at"),
        "Resource should have created_at from BaseResource. Got: {field_names:?}"
    );
    assert!(
        field_names.contains(&"name"),
        "Resource should have name from inline. Got: {field_names:?}"
    );
    assert!(
        field_names.contains(&"metadata"),
        "Resource should have metadata. Got: {field_names:?}"
    );

    // Verify metadata is a Map type
    let metadata = resource
        .fields
        .iter()
        .find(|f| f.name == "metadata")
        .unwrap();
    assert_eq!(
        metadata.field_type,
        FieldType::Map {
            value: Box::new(FieldType::String)
        },
        "metadata should be Map<string>"
    );

    // sdk/ directory has .luau files
    let sdk_dir = output_dir.path().join("sdk");
    assert!(sdk_dir.exists(), "sdk/ directory should exist");
    let luau_files: Vec<_> = std::fs::read_dir(&sdk_dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "luau"))
        .collect();
    assert!(!luau_files.is_empty(), "Should have .luau files");

    // Find the main annotation file (not _meta) and verify content
    let annotation_file = luau_files
        .iter()
        .find(|e| {
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            name_str != "_meta.luau"
        })
        .expect("Should have at least one non-meta .luau file");

    let content = std::fs::read_to_string(annotation_file.path()).unwrap();

    // Map type syntax: { [string]: string }
    assert!(
        content.contains("[string]:"),
        "Luau file should contain Map type syntax [string]:. Got:\n{content}"
    );

    // Nullable fields get ? markers
    assert!(
        content.contains('?'),
        "Luau file should contain ? for nullable/optional fields. Got:\n{content}"
    );

    // Format comments: (uuid), (date-time), etc.
    assert!(
        content.contains("(uuid)") || content.contains("(date-time)"),
        "Luau file should contain format comments like (uuid) or (date-time). Got:\n{content}"
    );
}

#[tokio::test]
async fn test_frozen_params_end_to_end() {
    let output_dir = tempfile::tempdir().unwrap();
    let mut per_api_frozen = HashMap::new();
    let mut petstore_frozen = HashMap::new();
    petstore_frozen.insert("limit".to_string(), "25".to_string());
    per_api_frozen.insert("petstore".to_string(), petstore_frozen);

    toolscript::codegen::generate::generate(
        &[SpecInput {
            name: Some("petstore".to_string()),
            source: "testdata/petstore.yaml".to_string(),
        }],
        output_dir.path(),
        &HashMap::new(),
        &per_api_frozen,
    )
    .await
    .unwrap();

    // Check manifest has frozen_value set
    let manifest: toolscript::codegen::manifest::Manifest = serde_json::from_str(
        &std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap(),
    )
    .unwrap();

    let list_pets = manifest
        .functions
        .iter()
        .find(|f| f.name == "list_pets")
        .unwrap();
    let limit = list_pets
        .parameters
        .iter()
        .find(|p| p.name == "limit")
        .unwrap();
    assert_eq!(limit.frozen_value, Some("25".to_string()));

    // Check that Luau function signature doesn't mention the frozen param
    let sdk_dir = output_dir.path().join("sdk");
    for entry in std::fs::read_dir(&sdk_dir).unwrap() {
        let entry = entry.unwrap();
        let content = std::fs::read_to_string(entry.path()).unwrap();
        for line in content.lines() {
            if line.contains("function sdk.list_pets") {
                assert!(
                    !line.contains("limit"),
                    "Frozen param 'limit' should not appear in Luau function signature. Got:\n{line}"
                );
            }
        }
    }
}
