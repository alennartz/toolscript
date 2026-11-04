//! Behavioral tests for `OpenAPI` 3.1/3.2 support.
//!
//! The fixtures mirror the 3.0 twins (`petstore.yaml`, `advanced.yaml`) with
//! the same API expressed in 2020-12 idioms: `type: [T, "null"]` nullability
//! and one 3.1 fixture variant each for `const` fields and boolean schemas.
//! The 3.2 fixtures add a `query` method and a webhook to prove both degrade.
//!
//! Contracts under test (from the plan's Architecture section):
//! - version routing (`3.0.*` -> openapiv3, `3.1.*`/`3.2.*` -> oas3,
//!   Swagger 2.0 -> friendly error)
//! - equivalence: a 3.1/3.2 fixture generates the same manifest content as its
//!   3.0 twin (names, fields, types, nullability, formats, enums, docs)
//! - the accepted differences: nested inline object field order becomes
//!   document order, and 3.2-only constructs (query method, webhooks) are
//!   excluded from generation

#![allow(clippy::unwrap_used, clippy::expect_used)]

use toolscript::codegen::manifest::{AuthConfig, FieldType, Manifest, ParamType};
use toolscript::codegen::parser::{load_spec_from_file, spec_to_manifest};

fn manifest_from(path: &str, name: &str) -> Manifest {
    let doc = load_spec_from_file(std::path::Path::new(path)).unwrap();
    spec_to_manifest(&doc, name, None).unwrap()
}

fn function<'a>(
    manifest: &'a Manifest,
    name: &str,
) -> &'a toolscript::codegen::manifest::FunctionDef {
    manifest
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("function {name} missing"))
}

fn schema<'a>(manifest: &'a Manifest, name: &str) -> &'a toolscript::codegen::manifest::SchemaDef {
    manifest
        .schemas
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("schema {name} missing"))
}

fn field<'a>(
    schema_def: &'a toolscript::codegen::manifest::SchemaDef,
    name: &str,
) -> &'a toolscript::codegen::manifest::FieldDef {
    schema_def
        .fields
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("field {name} missing"))
}

// ---------------------------------------------------------------------------
// Version routing
// ---------------------------------------------------------------------------

#[test]
fn test_31_petstore_spec_parses_and_generates() {
    let manifest = manifest_from("testdata/petstore_31.yaml", "petstore");
    assert_eq!(manifest.apis.len(), 1);
    assert_eq!(manifest.apis[0].base_url, "https://petstore.example.com/v1");
    assert_eq!(manifest.apis[0].version.as_deref(), Some("1.0.0"));
}

#[test]
fn test_32_petstore_spec_parses_and_generates() {
    let manifest = manifest_from("testdata/petstore_32.yaml", "petstore");
    assert_eq!(manifest.apis.len(), 1);
    assert_eq!(manifest.apis[0].base_url, "https://petstore.example.com/v1");
}

#[test]
fn test_31_and_32_petstore_functions_match_30_twin() {
    let m30 = manifest_from("testdata/petstore.yaml", "petstore");
    let m31 = manifest_from("testdata/petstore_31.yaml", "petstore");
    let m32 = manifest_from("testdata/petstore_32.yaml", "petstore");

    let mut names_30: Vec<&str> = m30.functions.iter().map(|f| f.name.as_str()).collect();
    let mut names_31: Vec<&str> = m31.functions.iter().map(|f| f.name.as_str()).collect();
    let mut names_32: Vec<&str> = m32.functions.iter().map(|f| f.name.as_str()).collect();
    names_30.sort_unstable();
    names_31.sort_unstable();
    names_32.sort_unstable();

    assert_eq!(
        names_30, names_31,
        "3.1 twin must expose the same functions"
    );
    assert_eq!(
        names_30, names_32,
        "3.2 twin must expose the same functions"
    );
    assert!(names_30.contains(&"list_pets"));
    assert!(names_30.contains(&"create_pet"));
    assert!(names_30.contains(&"get_pet_by_id"));
}

#[test]
fn test_32_query_method_and_webhooks_are_excluded() {
    let manifest = manifest_from("testdata/petstore_32.yaml", "petstore");
    let names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(
        !names.contains(&"search_pets"),
        "3.2 query method must not produce a function, got: {names:?}"
    );
    assert!(
        !names.contains(&"pet_created_webhook"),
        "webhooks must not produce functions, got: {names:?}"
    );
    assert_eq!(names.len(), 3, "only get/post/put/patch/delete operations");
}

// ---------------------------------------------------------------------------
// 2020-12 idioms -> identical manifest content
// ---------------------------------------------------------------------------

