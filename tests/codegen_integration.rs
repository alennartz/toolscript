#[tokio::test]
async fn test_generate_from_petstore() {
    let output_dir = tempfile::tempdir().unwrap();
    code_mcp::codegen::generate::generate(
        &["testdata/petstore.yaml".to_string()],
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

    // sdk/ directory has .lua files
    let sdk_dir = output_dir.path().join("sdk");
    assert!(sdk_dir.exists());
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
    assert!(!lua_files.is_empty());

    // Verify lua files contain proper annotations (skip _meta.lua which is metadata-only)
    for entry in &lua_files {
        let content = std::fs::read_to_string(entry.path()).unwrap();
        let filename = entry.file_name();
        let filename_str = filename.to_string_lossy();
        if filename_str == "_meta.lua" {
            // Meta file should contain API metadata
            assert!(
                content.contains("-- API:") || content.contains("-- Version:"),
                "File {} doesn't contain metadata",
                entry.path().display()
            );
        } else {
            assert!(
                content.contains("function sdk.") || content.contains("@class"),
                "File {} doesn't contain annotations",
                entry.path().display()
            );
        }
    }
}
