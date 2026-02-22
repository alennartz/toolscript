use serde::{Deserialize, Serialize};

/// The top-level manifest produced by codegen. Contains API configurations,
/// function definitions, and schema definitions extracted from `OpenAPI` specs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub apis: Vec<ApiConfig>,
    pub functions: Vec<FunctionDef>,
    pub schemas: Vec<SchemaDef>,
}

/// Configuration for a single API, extracted from info + servers + security.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiConfig {
    pub name: String,
    pub base_url: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub auth: Option<AuthConfig>,
}

/// Authentication configuration for an API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    Bearer { header: String, prefix: String },
    ApiKey { header: String },
    Basic,
}

/// A single function (API operation) exposed in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionDef {
    pub name: String,
    pub api: String,
    pub tag: Option<String>,
    pub method: HttpMethod,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub parameters: Vec<ParamDef>,
    pub request_body: Option<RequestBodyDef>,
    pub response_schema: Option<String>,
}

/// HTTP method for a function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// A parameter definition for a function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParamDef {
    pub name: String,
    pub location: ParamLocation,
    pub param_type: ParamType,
    pub required: bool,
    pub description: Option<String>,
    pub default: Option<serde_json::Value>,
    pub enum_values: Option<Vec<String>>,
}

/// Where a parameter is located in the request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
}

/// The scalar type of a parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
}

/// A request body definition for a function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestBodyDef {
    pub content_type: String,
    pub schema: String,
    pub required: bool,
    pub description: Option<String>,
}

/// A schema (data type) definition extracted from components/schemas.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchemaDef {
    pub name: String,
    pub description: Option<String>,
    pub fields: Vec<FieldDef>,
}

/// A single field within a schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldDef {
    pub name: String,
    pub field_type: FieldType,
    pub required: bool,
    pub description: Option<String>,
    pub enum_values: Option<Vec<String>>,
}

