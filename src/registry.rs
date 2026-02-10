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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_registry() -> (Registry, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.json");
        let registry = Registry::load_from(path).unwrap();
        (registry, dir)
    }

    fn sample_tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            command: vec!["/usr/bin/echo".to_string(), "hello".to_string()],
            env: HashMap::new(),
        }
    }

    #[test]
    fn empty_registry() {
        let (reg, _dir) = temp_registry();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn register_and_list() {
        let (mut reg, _dir) = temp_registry();
        reg.register(sample_tool("test")).unwrap();
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
        let tools: Vec<_> = reg.list().collect();
        assert_eq!(tools[0].name, "test");
    }

    #[test]
    fn register_persists_to_disk() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.json");

        {
            let mut reg = Registry::load_from(path.clone()).unwrap();
            reg.register(sample_tool("persist")).unwrap();
        }

        let reg2 = Registry::load_from(path).unwrap();
        assert_eq!(reg2.len(), 1);
        let tools: Vec<_> = reg2.list().collect();
        assert_eq!(tools[0].name, "persist");
    }

    #[test]
    fn unregister_existing() {
        let (mut reg, _dir) = temp_registry();
        reg.register(sample_tool("test")).unwrap();
        assert!(reg.unregister("test").unwrap());
        assert!(reg.is_empty());
    }

    #[test]
    fn unregister_nonexistent() {
        let (mut reg, _dir) = temp_registry();
        assert!(!reg.unregister("nonexistent").unwrap());
    }

    #[test]
    fn reload_picks_up_external_changes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.json");

        let mut reg = Registry::load_from(path.clone()).unwrap();
        reg.register(sample_tool("original")).unwrap();

        let new_data = RegistryData {
            tools: {
                let mut m = HashMap::new();
                m.insert("external".to_string(), sample_tool("external"));
                m
            },
        };
        let content = serde_json::to_string_pretty(&new_data).unwrap();
        std::fs::write(&path, content).unwrap();

        reg.reload().unwrap();
        assert_eq!(reg.len(), 1);
        let names = reg.names();
        assert!(names.contains("external"));
        assert!(!names.contains("original"));
    }

    #[test]
    fn names_returns_correct_set() {
        let (mut reg, _dir) = temp_registry();
        reg.register(sample_tool("a")).unwrap();
        reg.register(sample_tool("b")).unwrap();
        let names = reg.names();
        assert_eq!(names.len(), 2);
        assert!(names.contains("a"));
        assert!(names.contains("b"));
    }

    #[test]
    fn register_overwrites_existing() {
        let (mut reg, _dir) = temp_registry();
        reg.register(sample_tool("test")).unwrap();
        let mut tool = sample_tool("test");
        tool.command = vec!["/usr/bin/true".to_string()];
        reg.register(tool).unwrap();
        assert_eq!(reg.len(), 1);
        let tools: Vec<_> = reg.list().collect();
        assert_eq!(tools[0].command, vec!["/usr/bin/true".to_string()]);
    }

    #[test]
    fn tool_with_env_vars_persists() {
        let (mut reg, _dir) = temp_registry();
        let mut tool = sample_tool("envtest");
        tool.env.insert("API_KEY".to_string(), "secret".to_string());
        reg.register(tool).unwrap();

        reg.reload().unwrap();
        let tools: Vec<_> = reg.list().collect();
        assert_eq!(tools[0].env.get("API_KEY").unwrap(), "secret");
    }
}
