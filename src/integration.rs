use anyhow::{Context, Result, anyhow};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

trait ClientIntegration: Sync {
    fn name(&self) -> &'static str;
    fn install(&self) -> Result<()>;
}

struct PiIntegration;

struct NativeIntegration {
    name: &'static str,
    executable: &'static str,
    arguments: fn(&str) -> Vec<OsString>,
}

static PI: PiIntegration = PiIntegration;
static CODEX: NativeIntegration = NativeIntegration {
    name: "codex",
    executable: "codex",
    arguments: |mcpd| {
        ["mcp", "add", "mcpd", "--", mcpd, "serve"]
            .into_iter()
            .map(OsString::from)
            .collect()
    },
};
static CLAUDE: NativeIntegration = NativeIntegration {
    name: "claude",
    executable: "claude",
    arguments: |mcpd| {
        [
            "mcp",
            "add",
            "--transport",
            "stdio",
            "--scope",
            "user",
            "mcpd",
            "--",
            mcpd,
            "serve",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    },
};
static INTEGRATIONS: [&dyn ClientIntegration; 3] = [&PI, &CODEX, &CLAUDE];

impl ClientIntegration for PiIntegration {
    fn name(&self) -> &'static str {
        "pi"
    }

    fn install(&self) -> Result<()> {
        let agent_dir = match std::env::var_os("PI_CODING_AGENT_DIR") {
            Some(path) => PathBuf::from(path),
            None => dirs::home_dir()
                .context("Could not determine home directory")?
                .join(".pi/agent"),
        };
        let directory = agent_dir.join("extensions");
        std::fs::create_dir_all(&directory)?;
        let destination = directory.join("mcpd.ts");
        let temporary = directory.join(format!(".mcpd-{}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            let source = include_str!("../integrations/pi.ts")
                .replace("__MCPD_VERSION__", env!("CARGO_PKG_VERSION"));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(source.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.with_context(|| {
            format!(
                "Failed to install Pi extension at {}",
                destination.display()
            )
        })?;
        println!("Installed Pi extension: {}", destination.display());
        println!("The extension runs mcpd from PATH. Use /reload in Pi or start a new session.");
        Ok(())
    }
}

impl ClientIntegration for NativeIntegration {
    fn name(&self) -> &'static str {
        self.name
    }

    fn install(&self) -> Result<()> {
        let executable = which::which(self.executable)
            .with_context(|| format!("Could not find {} in PATH", self.executable))?;
        let mcpd = which::which("mcpd")
            .or_else(|_| std::env::current_exe())?
            .to_string_lossy()
            .into_owned();
        let status = Command::new(executable)
            .args((self.arguments)(&mcpd))
            .status()
            .with_context(|| format!("Failed to run {} MCP setup", self.name))?;
        if !status.success() {
            return Err(anyhow!(
                "{} MCP setup exited with status {}",
                self.name,
                status
            ));
        }
        println!("Configured mcpd for {}.", self.name);
        Ok(())
    }
}

pub fn install(name: &str) -> Result<()> {
    let integration = INTEGRATIONS
        .iter()
        .find(|integration| integration.name() == name)
        .ok_or_else(|| {
            anyhow!(
                "Unknown client '{}'. Supported clients: {}",
                name,
                names().join(", ")
            )
        })?;
    integration.install()
}

pub fn names() -> Vec<&'static str> {
    INTEGRATIONS
        .iter()
        .map(|integration| integration.name())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_integrations_are_stable() {
        assert_eq!(names(), vec!["pi", "codex", "claude"]);
    }

    #[test]
    fn native_arguments_launch_mcpd_over_stdio() {
        assert_eq!(
            (CODEX.arguments)("/bin/mcpd"),
            ["mcp", "add", "mcpd", "--", "/bin/mcpd", "serve"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            (CLAUDE.arguments)("/bin/mcpd"),
            [
                "mcp",
                "add",
                "--transport",
                "stdio",
                "--scope",
                "user",
                "mcpd",
                "--",
                "/bin/mcpd",
                "serve"
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );
    }
}
