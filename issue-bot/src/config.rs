use config::{Config, ConfigError};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct EmbeddingApiConfig {
    pub auth_token: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SummarizationApiConfig {
    pub auth_token: String,
    pub model: String,
    pub special_tokens_used: Vec<String>,
    pub system_prompt: String,
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
pub struct GithubApiConfig {
    pub auth_token: String,
    pub comments_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct HuggingfaceApiConfig {
    pub auth_token: String,
    pub comments_enabled: bool,
}

/// bot's comment message
/// will be of the form:
/// ```
/// format!("{}{}{}", message_config.pre, closest_issues, message_config.post);
/// ```
/// Which gives something like this:
/// ```txt
/// Hello!
///
/// A maintainer will soon take a look, in the meantime you might find these related issues interesting:
/// - Test issue (#29)
/// - Another issue (#30)
///
/// Thank you for opening this issue!
/// ```
#[derive(Clone, Debug, Deserialize)]
pub struct MessageConfig {
    pub pre: String,
    pub post: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SlackConfig {
    pub auth_token: String,
    pub channel: String,
    pub chat_write_url: String,
}

#[derive(Debug, Deserialize)]
pub struct IssueBotConfig {
    pub auth_token: String,
    pub database: DatabaseConfig,
    pub embedding_api: EmbeddingApiConfig,
    pub github_api: GithubApiConfig,
    pub huggingface_api: HuggingfaceApiConfig,
    pub message_config: MessageConfig,
    pub server: ServerConfig,
    pub slack: SlackConfig,
    pub summarization_api: SummarizationApiConfig,
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
