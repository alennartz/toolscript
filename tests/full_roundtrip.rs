use code_mcp::codegen::generate::generate;
use code_mcp::codegen::manifest::Manifest;
use code_mcp::runtime::executor::{ExecutorConfig, ScriptExecutor};
use code_mcp::runtime::http::{AuthCredentialsMap, HttpHandler};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn test_full_roundtrip_with_mock_api() {
    // 1. Generate from petstore spec
    let output_dir = tempfile::tempdir().unwrap();
    generate(
        &["testdata/petstore.yaml".to_string()],
        output_dir.path(),
    )
    .await
    .unwrap();

    // 2. Load manifest
    let manifest_str =
        std::fs::read_to_string(output_dir.path().join("manifest.json")).unwrap();
    let manifest: Manifest = serde_json::from_str(&manifest_str).unwrap();

    // Verify manifest has expected content
    assert!(
        !manifest.functions.is_empty(),
        "manifest should have functions"
    );
    assert!(
        !manifest.schemas.is_empty(),
        "manifest should have schemas"
    );
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
            Err(anyhow::anyhow!("unexpected request: {} {}", method, url))
        }
    });

    let executor = ScriptExecutor::new(
        manifest,
        Arc::new(handler),
        ExecutorConfig::default(),
    );

    // 4. Execute scripts that use the generated SDK functions
    let auth = AuthCredentialsMap::new();

    // Test: get a single pet by id
    let result = executor
        .execute(
            "local pet = sdk.get_pet_by_id('pet-1')\nreturn pet.name",
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
            r#"
        local pets = sdk.list_pets()
        local first_pet = sdk.get_pet_by_id(pets[1].id)
        return {
            total = #pets,
            first_name = first_pet.name,
            first_status = first_pet.status
        }
    "#,
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
        local pet = sdk.get_pet_by_id("pet-1")
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
    generate(
        &["testdata/petstore.yaml".to_string()],
        output_dir.path(),
    )
    .await
    .unwrap();

    let sdk_dir = output_dir.path().join("sdk");
    for entry in std::fs::read_dir(&sdk_dir).unwrap() {
        let entry = entry.unwrap();
        if entry
            .path()
            .extension()
            .map(|e| e == "lua")
            .unwrap_or(false)
        {
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
                    content.contains("---") || content.contains("@"),
                    "File {} should contain Lua annotations",
                    entry.path().display()
                );
            }
        }
    }
}
