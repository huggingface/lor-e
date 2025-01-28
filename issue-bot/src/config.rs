use config::{Config, ConfigError};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct DatabaseConfig {
    pub host: String,
    pub password: String,
    pub port: u16,
    pub user: String,
}

#[derive(Deserialize)]
pub struct ServerConfig {
    pub ip: String,
    pub port: u16,
}

#[derive(Deserialize)]
pub struct IssueBotConfig {
    pub database: DatabaseConfig,
    pub server: ServerConfig,
}

pub fn load_config<'de, T: Deserialize<'de>>(prefix: &str) -> Result<T, ConfigError> {
    let base_path = std::env::current_dir().expect("Failed to determine the current directory");
    let configuration_directory = base_path.join("configuration");

    let mut config_builder = Config::builder().add_source(config::File::from(
        configuration_directory.join("base.yaml"),
    ));
    let environment = config::Environment::default()
        .separator("__")
        .prefix(prefix)
        .prefix_separator("__");
    config_builder = config_builder.add_source(environment);
    let config = config_builder.build()?.try_deserialize()?;
    Ok(config)
}
