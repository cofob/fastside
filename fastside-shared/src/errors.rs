use thiserror::Error;

#[derive(Error, Debug)]
pub enum CliError {
    #[error("no subcommand was used")]
    NoSubcommand,
}

#[derive(Error, Debug)]
pub enum UserConfigError {
    #[error("serialization error: `{0}`")]
    Serialization(#[from] serde_json::Error),
    #[error("urlencode error: `{0}`")]
    Base64Decode(#[from] base64::DecodeError),
}
