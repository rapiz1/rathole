use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use toml;

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub enum Encryption {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "aes")]
    Aes,
}

fn default_encryption() -> Encryption {
    Encryption::None
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientServiceConfig {
    #[serde(skip)]
    pub name: String,
    pub local_addr: String,
    pub token: Option<String>,
    #[serde(default = "default_encryption")]
    pub encryption: Encryption,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServerServiceConfig {
    #[serde(skip)]
    pub name: String,
    pub bind_addr: String,
    pub token: Option<String>,
    #[serde(default = "default_encryption")]
    pub encryption: Encryption,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ClientConfig {
    pub remote_addr: String,
    pub default_token: Option<String>,
    pub services: HashMap<String, ClientServiceConfig>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub default_token: Option<String>,
    pub services: HashMap<String, ServerServiceConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: Option<ServerConfig>,
    pub client: Option<ClientConfig>,
}

impl Config {
    fn from_str(s: &str) -> Result<Config> {
        let mut config: Config =
            toml::from_str(&s).with_context(|| "Failed to parse the config")?;
        if let Some(server) = config.server.as_mut() {
            for (name, s) in &mut server.services {
                s.name = name.clone();
                if s.token.is_none() {
                    s.token = server.default_token.clone();
                    if s.token.is_none() {
                        bail!("The token of service {} is not set", name);
                    }
                }
            }
        }
        if let Some(client) = config.client.as_mut() {
            for (name, s) in &mut client.services {
                s.name = name.clone();
                if s.token.is_none() {
                    s.token = client.default_token.clone();
                    if s.token.is_none() {
                        bail!("The token of service {} is not set", name);
                    }
                }
            }
        }
        if config.server.is_none() && config.client.is_none() {
            Err(anyhow!("Neither of `[server]` or `[client]` is defined"))
        } else {
            Ok(config)
        }
    }

    pub async fn from_file(path: &PathBuf) -> Result<Config> {
        let s: String = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read the config {:?}", path))?;
        Config::from_str(&s).with_context(|| {
            "Configuration is invalid. Please refer to the configuration specification."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use anyhow::Result;

    #[test]
    fn test_mimic_client_config() -> Result<()> {
        let s = fs::read_to_string("./example/mimic/client.toml").unwrap();
        Config::from_str(&s)?;
        Ok(())
    }

    #[test]
    fn test_mimic_server_config() -> Result<()> {
        let s = fs::read_to_string("./example/mimic/server.toml").unwrap();
        Config::from_str(&s)?;
        Ok(())
    }
}
