use config::{Config, ConfigError};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub revision: String,
    pub embeddings_size: usize,
    pub max_input_size: usize,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            id: "NovaSearch/stella_en_1.5B_v5".to_string(),
            revision: "main".to_string(),
            embeddings_size: 384,
            max_input_size: 512,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelApiConfig {
    pub auth_token: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub connection_string: String,
    pub max_connections: u32,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub ip: String,
    pub metrics_port: u16,
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct IssueBotConfig {
    pub auth_token: String,
    pub database: DatabaseConfig,
    pub model_api: ModelApiConfig,
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
