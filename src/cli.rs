//! Command-line interface for mcpd.

use crate::registry::{Registry, Tool};
use crate::server::Server;
use anyhow::Result;
use clap::{Parser, Subcommand};
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

                let tool = Tool {
                    name: name.clone(),
                    command: resolved_command.clone(),
                    env: env.into_iter().collect(),
                };

                registry.register(tool)?;
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
                for tool in registry.list() {
                    println!("  {} -> {:?}", tool.name, tool.command);
                    if !tool.env.is_empty() {
                        for (k, v) in &tool.env {
                            println!("    {}={}", k, v);
                        }
                    }
                }
                Ok(())
            }

            Commands::Serve => {
                let registry = Registry::load()?;
                info!(backends = registry.len(), "Starting MCP server (2 meta-tools: list_tools, use_tool)");

                let server = Server::new(registry);
                server.run().await
            }
        }
    }
}