/// The type of a schema field, including compound types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldType {
    String,
    Integer,
    Number,
    Boolean,
    Array { items: Box<Self> },
    Object { schema: String },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_manifest_serialization_roundtrip() {
        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "petstore".to_string(),
                base_url: "https://petstore.example.com/v1".to_string(),
                description: Some("A sample petstore API".to_string()),
                version: Some("1.0.0".to_string()),
                auth: Some(AuthConfig::Bearer {
                    header: "Authorization".to_string(),
                    prefix: "Bearer ".to_string(),
                }),
            }],
            functions: vec![FunctionDef {
                name: "list_pets".to_string(),
                api: "petstore".to_string(),
                tag: Some("pets".to_string()),
                method: HttpMethod::Get,
                path: "/pets".to_string(),
                summary: Some("List all pets".to_string()),
                description: Some("Returns a list of pets".to_string()),
                deprecated: false,
                parameters: vec![
                    ParamDef {
                        name: "status".to_string(),
                        location: ParamLocation::Query,
                        param_type: ParamType::String,
                        required: false,
                        description: Some("Filter by status".to_string()),
                        default: None,
                        enum_values: Some(vec![
                            "available".to_string(),
                            "pending".to_string(),
                            "sold".to_string(),
                        ]),
                    },
                    ParamDef {
                        name: "limit".to_string(),
                        location: ParamLocation::Query,
                        param_type: ParamType::Integer,
                        required: false,
                        description: Some("Max items to return".to_string()),
                        default: Some(serde_json::Value::Number(20.into())),
                        enum_values: None,
                    },
                ],
                request_body: None,
                response_schema: Some("Pet".to_string()),
            }],
            schemas: vec![SchemaDef {
                name: "Pet".to_string(),
                description: Some("A pet in the store".to_string()),
                fields: vec![
                    FieldDef {
                        name: "id".to_string(),
                        field_type: FieldType::String,
                        required: true,
                        description: Some("Unique identifier".to_string()),
                        enum_values: None,
                    },
                    FieldDef {
                        name: "name".to_string(),
                        field_type: FieldType::String,
                        required: true,
                        description: Some("Pet name".to_string()),
                        enum_values: None,
                    },
                    FieldDef {
                        name: "status".to_string(),
                        field_type: FieldType::String,
                        required: true,
                        description: Some("Adoption status".to_string()),
                        enum_values: Some(vec![
                            "available".to_string(),
                            "pending".to_string(),
                            "sold".to_string(),
                        ]),
                    },
                    FieldDef {
                        name: "tag".to_string(),
                        field_type: FieldType::String,
                        required: false,
                        description: Some("Classification tag".to_string()),
                        enum_values: None,
                    },
                ],
            }],
        };

        // Serialize to JSON
        let json = serde_json::to_string_pretty(&manifest).expect("Failed to serialize manifest");

        // Deserialize back
        let roundtripped: Manifest =
            serde_json::from_str(&json).expect("Failed to deserialize manifest");

        // Verify key fields survived the roundtrip
        assert_eq!(roundtripped.apis.len(), 1);
        assert_eq!(roundtripped.apis[0].name, "petstore");
        assert_eq!(
            roundtripped.apis[0].base_url,
            "https://petstore.example.com/v1"
        );
        assert_eq!(
            roundtripped.apis[0].auth,
            Some(AuthConfig::Bearer {
                header: "Authorization".to_string(),
                prefix: "Bearer ".to_string(),
            })
        );

        assert_eq!(roundtripped.functions.len(), 1);
        assert_eq!(roundtripped.functions[0].name, "list_pets");
        assert_eq!(roundtripped.functions[0].method, HttpMethod::Get);
        assert_eq!(roundtripped.functions[0].parameters.len(), 2);
        assert_eq!(
            roundtripped.functions[0].parameters[0].location,
            ParamLocation::Query
        );

        assert_eq!(roundtripped.schemas.len(), 1);
        assert_eq!(roundtripped.schemas[0].name, "Pet");
        assert_eq!(roundtripped.schemas[0].fields.len(), 4);
    }

    #[test]
    fn test_auth_config_serde_tags() {
        let bearer = AuthConfig::Bearer {
            header: "Authorization".to_string(),
            prefix: "Bearer ".to_string(),
        };
        let json = serde_json::to_string(&bearer).unwrap();
        assert!(json.contains(r#""type":"bearer"#));

        let api_key = AuthConfig::ApiKey {
            header: "X-API-Key".to_string(),
        };
        let json = serde_json::to_string(&api_key).unwrap();
        assert!(json.contains(r#""type":"api_key"#));

        let basic = AuthConfig::Basic;
        let json = serde_json::to_string(&basic).unwrap();
        assert!(json.contains(r#""type":"basic"#));
    }

    #[test]
    fn test_http_method_serde() {
        let get = HttpMethod::Get;
        let json = serde_json::to_string(&get).unwrap();
        assert_eq!(json, r#""GET""#);

        let post = HttpMethod::Post;
        let json = serde_json::to_string(&post).unwrap();
        assert_eq!(json, r#""POST""#);

        let deserialized: HttpMethod = serde_json::from_str(r#""DELETE""#).unwrap();
        assert_eq!(deserialized, HttpMethod::Delete);
    }

    #[test]
    fn test_field_type_array_serde() {
        let array_type = FieldType::Array {
            items: Box::new(FieldType::String),
        };
        let json = serde_json::to_string(&array_type).unwrap();
        let deserialized: FieldType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, array_type);
    }

    #[test]
    fn test_field_type_object_serde() {
        let obj_type = FieldType::Object {
            schema: "Pet".to_string(),
        };
        let json = serde_json::to_string(&obj_type).unwrap();
        let deserialized: FieldType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, obj_type);
    }

    #[test]
    fn test_manifest_yaml_roundtrip() {
        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "test_api".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: Some("2.0.0".to_string()),
                auth: Some(AuthConfig::ApiKey {
                    header: "X-API-Key".to_string(),
                }),
            }],
            functions: vec![],
            schemas: vec![],
        };

        let yaml = serde_yaml::to_string(&manifest).expect("Failed to serialize to YAML");
        let roundtripped: Manifest =
            serde_yaml::from_str(&yaml).expect("Failed to deserialize from YAML");
        assert_eq!(roundtripped, manifest);
    }

    #[test]
    fn test_manifest_json_structure() {
        let manifest = Manifest {
            apis: vec![ApiConfig {
                name: "myapi".to_string(),
                base_url: "https://api.example.com".to_string(),
                description: None,
                version: None,
                auth: Some(AuthConfig::Bearer {
                    header: "Authorization".to_string(),
                    prefix: "Bearer ".to_string(),
                }),
            }],
            functions: vec![FunctionDef {
                name: "get_item".to_string(),
                api: "myapi".to_string(),
                tag: None,
                method: HttpMethod::Get,
                path: "/items/{id}".to_string(),
                summary: Some("Get item".to_string()),
                description: None,
                deprecated: true,
                parameters: vec![ParamDef {
                    name: "id".to_string(),
                    location: ParamLocation::Path,
                    param_type: ParamType::String,
                    required: true,
                    description: None,
                    default: None,
                    enum_values: None,
                }],
                request_body: None,
                response_schema: None,
            }],
            schemas: vec![],
        };

        let value: serde_json::Value = serde_json::to_value(&manifest).unwrap();

        // Verify the JSON structure matches expected layout
        assert!(value["apis"].is_array());
        assert_eq!(value["apis"][0]["auth"]["type"], "bearer");
        assert_eq!(value["apis"][0]["auth"]["header"], "Authorization");
        assert_eq!(value["apis"][0]["auth"]["prefix"], "Bearer ");
        assert_eq!(value["functions"][0]["method"], "GET");
        assert_eq!(value["functions"][0]["deprecated"], true);
        assert_eq!(value["functions"][0]["parameters"][0]["location"], "path");
        assert_eq!(
            value["functions"][0]["parameters"][0]["param_type"],
            "string"
        );
    }

    #[test]
    fn test_request_body_def_roundtrip() {
        let func = FunctionDef {
            name: "create_pet".to_string(),
            api: "petstore".to_string(),
            tag: Some("pets".to_string()),
            method: HttpMethod::Post,
            path: "/pets".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![],
            request_body: Some(RequestBodyDef {
                content_type: "application/json".to_string(),
                schema: "NewPet".to_string(),
                required: true,
                description: Some("The pet to create".to_string()),
            }),
            response_schema: Some("Pet".to_string()),
        };

        let json = serde_json::to_string(&func).unwrap();
        let roundtripped: FunctionDef = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtripped, func);
    }

    #[test]
    fn test_param_location_variants() {
        for (variant, expected) in [
            (ParamLocation::Path, "\"path\""),
            (ParamLocation::Query, "\"query\""),
            (ParamLocation::Header, "\"header\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn test_param_type_variants() {
        for (variant, expected) in [
            (ParamType::String, "\"string\""),
            (ParamType::Integer, "\"integer\""),
            (ParamType::Number, "\"number\""),
            (ParamType::Boolean, "\"boolean\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
        }
    }
}
