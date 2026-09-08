//! Tool registry - persistent storage of registered MCP tools.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

/// A registered MCP tool server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TransportSpec {
    Stdio {
        command: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendSpec {
    pub name: String,
    pub transport: TransportSpec,
}

impl From<Tool> for BackendSpec {
    fn from(tool: Tool) -> Self {
        Self {
            name: tool.name,
            transport: TransportSpec::Stdio {
                command: tool.command,
                env: tool.env,
            },
        }
    }
}

/// Registry file format
#[derive(Debug, Serialize, Deserialize)]
pub struct RegistryData {
    pub version: u32,
    #[serde(default)]
    pub backends: HashMap<String, BackendSpec>,
}

impl Default for RegistryData {
    fn default() -> Self {
        Self {
            version: 1,
            backends: HashMap::new(),
        }
    }
}

#[derive(Deserialize)]
struct LegacyRegistryData {
    #[serde(default)]
    tools: HashMap<String, Tool>,
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
            Self::parse(&content)
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
        let parent = self
            .path
            .parent()
            .context("Registry path has no parent directory")?;
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(
            ".registry-{}-{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let result = (|| -> Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.with_context(|| format!("Failed to write registry to {}", self.path.display()))?;
        Ok(())
    }

    /// Register a new tool
    pub fn register(&mut self, backend: impl Into<BackendSpec>) -> Result<()> {
        let backend = backend.into();
        self.data.backends.insert(backend.name.clone(), backend);
        self.save()
    }

    /// Unregister a tool by name
    pub fn unregister(&mut self, name: &str) -> Result<bool> {
        let removed = self.data.backends.remove(name).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// List all registered tools
    pub fn list(&self) -> impl Iterator<Item = &BackendSpec> {
        self.data.backends.values()
    }

    /// Number of registered tools
    pub fn len(&self) -> usize {
        self.data.backends.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.data.backends.is_empty()
    }

    /// Reload registry from disk. Returns the set of current tool names.
    pub fn reload(&mut self) -> Result<()> {
        let data = if self.path.exists() {
            let content = std::fs::read_to_string(&self.path)
                .with_context(|| format!("Failed to read registry from {}", self.path.display()))?;
            Self::parse(&content)
                .with_context(|| format!("Failed to parse registry from {}", self.path.display()))?
        } else {
            RegistryData::default()
        };
        self.data = data;
        Ok(())
    }

    /// Get the set of registered tool names
    pub fn names(&self) -> std::collections::HashSet<String> {
        self.data.backends.keys().cloned().collect()
    }

    fn parse(content: &str) -> Result<RegistryData> {
        let value: serde_json::Value = serde_json::from_str(content)?;
        if value.get("backends").is_some() {
            let data: RegistryData = serde_json::from_value(value)?;
            if data.version != 1 {
                anyhow::bail!("Unsupported registry version: {}", data.version);
            }
            return Ok(data);
        }
        let legacy: LegacyRegistryData = serde_json::from_value(value)?;
        Ok(RegistryData {
            version: 1,
            backends: legacy
                .tools
                .into_iter()
                .map(|(name, tool)| (name, tool.into()))
                .collect(),
        })
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
            version: 1,
            backends: {
                let mut m = HashMap::new();
                m.insert("external".to_string(), sample_tool("external").into());
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
        let TransportSpec::Stdio { command, .. } = &tools[0].transport;
        assert_eq!(command, &vec!["/usr/bin/true".to_string()]);
    }

    #[test]
    fn tool_with_env_vars_persists() {
        let (mut reg, _dir) = temp_registry();
        let mut tool = sample_tool("envtest");
        tool.env.insert("API_KEY".to_string(), "secret".to_string());
        reg.register(tool).unwrap();

        reg.reload().unwrap();
        let tools: Vec<_> = reg.list().collect();
        let TransportSpec::Stdio { env, .. } = &tools[0].transport;
        assert_eq!(env.get("API_KEY").unwrap(), "secret");
    }

    #[test]
    fn legacy_registry_migrates_on_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.json");
        std::fs::write(
            &path,
            r#"{"tools":{"legacy":{"name":"legacy","command":["/bin/true"],"env":{}}}}"#,
        )
        .unwrap();
        let registry = Registry::load_from(path.clone()).unwrap();
        assert_eq!(registry.len(), 1);
        registry.save().unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["backends"]["legacy"]["transport"]["type"], "stdio");
        assert!(value.get("tools").is_none());
    }
}
