#![allow(clippy::unwrap_used, clippy::expect_used)]

use code_mcp::config::SpecInput;

#[tokio::test]
async fn test_generate_from_petstore() {
    let output_dir = tempfile::tempdir().unwrap();
    code_mcp::codegen::generate::generate(
        &[SpecInput {
            name: None,
            source: "testdata/petstore.yaml".to_string(),
        }],
        output_dir.path(),
    )
    .await
    .unwrap();

    // manifest.json exists and is valid JSON
    let manifest_path = output_dir.path().join("manifest.json");
    assert!(manifest_path.exists());
    let manifest: code_mcp::codegen::manifest::Manifest =
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
