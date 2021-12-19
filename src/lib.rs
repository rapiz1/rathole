mod cli;
mod client;
mod config;
mod helper;
mod multi_map;
mod protocol;
mod server;
mod transport;

pub use cli::Cli;
pub use config::Config;

use anyhow::{anyhow, Result};
use tokio::sync::broadcast;
use tracing::debug;

use client::run_client;
use server::run_server;

pub async fn run(args: &Cli, shutdown_rx: broadcast::Receiver<bool>) -> Result<()> {
    let config = Config::from_file(&args.config_path).await?;

    debug!("{:?}", config);

    // Raise `nofile` limit on linux and mac
    fdlimit::raise_fd_limit();

    match determine_run_mode(&config, args) {
        RunMode::Undetermine => Err(anyhow!("Cannot determine running as a server or a client")),
        RunMode::Client => run_client(&config, shutdown_rx).await,
        RunMode::Server => run_server(&config, shutdown_rx).await,
    }
}

#[derive(PartialEq, Eq, Debug)]
enum RunMode {
    Server,
    Client,
    Undetermine,
}

fn determine_run_mode(config: &Config, args: &Cli) -> RunMode {
    use RunMode::*;
    if args.client && args.server {
        Undetermine
    } else if args.client {
        Client
    } else if args.server {
        Server
    } else if config.client.is_some() && config.server.is_none() {
        Client
    } else if config.server.is_some() && config.client.is_none() {
        Server
    } else {
        Undetermine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_run_mode() {
        use config::*;
        use RunMode::*;

        struct T {
            cfg_s: bool,
            cfg_c: bool,
            arg_s: bool,
            arg_c: bool,
            run_mode: RunMode,
        }

        let tests = [
            T {
                cfg_s: false,
                cfg_c: false,
                arg_s: false,
                arg_c: false,
                run_mode: Undetermine,
            },
            T {
                cfg_s: true,
                cfg_c: false,
                arg_s: false,
                arg_c: false,
                run_mode: Server,
            },
            T {
                cfg_s: false,
                cfg_c: true,
                arg_s: false,
                arg_c: false,
                run_mode: Client,
            },
            T {
                cfg_s: true,
                cfg_c: true,
                arg_s: false,
                arg_c: false,
                run_mode: Undetermine,
            },
            T {
                cfg_s: true,
                cfg_c: true,
                arg_s: true,
                arg_c: false,
                run_mode: Server,
            },
            T {
                cfg_s: true,
                cfg_c: true,
                arg_s: false,
                arg_c: true,
                run_mode: Client,
            },
            T {
                cfg_s: true,
                cfg_c: true,
                arg_s: true,
                arg_c: true,
                run_mode: Undetermine,
            },
        ];

        for t in tests {
            let config = Config {
                server: match t.cfg_s {
                    true => Some(ServerConfig::default()),
                    false => None,
                },
                client: match t.cfg_c {
                    true => Some(ClientConfig::default()),
                    false => None,
                },
            };

            let args = Cli {
                config_path: std::path::PathBuf::new(),
                server: t.arg_s,
                client: t.arg_c,
            };

            assert_eq!(determine_run_mode(&config, &args), t.run_mode);
        }
    }
}
