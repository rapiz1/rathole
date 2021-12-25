use crate::{
    config::{ClientServiceConfig, ServerServiceConfig},
    Config,
};
use anyhow::{Context, Result};
use notify::{EventKind, RecursiveMode, Watcher};
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, instrument};

#[derive(Debug)]
pub enum ConfigChangeEvent {
    General(Config), // Trigger a full restart
    ServiceChange(ServiceChangeEvent),
}

#[derive(Debug)]
pub enum ServiceChangeEvent {
    AddClientService(ClientServiceConfig),
    DeleteClientService(ClientServiceConfig),
    AddServerService(ServerServiceConfig),
    DeleteServerService(ServerServiceConfig),
}

pub struct ConfigWatcherHandle {
    pub event_rx: mpsc::Receiver<ConfigChangeEvent>,
}

impl ConfigWatcherHandle {
    pub async fn new(path: &PathBuf, shutdown_rx: broadcast::Receiver<bool>) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::channel(16);

        let origin_cfg = Config::from_file(path).await?;

        tokio::spawn(config_watcher(
            path.to_owned(),
            shutdown_rx,
            event_tx,
            origin_cfg,
        ));

        Ok(ConfigWatcherHandle { event_rx })
    }
}

#[instrument(skip(shutdown_rx, cfg_event_tx))]
async fn config_watcher(
    path: PathBuf,
    mut shutdown_rx: broadcast::Receiver<bool>,
    cfg_event_tx: mpsc::Sender<ConfigChangeEvent>,
    mut old: Config,
) -> Result<()> {
    let (fevent_tx, mut fevent_rx) = mpsc::channel(16);

    let mut watcher = notify::recommended_watcher(move |res| match res {
        Ok(event) => {
            let _ = fevent_tx.blocking_send(event);
        }
        Err(e) => error!("watch error: {:?}", e),
    })?;

    // Initial start
    cfg_event_tx
        .send(ConfigChangeEvent::General(old.clone()))
        .await
        .unwrap();

    watcher.watch(&path, RecursiveMode::NonRecursive)?;
    info!("Start watching the config");

    loop {
        tokio::select! {
          e = fevent_rx.recv() => {
            match e {
              Some(e) => {
                match e.kind {
                  EventKind::Modify(_) => {
                    info!("Configuration modify event is detected");
                    let new = match Config::from_file(&path).await.with_context(|| "The changed configuration is invalid. Ignored") {
                      Ok(v) => v,
                      Err(e) => {
                        error!("{:?}", e);
                        // If the config is invalid, just ignore it
                        continue;
                      }
                    };

                    for event in calculate_event(&old, &new) {
                      cfg_event_tx.send(event).await?;
                    }

                    old = new;
                  },
                  _ => (), // Just ignore other events
                }
              },
              None => break
            }
          },
          _ = shutdown_rx.recv() => break
        }
    }

    info!("Config watcher exiting");

    Ok(())
}

fn calculate_event(old: &Config, new: &Config) -> Vec<ConfigChangeEvent> {
    let mut ret = Vec::new();

    if old == new {
        return ret;
    }

    ret.push(ConfigChangeEvent::General(new.to_owned()));

    ret
}
