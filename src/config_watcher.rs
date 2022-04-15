use crate::{
    config::{ClientConfig, ClientServiceConfig, ServerConfig, ServerServiceConfig},
    Config,
};
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, instrument};

#[cfg(feature = "notify")]
use notify::{EventKind, RecursiveMode, Watcher};

#[derive(Debug, PartialEq)]
pub enum ConfigChange {
    General(Box<Config>), // Trigger a full restart
    ServiceChange(ServiceChange),
}

#[derive(Debug, PartialEq)]
pub enum ServiceChange {
    ClientAdd(ClientServiceConfig),
    ClientDelete(String),
    ServerAdd(ServerServiceConfig),
    ServerDelete(String),
}

impl From<ClientServiceConfig> for ServiceChange {
    fn from(c: ClientServiceConfig) -> Self {
        ServiceChange::ClientAdd(c)
    }
}

impl From<ServerServiceConfig> for ServiceChange {
    fn from(c: ServerServiceConfig) -> Self {
        ServiceChange::ServerAdd(c)
    }
}

trait InstanceConfig: Clone {
    type ServiceConfig: Into<ServiceChange> + PartialEq + Clone;
    fn equal_without_service(&self, rhs: &Self) -> bool;
    fn to_service_change_delete(s: String) -> ServiceChange;
    fn get_services(&self) -> &HashMap<String, Self::ServiceConfig>;
}

impl InstanceConfig for ServerConfig {
    type ServiceConfig = ServerServiceConfig;
    fn equal_without_service(&self, rhs: &Self) -> bool {
        let left = ServerConfig {
            services: Default::default(),
            ..self.clone()
        };

        let right = ServerConfig {
            services: Default::default(),
            ..rhs.clone()
        };

        left == right
    }
    fn to_service_change_delete(s: String) -> ServiceChange {
        ServiceChange::ServerDelete(s)
    }
    fn get_services(&self) -> &HashMap<String, Self::ServiceConfig> {
        &self.services
    }
}

impl InstanceConfig for ClientConfig {
    type ServiceConfig = ClientServiceConfig;
    fn equal_without_service(&self, rhs: &Self) -> bool {
        let left = ClientConfig {
            services: Default::default(),
            ..self.clone()
        };

        let right = ClientConfig {
            services: Default::default(),
            ..rhs.clone()
        };

        left == right
    }
    fn to_service_change_delete(s: String) -> ServiceChange {
        ServiceChange::ClientDelete(s)
    }
    fn get_services(&self) -> &HashMap<String, Self::ServiceConfig> {
        &self.services
    }
}

pub struct ConfigWatcherHandle {
    pub event_rx: mpsc::UnboundedReceiver<ConfigChange>,
}

impl ConfigWatcherHandle {
    pub async fn new(path: &Path, shutdown_rx: broadcast::Receiver<bool>) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let origin_cfg = Config::from_file(path).await?;

        // Initial start
        event_tx
            .send(ConfigChange::General(Box::new(origin_cfg.clone())))
            .unwrap();

        tokio::spawn(config_watcher(
            path.to_owned(),
            shutdown_rx,
            event_tx,
            origin_cfg,
        ));

        Ok(ConfigWatcherHandle { event_rx })
    }
}

// Fake config watcher when compiling without `notify`
#[cfg(not(feature = "notify"))]
async fn config_watcher(
    _path: PathBuf,
    mut shutdown_rx: broadcast::Receiver<bool>,
    _event_tx: mpsc::UnboundedSender<ConfigChange>,
    _old: Config,
) -> Result<()> {
    // Do nothing except waiting for ctrl-c
    let _ = shutdown_rx.recv().await;
    Ok(())
}

