//! Tool registry - persistent storage of registered MCP tools.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A registered MCP tool server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Registry file format
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RegistryData {
    #[serde(default)]
    pub tools: HashMap<String, Tool>,
}

/// Tool registry with JSON file persistence
pub struct Registry {
    path: PathBuf,
    data: RegistryData,
}

impl Registry {
    /// Load registry from default location (~/.config/mcpd/registry.json)
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        Self::load_from(path)
    }

    /// Load registry from a specific path
    pub fn load_from(path: PathBuf) -> Result<Self> {
        let data = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read registry from {}", path.display()))?;
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse registry from {}", path.display()))?
        } else {
            RegistryData::default()
        };

        Ok(Self { path, data })
    }

    /// Get the default registry path
    pub fn default_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("mcpd");

        std::fs::create_dir_all(&config_dir).with_context(|| {
            format!(
                "Failed to create config directory: {}",
                config_dir.display()
            )
        })?;

        Ok(config_dir.join("registry.json"))
    }

    /// Save registry to disk
    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self.data)?;
        std::fs::write(&self.path, content)
            .with_context(|| format!("Failed to write registry to {}", self.path.display()))?;
        Ok(())
    }

    /// Register a new tool
    pub fn register(&mut self, tool: Tool) -> Result<()> {
        self.data.tools.insert(tool.name.clone(), tool);
        self.save()
    }

    /// Unregister a tool by name
    pub fn unregister(&mut self, name: &str) -> Result<bool> {
        let removed = self.data.tools.remove(name).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// List all registered tools
    pub fn list(&self) -> impl Iterator<Item = &Tool> {
        self.data.tools.values()
    }

    /// Number of registered tools
    pub fn len(&self) -> usize {
        self.data.tools.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.data.tools.is_empty()
    }

    /// Reload registry from disk. Returns the set of current tool names.
    pub fn reload(&mut self) -> Result<()> {
        let data = if self.path.exists() {
            let content = std::fs::read_to_string(&self.path)
                .with_context(|| format!("Failed to read registry from {}", self.path.display()))?;
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse registry from {}", self.path.display()))?
        } else {
            RegistryData::default()
        };
        self.data = data;
        Ok(())
    }

    /// Get the set of registered tool names
    pub fn names(&self) -> std::collections::HashSet<String> {
        self.data.tools.keys().cloned().collect()
    }
}
