use crate::{
    config::{ClientServiceConfig, ServerServiceConfig},
    Config,
};
use anyhow::{Context, Result};
use notify::{event::ModifyKind, EventKind, RecursiveMode, Watcher};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, instrument};

#[derive(Debug)]
pub enum ConfigChangeEvent {
    General(Box<Config>), // Trigger a full restart
    ServiceChange(ServiceChangeEvent),
}

#[derive(Debug)]
pub enum ServiceChangeEvent {
    ClientAdd(ClientServiceConfig),
    ClientDelete(String),
    ServerAdd(ServerServiceConfig),
    ServerDelete(String),
}

pub struct ConfigWatcherHandle {
    pub event_rx: mpsc::Receiver<ConfigChangeEvent>,
}

impl ConfigWatcherHandle {
    pub async fn new(path: &Path, shutdown_rx: broadcast::Receiver<bool>) -> Result<Self> {
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

#[instrument(skip(shutdown_rx, cfg_event_tx, old))]
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
        .send(ConfigChangeEvent::General(Box::new(old.clone())))
        .await
        .unwrap();

    watcher.watch(&path, RecursiveMode::NonRecursive)?;
    info!("Start watching the config");

    loop {
        tokio::select! {
          e = fevent_rx.recv() => {
            match e {
              Some(e) => {
                if let EventKind::Modify(kind) = e.kind {
                    match kind {
                        ModifyKind::Data(_) => (),
                        _ => continue
                    }
                    info!("Rescan the configuration");
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

    if old != new {
        if old.server.is_some() && new.server.is_some() {
            let mut e: Vec<ConfigChangeEvent> = calculate_service_delete_event(
                &old.server.as_ref().unwrap().services,
                &new.server.as_ref().unwrap().services,
            )
            .into_iter()
            .map(|x| ConfigChangeEvent::ServiceChange(ServiceChangeEvent::ServerDelete(x)))
            .collect();
            ret.append(&mut e);

            let mut e: Vec<ConfigChangeEvent> = calculate_service_add_event(
                &old.server.as_ref().unwrap().services,
                &new.server.as_ref().unwrap().services,
            )
            .into_iter()
            .map(|x| ConfigChangeEvent::ServiceChange(ServiceChangeEvent::ServerAdd(x)))
            .collect();

            ret.append(&mut e);
        } else if old.client.is_some() && new.client.is_some() {
            let mut e: Vec<ConfigChangeEvent> = calculate_service_delete_event(
                &old.client.as_ref().unwrap().services,
                &new.client.as_ref().unwrap().services,
            )
            .into_iter()
            .map(|x| ConfigChangeEvent::ServiceChange(ServiceChangeEvent::ClientDelete(x)))
            .collect();
            ret.append(&mut e);

            let mut e: Vec<ConfigChangeEvent> = calculate_service_add_event(
                &old.client.as_ref().unwrap().services,
                &new.client.as_ref().unwrap().services,
            )
            .into_iter()
            .map(|x| ConfigChangeEvent::ServiceChange(ServiceChangeEvent::ClientAdd(x)))
            .collect();

            ret.append(&mut e);
        } else {
            ret.push(ConfigChangeEvent::General(Box::new(new.clone())));
        }
    }

    ret
}

fn calculate_service_delete_event<T: PartialEq>(
    old_services: &HashMap<String, T>,
    new_services: &HashMap<String, T>,
) -> Vec<String> {
    old_services
        .keys()
        .filter(|&name| old_services.get(name) != new_services.get(name))
        .map(|x| x.to_owned())
        .collect()
}

fn calculate_service_add_event<T: PartialEq + Clone>(
    old_services: &HashMap<String, T>,
    new_services: &HashMap<String, T>,
) -> Vec<T> {
    new_services
        .iter()
        .filter(|(name, _)| old_services.get(*name) != new_services.get(*name))
        .map(|(_, c)| c.clone())
        .collect()
}
