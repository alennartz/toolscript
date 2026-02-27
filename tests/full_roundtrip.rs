#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use toolscript::codegen::generate::generate;
use toolscript::codegen::manifest::Manifest;
use toolscript::config::SpecInput;
use toolscript::runtime::executor::{ExecutorConfig, ScriptExecutor};
use toolscript::runtime::http::{AuthCredentialsMap, HttpHandler};
use toolscript::runtime::mcp_client::McpClientManager;

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn test_full_roundtrip_with_mock_api() {
    // 1. Generate from petstore spec
    let output_dir = tempfile::tempdir().unwrap();
    let no_frozen: HashMap<String, HashMap<String, String>> = HashMap::new();
    generate(
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

    // 2. Load manifest
    let manifest_str = std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap();
    let manifest: Manifest = serde_json::from_str(&manifest_str).unwrap();

    // Verify manifest has expected content
    assert!(
        !manifest.functions.is_empty(),
        "manifest should have functions"
    );
    assert!(!manifest.schemas.is_empty(), "manifest should have schemas");
    assert!(!manifest.apis.is_empty(), "manifest should have apis");

    // 3. Create executor with mock HTTP handler
    // The actual generated functions are:
    //   list_pets     - GET /pets (optional query params: status, limit)
    //   create_pet    - POST /pets (body)
    //   get_pet_by_id - GET /pets/{petId} (path param: petId)
    let handler = HttpHandler::mock(|method, url, _query, _body| {
        if method == "GET" && url.ends_with("/pets") {
            Ok(serde_json::json!([
                {"id": "pet-1", "name": "Buddy", "status": "available", "tag": "dog"},
                {"id": "pet-2", "name": "Max", "status": "pending", "tag": "cat"}
            ]))
        } else if method == "GET" && url.contains("/pets/") {
            // Extract pet ID from URL
            let id = url.rsplit('/').next().unwrap_or("unknown");
            Ok(serde_json::json!({
                "id": id,
                "name": "Buddy",
                "status": "available",
                "tag": "dog"
            }))
        } else if method == "POST" && url.ends_with("/pets") {
            Ok(serde_json::json!({
                "id": "pet-3",
                "name": "New Pet",
                "status": "available"
            }))
        } else {
            Err(anyhow::anyhow!("unexpected request: {method} {url}"))
        }
    });

    let executor = ScriptExecutor::new(
        manifest,
        Arc::new(handler),
        ExecutorConfig::default(),
        None,
        Arc::new(McpClientManager::empty()),
    );

    // 4. Execute scripts that use the generated SDK functions
    let auth = AuthCredentialsMap::new();

    // Test: get a single pet by id
    let result = executor
        .execute(
            "local pet = sdk.get_pet_by_id({ petId = 'pet-1' })\nreturn pet.name",
            &auth,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.result, serde_json::json!("Buddy"));

    // Test: list pets and count them
    let result = executor
        .execute("local pets = sdk.list_pets()\nreturn #pets", &auth, None)
        .await
        .unwrap();
    assert_eq!(result.result, serde_json::json!(2));

    // Test: chain multiple calls
    let result = executor
        .execute(
            r"
        local pets = sdk.list_pets()
        local first_pet = sdk.get_pet_by_id({ petId = pets[1].id })
        return {
            total = #pets,
            first_name = first_pet.name,
            first_status = first_pet.status
        }
    ",
            &auth,
            None,
        )
        .await
        .unwrap();

    let r = &result.result;
    assert_eq!(r["total"], 2);
    assert_eq!(r["first_name"], "Buddy");
    assert_eq!(r["first_status"], "available");
    assert!(result.stats.api_calls >= 2);

    // Test: logs are captured
    let result = executor
        .execute(
            r#"
        print("starting")
        local pet = sdk.get_pet_by_id({ petId = "pet-1" })
        print("got pet: " .. pet.name)
        return pet.id
    "#,
            &auth,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.result, serde_json::json!("pet-1"));
    assert!(result.logs.len() >= 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_generated_lua_annotations_are_valid() {
    // Generate and verify the Lua annotation files have proper content
    let output_dir = tempfile::tempdir().unwrap();
    let no_frozen: HashMap<String, HashMap<String, String>> = HashMap::new();
    generate(
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

    let sdk_dir = output_dir.path().join("sdk");
    for entry in std::fs::read_dir(&sdk_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|e| e == "lua") {
            let content = std::fs::read_to_string(entry.path()).unwrap();
            let filename = entry.file_name();
            let filename_str = filename.to_string_lossy();
            if filename_str == "_meta.lua" {
                // Meta file should contain API metadata comments
                assert!(
                    content.contains("-- API:") || content.contains("-- Version:"),
                    "File {} should contain API metadata",
                    entry.path().display()
                );
            } else {
                // SDK files should have proper LuaLS annotation markers
                assert!(
                    content.contains("---") || content.contains('@'),
                    "File {} should contain Lua annotations",
                    entry.path().display()
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_roundtrip_with_named_spec() {
    let output_dir = tempfile::tempdir().unwrap();
    let no_frozen: HashMap<String, HashMap<String, String>> = HashMap::new();
    generate(
        &[SpecInput {
            name: Some("mystore".to_string()),
            source: "testdata/petstore.yaml".to_string(),
        }],
        output_dir.path(),
        &HashMap::new(),
        &no_frozen,
    )
    .await
    .unwrap();

    let manifest_str = std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap();
    let manifest: Manifest = serde_json::from_str(&manifest_str).unwrap();

    // API name should be the user-chosen name
    assert_eq!(manifest.apis[0].name, "mystore");

    // Functions should reference the user-chosen name
    for func in &manifest.functions {
        assert_eq!(func.api, "mystore");
    }

    // Execute a script to verify the SDK still works with the custom name
    let handler = HttpHandler::mock(|method, url, _query, _body| {
        if method == "GET" && url.contains("/pets/") {
            Ok(serde_json::json!({"id": "pet-1", "name": "Buddy", "status": "available"}))
        } else {
            Err(anyhow::anyhow!("unexpected: {method} {url}"))
        }
    });

    let executor = ScriptExecutor::new(
        manifest,
        Arc::new(handler),
        ExecutorConfig::default(),
        None,
        Arc::new(McpClientManager::empty()),
    );
    let auth = AuthCredentialsMap::new();

    let result = executor
        .execute(
            "local pet = sdk.get_pet_by_id({ petId = 'pet-1' })\nreturn pet.name",
            &auth,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.result, serde_json::json!("Buddy"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_file_save_roundtrip() {
    let output_dir = tempfile::tempdir().unwrap();
    let spec_output = tempfile::tempdir().unwrap();
    let no_frozen: HashMap<String, HashMap<String, String>> = HashMap::new();

    generate(
        &[SpecInput {
            name: None,
            source: "testdata/petstore.yaml".to_string(),
        }],
        spec_output.path(),
        &HashMap::new(),
        &no_frozen,
    )
    .await
    .unwrap();

    let manifest_str = std::fs::read_to_string(spec_output.path().join("manifest.json")).unwrap();
    let manifest: Manifest = serde_json::from_str(&manifest_str).unwrap();

    let handler = HttpHandler::mock(|method, url, _query, _body| {
        if method == "GET" && url.ends_with("/pets") {
            Ok(serde_json::json!([
                {"id": "1", "name": "Buddy", "status": "available"},
                {"id": "2", "name": "Max", "status": "pending"}
            ]))
        } else {
            Err(anyhow::anyhow!("unexpected: {method} {url}"))
        }
    });

    let executor = ScriptExecutor::new(
        manifest,
        Arc::new(handler),
        ExecutorConfig::default(),
        Some(toolscript::runtime::executor::OutputConfig {
            dir: output_dir.path().to_path_buf(),
            max_bytes: 50 * 1024 * 1024,
        }),
        Arc::new(McpClientManager::empty()),
    );
    let auth = AuthCredentialsMap::new();

    let result = executor
        .execute(
            r#"
            local pets = sdk.list_pets()
            local csv = "id,name,status\n"
            for _, p in ipairs(pets) do
                csv = csv .. p.id .. "," .. p.name .. "," .. p.status .. "\n"
            end
            file.save("pets.csv", csv)
            file.save("summary.json", json.encode({ count = #pets }))
            return "saved"
        "#,
            &auth,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.result, serde_json::json!("saved"));
    assert_eq!(result.files_written.len(), 2);

    // Verify CSV file
    let csv = std::fs::read_to_string(output_dir.path().join("pets.csv")).unwrap();
    assert!(csv.contains("Buddy"));
    assert!(csv.contains("Max"));

    // Verify JSON file
    let json_str = std::fs::read_to_string(output_dir.path().join("summary.json")).unwrap();
    let summary: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(summary["count"], 2);
}