#[test]
fn test_31_type_array_null_renders_nullable_string_like_30() {
    let m30 = manifest_from("testdata/petstore.yaml", "petstore");
    let m31 = manifest_from("testdata/petstore_31.yaml", "petstore");

    let tag_30 = field(schema(&m30, "Pet"), "tag");
    let tag_31 = field(schema(&m31, "Pet"), "tag");
    assert!(!tag_30.required && !tag_31.required);
    assert!(tag_30.nullable && tag_31.nullable);
    assert_eq!(tag_30.field_type, tag_31.field_type);
    assert_eq!(tag_31.field_type, FieldType::String);

    // Same nullability treatment on the optional request-body field.
    let new_tag_30 = field(schema(&m30, "NewPet"), "tag");
    let new_tag_31 = field(schema(&m31, "NewPet"), "tag");
    assert!(new_tag_30.nullable && new_tag_31.nullable);
    assert_eq!(new_tag_30.field_type, new_tag_31.field_type);
}

#[test]
fn test_31_twin_matches_30_schemas_field_by_field() {
    let m30 = manifest_from("testdata/petstore.yaml", "petstore");
    let m31 = manifest_from("testdata/petstore_31.yaml", "petstore");

    let pet_30 = schema(&m30, "Pet");
    let pet_31 = schema(&m31, "Pet");
    assert_eq!(pet_30.description, pet_31.description);

    let names_30: Vec<&str> = pet_30.fields.iter().map(|f| f.name.as_str()).collect();
    let names_31: Vec<&str> = pet_31.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names_30, names_31, "field order follows document order");

    for (f30, f31) in pet_30.fields.iter().zip(pet_31.fields.iter()) {
        assert_eq!(f30.name, f31.name);
        assert_eq!(f30.field_type, f31.field_type, "field {}", f30.name);
        assert_eq!(f30.required, f31.required, "field {}", f30.name);
        assert_eq!(f30.nullable, f31.nullable, "field {}", f30.name);
        assert_eq!(f30.format, f31.format, "field {}", f30.name);
        assert_eq!(f30.enum_values, f31.enum_values, "field {}", f30.name);
        assert_eq!(f30.description, f31.description, "field {}", f30.name);
    }
}

#[test]
fn test_31_twin_matches_30_operations_in_detail() {
    let m30 = manifest_from("testdata/petstore.yaml", "petstore");
    let m31 = manifest_from("testdata/petstore_31.yaml", "petstore");

    for name in ["list_pets", "create_pet", "get_pet_by_id"] {
        let f30 = function(&m30, name);
        let f31 = function(&m31, name);
        assert_eq!(f30.method, f31.method, "{name}");
        assert_eq!(f30.path, f31.path, "{name}");
        assert_eq!(f30.tag, f31.tag, "{name}");
        assert_eq!(f30.summary, f31.summary, "{name}");
        assert_eq!(f30.description, f31.description, "{name}");
        assert_eq!(f30.deprecated, f31.deprecated, "{name}");
        assert_eq!(f30.response_schema, f31.response_schema, "{name}");

        let params_30: Vec<&str> = f30.parameters.iter().map(|p| p.name.as_str()).collect();
        let params_31: Vec<&str> = f31.parameters.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(params_30, params_31, "{name} parameters");

        for (p30, p31) in f30.parameters.iter().zip(f31.parameters.iter()) {
            assert_eq!(p30.location, p31.location, "{name}/{}", p30.name);
            assert_eq!(p30.param_type, p31.param_type, "{name}/{}", p30.name);
            assert_eq!(p30.required, p31.required, "{name}/{}", p30.name);
            assert_eq!(p30.description, p31.description, "{name}/{}", p30.name);
            assert_eq!(p30.default, p31.default, "{name}/{}", p30.name);
            assert_eq!(p30.enum_values, p31.enum_values, "{name}/{}", p30.name);
        }

        assert_eq!(
            f30.request_body.is_some(),
            f31.request_body.is_some(),
            "{name}"
        );
        if let (Some(b30), Some(b31)) = (&f30.request_body, &f31.request_body) {
            assert_eq!(b30.content_type, b31.content_type, "{name}");
            assert_eq!(b30.schema, b31.schema, "{name}");
            assert_eq!(b30.required, b31.required, "{name}");
            assert_eq!(b30.description, b31.description, "{name}");
        }
    }
}

