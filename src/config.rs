use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use toml;

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub enum TransportType {
    #[serde(rename = "tcp")]
    Tcp,
    #[serde(rename = "tls")]
    Tls,
}

impl Default for TransportType {
    fn default() -> TransportType {
        TransportType::Tcp
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientServiceConfig {
    #[serde(skip)]
    pub name: String,
    pub local_addr: String,
    pub token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServerServiceConfig {
    #[serde(skip)]
    pub name: String,
    pub bind_addr: String,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TlsConfig {
    pub hostname: Option<String>,
    pub trusted_root: Option<String>,
    pub pkcs12: Option<String>,
    pub pkcs12_password: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct TransportConfig {
    #[serde(rename = "type")]
    pub transport_type: TransportType,
    pub tls: Option<TlsConfig>,
}

fn default_transport() -> TransportConfig {
    Default::default()
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ClientConfig {
    pub remote_addr: String,
    pub default_token: Option<String>,
    pub services: HashMap<String, ClientServiceConfig>,
    #[serde(default = "default_transport")]
    pub transport: TransportConfig,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub default_token: Option<String>,
    pub services: HashMap<String, ServerServiceConfig>,
    #[serde(default = "default_transport")]
    pub transport: TransportConfig,
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
            Config::validate_server_config(server)?;
        }

        if let Some(client) = config.client.as_mut() {
            Config::validate_client_config(client)?;
        }

        if config.server.is_none() && config.client.is_none() {
            Err(anyhow!("Neither of `[server]` or `[client]` is defined"))
        } else {
            Ok(config)
        }
    }

    fn validate_server_config(server: &mut ServerConfig) -> Result<()> {
        // Validate services
        for (name, s) in &mut server.services {
            s.name = name.clone();
            if s.token.is_none() {
                s.token = server.default_token.clone();
                if s.token.is_none() {
                    bail!("The token of service {} is not set", name);
                }
            }
        }

        Config::validate_transport_config(&server.transport, true)?;

        Ok(())
    }

    fn validate_client_config(client: &mut ClientConfig) -> Result<()> {
        // Validate services
        for (name, s) in &mut client.services {
            s.name = name.clone();
            if s.token.is_none() {
                s.token = client.default_token.clone();
                if s.token.is_none() {
                    bail!("The token of service {} is not set", name);
                }
            }
        }

        Config::validate_transport_config(&client.transport, false)?;

        Ok(())
    }

    fn validate_transport_config(config: &TransportConfig, is_server: bool) -> Result<()> {
        match config.transport_type {
            TransportType::Tcp => Ok(()),
            TransportType::Tls => {
                let tls_config = config
                    .tls
                    .as_ref()
                    .ok_or(anyhow!("Missing TLS configuration"))?;
                if is_server {
                    tls_config
                        .pkcs12
                        .as_ref()
                        .and(tls_config.pkcs12_password.as_ref())
                        .ok_or(anyhow!("Missing `pkcs12` or `pkcs12_password`"))?;
                } else {
                    tls_config
                        .trusted_root
                        .as_ref()
                        .ok_or(anyhow!("Missing `trusted_root`"))?;
                }
                Ok(())
            }
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
