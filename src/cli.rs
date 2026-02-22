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
    },
}
