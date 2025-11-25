use alloy::transports::http::reqwest::Url;
use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub rpc_url: Url,
    pub tycho_api_key: String,
    pub private_key: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        let rpc_url = std::env::var("RPC_URL")
            .context("RPC_URL not found in environment. Please add it to .env")?;
        let rpc_url = Url::parse(&rpc_url).context("Can't parse RPC_URL")?;

        let tycho_api_key = std::env::var("TYCHO_API_KEY")
            .context("TYCHO_API_KEY not found in environment. Please add it to .env")?;

        let private_key = std::env::var("PRIVATE_KEY")
            .context("PRIVATE_KEY not found in environment. Please add it to .env")?;

        Ok(Self {
            rpc_url,
            tycho_api_key,
            private_key,
        })
    }
}
