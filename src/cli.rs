use clap::{AppSettings, Parser};

#[derive(Parser, Debug)]
#[clap(about, version, setting(AppSettings::DeriveDisplayOrder))]
pub struct Cli {
    /// The path to the configuration file
    ///
    /// Running as a client or a server is automatically determined
    /// according to the configuration file.
    #[clap(parse(from_os_str), name = "config")]
    pub config_path: std::path::PathBuf,

    /// Run as a server
    #[clap(long, short)]
    pub server: bool,

    /// Run as a client
    #[clap(long, short)]
    pub client: bool,
}
