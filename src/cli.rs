//! Command-line interface for mcpd.

use crate::integration;
use crate::registry::{BackendSpec, Registry, TransportSpec};
#[cfg(not(unix))]
use crate::server::Server;
use anyhow::Result;
use clap::{Parser, Subcommand};
#[cfg(not(unix))]
use tracing::info;

#[derive(Parser)]
#[command(name = "mcpd")]
#[command(about = "MCP daemon - aggregate multiple MCP tool servers into one")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Register a new MCP tool server
    Register {
        /// Name for this tool (used as prefix)
        name: String,
        /// Command to run the MCP server
        #[arg(required = true, num_args = 1..)]
        command: Vec<String>,
        /// Environment variables (KEY=VALUE)
        #[arg(short, long, value_parser = parse_env_var)]
        env: Vec<(String, String)>,
    },

    /// Unregister a tool server
    Unregister {
        /// Name of the tool to remove
        name: String,
    },

    /// List registered tool servers
    List,

    /// Run the aggregating MCP server (stdio mode)
    Serve,

    #[cfg(unix)]
    #[command(about = "Run the shared MCP daemon over a Unix socket")]
    Daemon,

    #[command(about = "Install or update a bundled agent integration")]
    Setup { client: String },
}

fn parse_env_var(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("Invalid KEY=VALUE format: {}", s))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        match self.command {
            Commands::Setup { client } => integration::install(&client),
            Commands::Register { name, command, env } => {
                let mut registry = Registry::load()?;

                // Resolve the command path
                let resolved_command = if command[0].contains('/') {
                    command
                } else {
                    let mut resolved = command.clone();
                    if let Ok(path) = which::which(&command[0]) {
                        resolved[0] = path.to_string_lossy().to_string();
                    }
                    resolved
                };

                let backend = BackendSpec {
                    name: name.clone(),
                    transport: TransportSpec::Stdio {
                        command: resolved_command.clone(),
                        env: env.into_iter().collect(),
                    },
                };

                registry.register(backend)?;
                println!("Registered tool '{}': {:?}", name, resolved_command);
                Ok(())
            }

            Commands::Unregister { name } => {
                let mut registry = Registry::load()?;
                if registry.unregister(&name)? {
                    println!("Unregistered tool '{}'", name);
                } else {
                    println!("Tool '{}' not found", name);
                }
                Ok(())
            }

            Commands::List => {
                let registry = Registry::load()?;

                if registry.is_empty() {
                    println!("No tools registered");
                    return Ok(());
                }

                println!("Registered tools ({}):", registry.len());
                for backend in registry.list() {
                    match &backend.transport {
                        TransportSpec::Stdio { command, env } => {
                            println!("  {} -> {:?}", backend.name, command);
                            for key in env.keys() {
                                println!("    {}=<set>", key);
                            }
                        }
                    }
                }
                Ok(())
            }

            Commands::Serve => {
                #[cfg(unix)]
                return crate::bridge::run().await;
                #[cfg(not(unix))]
                {
                    let registry = Registry::load()?;
                    info!(
                        backends = registry.len(),
                        "Starting MCP server (find_tools, list_tools, use_tool)"
                    );
                    Server::new(registry).run().await
                }
            }
            #[cfg(unix)]
            Commands::Daemon => {
                let registry = Registry::load()?;
                crate::daemon::run(registry).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_var_valid() {
        let result = parse_env_var("KEY=VALUE").unwrap();
        assert_eq!(result, ("KEY".to_string(), "VALUE".to_string()));
    }

    #[test]
    fn parse_env_var_with_equals_in_value() {
        let result = parse_env_var("KEY=VAL=UE").unwrap();
        assert_eq!(result, ("KEY".to_string(), "VAL=UE".to_string()));
    }

    #[test]
    fn parse_env_var_empty_value() {
        let result = parse_env_var("KEY=").unwrap();
        assert_eq!(result, ("KEY".to_string(), "".to_string()));
    }

    #[test]
    fn parse_env_var_missing_equals() {
        let result = parse_env_var("KEYVALUE");
        assert!(result.is_err());
    }
}