#[cfg(feature = "notify")]
#[instrument(skip(shutdown_rx, event_tx, old))]
async fn config_watcher(
    path: PathBuf,
    mut shutdown_rx: broadcast::Receiver<bool>,
    event_tx: mpsc::UnboundedSender<ConfigChange>,
    mut old: Config,
) -> Result<()> {
    let (fevent_tx, mut fevent_rx) = mpsc::unbounded_channel();
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    };
    let parent_path = path.parent().expect("config file should have a parent dir");
    let path_clone = path.clone();
    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, _>| match res {
            Ok(e) => {
                if matches!(e.kind, EventKind::Modify(_))
                    && e.paths
                        .iter()
                        .map(|x| x.file_name())
                        .any(|x| x == path_clone.file_name())
                {
                    let _ = fevent_tx.send(true);
                }
            }
            Err(e) => error!("watch error: {:#}", e),
        })?;

    watcher.watch(parent_path, RecursiveMode::NonRecursive)?;
    info!("Start watching the config");

    loop {
        tokio::select! {
          e = fevent_rx.recv() => {
            match e {
              Some(_) => {
                    info!("Rescan the configuration");
                    let new = match Config::from_file(&path).await.with_context(|| "The changed configuration is invalid. Ignored") {
                      Ok(v) => v,
                      Err(e) => {
                        error!("{:#}", e);
                        // If the config is invalid, just ignore it
                        continue;
                      }
                    };

                    for event in calculate_events(&old, &new) {
                      event_tx.send(event)?;
                    }

                    old = new;
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

fn calculate_events(old: &Config, new: &Config) -> Vec<ConfigChange> {
    if old == new {
        return vec![];
    }

    let mut ret = vec![];

    if old.server != new.server {
        if old.server.is_some() != new.server.is_some() {
            return vec![ConfigChange::General(Box::new(new.clone()))];
        } else {
            match calculate_instance_config_events(
                old.server.as_ref().unwrap(),
                new.server.as_ref().unwrap(),
            ) {
                Some(mut v) => ret.append(&mut v),
                None => return vec![ConfigChange::General(Box::new(new.clone()))],
            }
        }
    }

    if old.client != new.client {
        if old.client.is_some() != new.client.is_some() {
            return vec![ConfigChange::General(Box::new(new.clone()))];
        } else {
            match calculate_instance_config_events(
                old.client.as_ref().unwrap(),
                new.client.as_ref().unwrap(),
            ) {
                Some(mut v) => ret.append(&mut v),
                None => return vec![ConfigChange::General(Box::new(new.clone()))],
            }
        }
    }

    ret
}

// None indicates a General change needed
fn calculate_instance_config_events<T: InstanceConfig>(
    old: &T,
    new: &T,
) -> Option<Vec<ConfigChange>> {
    if !old.equal_without_service(new) {
        return None;
    }

    let old = old.get_services();
    let new = new.get_services();

    let mut v = vec![];
    v.append(&mut calculate_service_delete_events::<T>(old, new));
    v.append(&mut calculate_service_add_events(old, new));

    Some(v.into_iter().map(ConfigChange::ServiceChange).collect())
}

fn calculate_service_delete_events<T: InstanceConfig>(
    old: &HashMap<String, T::ServiceConfig>,
    new: &HashMap<String, T::ServiceConfig>,
) -> Vec<ServiceChange> {
    old.keys()
        .filter(|&name| new.get(name).is_none())
        .map(|x| T::to_service_change_delete(x.to_owned()))
        .collect()
}

fn calculate_service_add_events<T: PartialEq + Clone + Into<ServiceChange>>(
    old: &HashMap<String, T>,
    new: &HashMap<String, T>,
) -> Vec<ServiceChange> {
    new.iter()
        .filter(|(name, c)| old.get(*name) != Some(*c))
        .map(|(_, c)| c.clone().into())
        .collect()
}

#[cfg(test)]
mod test {
    use crate::config::ServerConfig;

    use super::*;

    // macro to create map or set literal
    macro_rules! collection {
        // map-like
        ($($k:expr => $v:expr),* $(,)?) => {{
            use std::iter::{Iterator, IntoIterator};
            Iterator::collect(IntoIterator::into_iter([$(($k, $v),)*]))
        }};
    }

    #[test]
    fn test_calculate_events() {
        struct Test {
            old: Config,
            new: Config,
        }

        let tests = [
            Test {
                old: Config {
                    server: Some(Default::default()),
                    client: None,
                },
                new: Config {
                    server: Some(Default::default()),
                    client: Some(Default::default()),
                },
            },
            Test {
                old: Config {
                    server: Some(ServerConfig {
                        bind_addr: String::from("127.0.0.1:2334"),
                        ..Default::default()
                    }),
                    client: None,
                },
                new: Config {
                    server: Some(ServerConfig {
                        bind_addr: String::from("127.0.0.1:2333"),
                        services: collection!(String::from("foo") => Default::default()),
                        ..Default::default()
                    }),
                    client: None,
                },
            },
            Test {
                old: Config {
                    server: Some(Default::default()),
                    client: None,
                },
                new: Config {
                    server: Some(ServerConfig {
                        services: collection!(String::from("foo") => Default::default()),
                        ..Default::default()
                    }),
                    client: None,
                },
            },
            Test {
                old: Config {
                    server: Some(ServerConfig {
                        services: collection!(String::from("foo") => Default::default()),
                        ..Default::default()
                    }),
                    client: None,
                },
                new: Config {
                    server: Some(Default::default()),
                    client: None,
                },
            },
            Test {
                old: Config {
                    server: Some(ServerConfig {
                        services: collection!(String::from("foo1") => ServerServiceConfig::with_name("foo1"), String::from("foo2") => ServerServiceConfig::with_name("foo2")),
                        ..Default::default()
                    }),
                    client: Some(ClientConfig {
                        services: collection!(String::from("foo1") => ClientServiceConfig::with_name("foo1"), String::from("foo2") => ClientServiceConfig::with_name("foo2")),
                        ..Default::default()
                    }),
                },
                new: Config {
                    server: Some(ServerConfig {
                        services: collection!(String::from("bar1") => ServerServiceConfig::with_name("bar1"), String::from("foo2") => ServerServiceConfig::with_name("foo2")),
                        ..Default::default()
                    }),
                    client: Some(ClientConfig {
                        services: collection!(String::from("bar1") => ClientServiceConfig::with_name("bar1"), String::from("bar2") => ClientServiceConfig::with_name("bar2")),
                        ..Default::default()
                    }),
                },
            },
        ];

        let mut expected = [
            vec![ConfigChange::General(Box::new(tests[0].new.clone()))],
            vec![ConfigChange::General(Box::new(tests[1].new.clone()))],
            vec![ConfigChange::ServiceChange(ServiceChange::ServerAdd(
                Default::default(),
            ))],
            vec![ConfigChange::ServiceChange(ServiceChange::ServerDelete(
                String::from("foo"),
            ))],
            vec![
                ConfigChange::ServiceChange(ServiceChange::ServerDelete(String::from("foo1"))),
                ConfigChange::ServiceChange(ServiceChange::ServerAdd(
                    tests[4].new.server.as_ref().unwrap().services["bar1"].clone(),
                )),
                ConfigChange::ServiceChange(ServiceChange::ClientDelete(String::from("foo1"))),
                ConfigChange::ServiceChange(ServiceChange::ClientDelete(String::from("foo2"))),
                ConfigChange::ServiceChange(ServiceChange::ClientAdd(
                    tests[4].new.client.as_ref().unwrap().services["bar1"].clone(),
                )),
                ConfigChange::ServiceChange(ServiceChange::ClientAdd(
                    tests[4].new.client.as_ref().unwrap().services["bar2"].clone(),
                )),
            ],
        ];

        assert_eq!(tests.len(), expected.len());

        for i in 0..tests.len() {
            let mut actual = calculate_events(&tests[i].old, &tests[i].new);

            let get_key = |x: &ConfigChange| -> String {
                match x {
                    ConfigChange::General(_) => String::from("g"),
                    ConfigChange::ServiceChange(sc) => match sc {
                        ServiceChange::ClientAdd(c) => "c_add_".to_owned() + &c.name,
                        ServiceChange::ClientDelete(s) => "c_del_".to_owned() + s,
                        ServiceChange::ServerAdd(c) => "s_add_".to_owned() + &c.name,
                        ServiceChange::ServerDelete(s) => "s_del_".to_owned() + s,
                    },
                }
            };

            actual.sort_by_cached_key(get_key);
            expected[i].sort_by_cached_key(get_key);

            assert_eq!(actual, expected[i]);
        }
    }
}