#[test]
fn test_31_advanced_twin_matches_30_allof_formats_and_nullability() {
    let m30 = manifest_from("testdata/advanced.yaml", "advanced");
    let m31 = manifest_from("testdata/advanced_31.yaml", "advanced");

    let r30 = schema(&m30, "Resource");
    let r31 = schema(&m31, "Resource");
    let names_30: Vec<&str> = r30.fields.iter().map(|f| f.name.as_str()).collect();
    let names_31: Vec<&str> = r31.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names_30, names_31);

    for (f30, f31) in r30.fields.iter().zip(r31.fields.iter()) {
        assert_eq!(f30.name, f31.name);
        assert_eq!(f30.field_type, f31.field_type, "field {}", f30.name);
        assert_eq!(f30.required, f31.required, "field {}", f30.name);
        assert_eq!(f30.nullable, f31.nullable, "field {}", f30.name);
        assert_eq!(f30.format, f31.format, "field {}", f30.name);
        assert_eq!(f30.description, f31.description, "field {}", f30.name);
    }

    // Spot-checks that exercise the 2020-12 expressions in the 3.1 fixture:
    // `format: uuid` survives, type-array nullability renders nullable.
    let id_31 = field(r31, "id");
    assert_eq!(id_31.format.as_deref(), Some("uuid"));
    assert!(id_31.required);
    let updated_at_31 = field(r31, "updated_at");
    assert!(updated_at_31.nullable);
    assert_eq!(updated_at_31.format.as_deref(), Some("date-time"));

    // The Error schema keeps its integer format through the 3.1 path.
    let error_31 = schema(&m31, "Error");
    assert_eq!(field(error_31, "code").format.as_deref(), Some("int32"));
    assert_eq!(field(error_31, "code").field_type, FieldType::Integer);
}

#[test]
fn test_31_twin_matches_30_auth() {
    let m30 = manifest_from("testdata/petstore.yaml", "petstore");
    let m31 = manifest_from("testdata/petstore_31.yaml", "petstore");
    assert_eq!(
        m30.apis[0].auth.as_ref().unwrap(),
        m31.apis[0].auth.as_ref().unwrap()
    );
    assert_eq!(
        *m31.apis[0].auth.as_ref().unwrap(),
        AuthConfig::Bearer {
            header: "Authorization".to_string(),
            prefix: "Bearer ".to_string(),
        }
    );
}

#[test]
fn test_32_twin_matches_31_manifest() {
    let m31 = manifest_from("testdata/petstore_31.yaml", "petstore");
    let m32 = manifest_from("testdata/petstore_32.yaml", "petstore");
    assert_eq!(m31, m32, "3.2 core must generate identically to 3.1");
}

#[test]
fn test_31_parameters_keep_types_defaults_and_formats() {
    let m31 = manifest_from("testdata/petstore_31.yaml", "petstore");
    let list = function(&m31, "list_pets");
    assert_eq!(list.parameters.len(), 2);

    let status = list.parameters.iter().find(|p| p.name == "status").unwrap();
    assert_eq!(status.param_type, ParamType::String);
    assert!(!status.required);
    let enums = status.enum_values.as_ref().unwrap();
    assert_eq!(enums.len(), 3);
    assert!(enums.contains(&"available".to_string()));

    let limit = list.parameters.iter().find(|p| p.name == "limit").unwrap();
    assert_eq!(limit.param_type, ParamType::Integer);
    assert_eq!(limit.default, Some(serde_json::json!(20)));

    let get_pet = function(&m31, "get_pet_by_id");
    assert_eq!(get_pet.parameters.len(), 1);
    assert!(get_pet.parameters[0].required);
}

#[test]
fn test_31_response_and_request_body_schemas_resolve() {
    let m31 = manifest_from("testdata/petstore_31.yaml", "petstore");
    let list = function(&m31, "list_pets");
    assert_eq!(list.response_schema.as_deref(), Some("Pet"));

    let create = function(&m31, "create_pet");
    let body = create.request_body.as_ref().unwrap();
    assert_eq!(body.content_type, "application/json");
    assert_eq!(body.schema, "NewPet");
    assert!(body.required);
    assert_eq!(create.response_schema.as_deref(), Some("Pet"));
}

// ---------------------------------------------------------------------------
// 3.1-only idioms that have no 3.0 twin representation
// ---------------------------------------------------------------------------

#[test]
fn test_31_const_field_becomes_single_value_enum() {
    // A 3.1 spec can pin a field with `const`; it must surface as a
    // single-value enum, exactly like a one-element `enum` list.
    let yaml = r#"
openapi: "3.1.0"
info:
  title: Const API
  version: "1.0.0"
paths: {}
components:
  schemas:
    Event:
      type: object
      required: [kind]
      properties:
        kind:
          const: created
"#;
    let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
    let manifest = spec_to_manifest(&doc, "constapi", None).unwrap();
    let event = schema(&manifest, "Event");
    let kind = field(event, "kind");
    assert!(kind.required);
    assert_eq!(
        kind.enum_values,
        Some(vec!["created".to_string()]),
        "const must surface as a single-value enum"
    );
    assert_eq!(kind.field_type, FieldType::String);
}

