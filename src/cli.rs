use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "toolscript", about = "Generate MCP servers from OpenAPI specs")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Generate manifest and SDK annotations from `OpenAPI` specs
    Generate {
        /// Spec sources: `path`, `url`, or `name=path`/`name=url`
        specs: Vec<String>,
        #[arg(short, long, default_value = "./output")]
        output: PathBuf,
        /// Path to TOML config file
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Start MCP server from a generated directory
    Serve {
        #[arg(required = true)]
        dir: PathBuf,
        #[arg(long, default_value = "stdio")]
        transport: String,
        #[arg(long, default_value = "8080")]
        port: u16,
        #[arg(long, env = "MCP_AUTH_AUTHORITY")]
        auth_authority: Option<String>,
        #[arg(long, env = "MCP_AUTH_AUDIENCE")]
        auth_audience: Option<String>,
        #[arg(long, env = "MCP_AUTH_JWKS_URI")]
        auth_jwks_uri: Option<String>,
        /// Upstream API auth: `name:ENV_VAR` or `ENV_VAR` (for single-spec)
        #[arg(long = "auth")]
        api_auth: Vec<String>,
        #[arg(long, default_value = "30")]
        timeout: u64,
        #[arg(long, default_value = "64")]
        memory_limit: usize,
        #[arg(long, default_value = "100")]
        max_api_calls: usize,
        /// I/O directory for sandboxed file access in scripts
        #[arg(long)]
        io_dir: Option<String>,
        /// Upstream MCP servers (`name=command_or_url`)
        #[arg(long = "mcp", num_args = 1)]
        mcp_servers: Vec<String>,
    },
    /// Generate and serve in one step
    Run {
        /// Spec sources: `path`, `url`, or `name=path`/`name=url`
        specs: Vec<String>,
        /// Path to TOML config file
        #[arg(long)]
        config: Option<PathBuf>,
        /// Upstream API auth: `name:ENV_VAR` or `ENV_VAR` (for single-spec)
        #[arg(long = "auth")]
        api_auth: Vec<String>,
        #[arg(long, default_value = "stdio")]
        transport: String,
        #[arg(long, default_value = "8080")]
        port: u16,
        #[arg(long, env = "MCP_AUTH_AUTHORITY")]
        auth_authority: Option<String>,
        #[arg(long, env = "MCP_AUTH_AUDIENCE")]
        auth_audience: Option<String>,
        #[arg(long, env = "MCP_AUTH_JWKS_URI")]
        auth_jwks_uri: Option<String>,
        #[arg(long, default_value = "30")]
        timeout: u64,
        #[arg(long, default_value = "64")]
        memory_limit: usize,
        #[arg(long, default_value = "100")]
        max_api_calls: usize,
        /// I/O directory for sandboxed file access in scripts
        #[arg(long)]
        io_dir: Option<String>,
        /// Upstream MCP servers (`name=command_or_url`)
        #[arg(long = "mcp", num_args = 1)]
        mcp_servers: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use clap::Parser;

    #[test]
    fn test_run_with_spec() {
        let cli = Cli::parse_from(["toolscript", "run", "spec.yaml"]);
        match cli.command {
            Command::Run {
                specs,
                config,
                api_auth,
                ..
            } => {
                assert_eq!(specs, vec!["spec.yaml"]);
                assert!(config.is_none());
                assert!(api_auth.is_empty());
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_run_with_config() {
        let cli = Cli::parse_from(["toolscript", "run", "--config", "toolscript.toml"]);
        match cli.command {
            Command::Run { specs, config, .. } => {
                assert!(specs.is_empty());
                assert_eq!(config.unwrap().to_str().unwrap(), "toolscript.toml");
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_run_with_auth_flag() {
        let cli = Cli::parse_from([
            "toolscript",
            "run",
            "petstore=spec.yaml",
            "--auth",
            "petstore:MY_TOKEN",
        ]);
        match cli.command {
            Command::Run {
                specs, api_auth, ..
            } => {
                assert_eq!(specs, vec!["petstore=spec.yaml"]);
                assert_eq!(api_auth, vec!["petstore:MY_TOKEN"]);
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_run_with_multiple_auth() {
        let cli = Cli::parse_from([
            "toolscript",
            "run",
            "a=a.yaml",
            "b=b.yaml",
            "--auth",
            "a:TOKEN_A",
            "--auth",
            "b:TOKEN_B",
        ]);
        match cli.command {
            Command::Run { api_auth, .. } => {
                assert_eq!(api_auth.len(), 2);
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_run_defaults() {
        let cli = Cli::parse_from(["toolscript", "run", "spec.yaml"]);
        match cli.command {
            Command::Run {
                timeout,
                memory_limit,
                max_api_calls,
                ..
            } => {
                assert_eq!(timeout, 30);
                assert_eq!(memory_limit, 64);
                assert_eq!(max_api_calls, 100);
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_serve_defaults() {
        let cli = Cli::parse_from(["toolscript", "serve", "./output"]);
        match cli.command {
            Command::Serve {
                timeout,
                memory_limit,
                max_api_calls,
                ..
            } => {
                assert_eq!(timeout, 30);
                assert_eq!(memory_limit, 64);
                assert_eq!(max_api_calls, 100);
            }
            _ => panic!("expected Serve"),
        }
    }

    #[test]
    fn test_generate_with_config() {
        let cli = Cli::parse_from(["toolscript", "generate", "--config", "my.toml", "-o", "out"]);
        match cli.command {
            Command::Generate {
                specs,
                config,
                output,
            } => {
                assert!(specs.is_empty());
                assert_eq!(config.unwrap().to_str().unwrap(), "my.toml");
                assert_eq!(output.to_str().unwrap(), "out");
            }
            _ => panic!("expected Generate"),
        }
    }

    #[test]
    fn test_run_with_io_dir() {
        let cli = Cli::parse_from(["toolscript", "run", "spec.yaml", "--io-dir", "/tmp/out"]);
        match cli.command {
            Command::Run { io_dir, .. } => {
                assert_eq!(io_dir.as_deref(), Some("/tmp/out"));
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn test_serve_with_io_dir() {
        let cli = Cli::parse_from(["toolscript", "serve", "./output", "--io-dir", "/tmp/out"]);
        match cli.command {
            Command::Serve { io_dir, .. } => {
                assert_eq!(io_dir.as_deref(), Some("/tmp/out"));
            }
            _ => panic!("expected Serve"),
        }
    }

    #[test]
    fn test_serve_with_auth() {
        let cli = Cli::parse_from([
            "toolscript",
            "serve",
            "./output",
            "--auth",
            "petstore:MY_TOKEN",
        ]);
        match cli.command {
            Command::Serve { api_auth, .. } => {
                assert_eq!(api_auth, vec!["petstore:MY_TOKEN"]);
            }
            _ => panic!("expected Serve"),
        }
    }
}
