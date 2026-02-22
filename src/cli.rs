use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "code-mcp", about = "Generate MCP servers from OpenAPI specs")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Generate manifest and SDK annotations from `OpenAPI` specs
    Generate {
        /// `OpenAPI` spec sources (file paths or URLs)
        #[arg(required = true)]
        specs: Vec<String>,
        /// Output directory
        #[arg(short, long, default_value = "./output")]
        output: PathBuf,
    },
    /// Start MCP server from a generated directory
    Serve {
        /// Path to generated output directory
        #[arg(required = true)]
        dir: PathBuf,
        /// Transport type
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Port for SSE transport
        #[arg(long, default_value = "8080")]
        port: u16,
        /// OAuth authority URL for JWT validation (enables auth)
        #[arg(long, env = "MCP_AUTH_AUTHORITY")]
        auth_authority: Option<String>,
        /// Expected JWT audience (required if auth-authority is set)
        #[arg(long, env = "MCP_AUTH_AUDIENCE")]
        auth_audience: Option<String>,
        /// Explicit JWKS URI (optional, derived from authority via OIDC discovery if not set)
        #[arg(long, env = "MCP_AUTH_JWKS_URI")]
        auth_jwks_uri: Option<String>,
        /// Script execution timeout in seconds (default: 30)
        #[arg(long, default_value = "30")]
        timeout: u64,
        /// Luau VM memory limit in megabytes (default: 64)
        #[arg(long, default_value = "64")]
        memory_limit: usize,
        /// Maximum API calls per script execution (default: 100)
        #[arg(long, default_value = "100")]
        max_api_calls: usize,
    },
    /// Generate and serve in one step
    Run {
        /// `OpenAPI` spec sources (file paths or URLs)
        #[arg(required = true)]
        specs: Vec<String>,
        /// Transport type
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Port for SSE transport
        #[arg(long, default_value = "8080")]
        port: u16,
        /// OAuth authority URL for JWT validation (enables auth)
        #[arg(long, env = "MCP_AUTH_AUTHORITY")]
        auth_authority: Option<String>,
        /// Expected JWT audience (required if auth-authority is set)
        #[arg(long, env = "MCP_AUTH_AUDIENCE")]
        auth_audience: Option<String>,
        /// Explicit JWKS URI (optional, derived from authority via OIDC discovery if not set)
        #[arg(long, env = "MCP_AUTH_JWKS_URI")]
        auth_jwks_uri: Option<String>,
        /// Script execution timeout in seconds (default: 30)
        #[arg(long, default_value = "30")]
        timeout: u64,
        /// Luau VM memory limit in megabytes (default: 64)
        #[arg(long, default_value = "64")]
        memory_limit: usize,
        /// Maximum API calls per script execution (default: 100)
        #[arg(long, default_value = "100")]
        max_api_calls: usize,
    },
}