#[test]
fn test_31_boolean_schemas_are_permissive_unknown() {
    // `true`/`false` schema nodes carry no type information; they must not
    // break extraction and fall back to the unknown-type behavior.
    let yaml = r#"
openapi: "3.1.0"
info:
  title: Boolean Schema API
  version: "1.0.0"
paths: {}
components:
  schemas:
    Holder:
      type: object
      properties:
        anything:
          - true
"#;
    let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
    let manifest = spec_to_manifest(&doc, "boolschema", None).unwrap();
    let holder = schema(&manifest, "Holder");
    let anything = field(holder, "anything");
    assert_eq!(
        anything.field_type,
        FieldType::String,
        "unknown falls back to string"
    );
}

// ---------------------------------------------------------------------------
// Document-order field iteration (the accepted output change)
// ---------------------------------------------------------------------------

#[test]
fn test_nested_inline_object_fields_keep_document_order() {
    // With preserve_order enabled, inline object fields must follow the
    // order written in the spec, not alphabetical order.
    let yaml = r#"
openapi: "3.0.3"
info:
  title: Order API
  version: "1.0.0"
paths: {}
components:
  schemas:
    Config:
      type: object
      required: [settings]
      properties:
        settings:
          type: object
          required: [zulu]
          properties:
            zulu:
              type: string
            alpha:
              type: integer
"#;
    let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
    let manifest = spec_to_manifest(&doc, "order", None).unwrap();
    let config = schema(&manifest, "Config");
    let settings = field(config, "settings");
    match &settings.field_type {
        FieldType::InlineObject { fields } => {
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(
                names,
                vec!["zulu", "alpha"],
                "inline object fields must keep document order"
            );
            let zulu = fields.iter().find(|f| f.name == "zulu").unwrap();
            assert!(zulu.required);
            let alpha = fields.iter().find(|f| f.name == "alpha").unwrap();
            assert!(!alpha.required);
        }
        other => panic!("Expected InlineObject, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Reserved FieldType variants stay unreachable from valid input
// ---------------------------------------------------------------------------

#[test]
fn test_oneof_and_anyof_variants_exist_but_are_never_produced() {
    // The variants are reserved for the follow-up; valid input in this pass
    // never produces them. Construct them directly to pin the shape.
    let one_of = FieldType::OneOf {
        schemas: vec!["Cat".to_string(), "Dog".to_string()],
    };
    let any_of = FieldType::AnyOf {
        schemas: vec!["Cat".to_string()],
    };
    assert_ne!(one_of, any_of);
    assert_eq!(
        one_of,
        FieldType::OneOf {
            schemas: vec!["Cat".to_string(), "Dog".to_string()]
        }
    );

    // oneOf/anyOf schemas in a spec must not produce these variants yet.
    let yaml = r##"
openapi: "3.1.0"
info:
  title: Union API
  version: "1.0.0"
paths: {}
components:
  schemas:
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
    Filter:
      type: object
      properties:
        value:
          anyOf:
            - type: string
            - type: integer
    Cat:
      type: object
      properties:
        meow:
          type: string
    Dog:
      type: object
      properties:
        woof:
          type: string
"##;
    let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
    let manifest = spec_to_manifest(&doc, "unions", None).unwrap();
    for s in &manifest.schemas {
        for f in &s.fields {
            assert!(
                !matches!(
                    f.field_type,
                    FieldType::OneOf { .. } | FieldType::AnyOf { .. }
                ),
                "reserved variants must not appear from valid input: {}.{}",
                s.name,
                f.name
            );
        }
    }
    // The top-level oneOf schema is skipped, like today's unsupported kinds.
    assert!(manifest.schemas.iter().all(|s| s.name != "Pet"));
}

// ---------------------------------------------------------------------------
// Friendly Swagger 2.0 error through the full pipeline
// ---------------------------------------------------------------------------

#[test]
fn test_swagger_2_spec_fails_with_migration_guidance() {
    let yaml = r#"
swagger: "2.0"
info:
  title: Legacy
  version: "1.0.0"
paths: {}
"#;
    let doc: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
    let err = spec_to_manifest(&doc, "legacy", None)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Swagger 2.0 is not supported; convert to OpenAPI 3.x"),
        "error should name the migration path, got: {err}"
    );
}
