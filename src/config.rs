use anyhow::Result;

use crate::session::SessionMode;

#[derive(Debug, Clone)]
pub struct Config {
    pub proxy_addr: String,
    pub api_addr: String,
    pub database_url: String,
    pub session_ttl_seconds: u64,
    pub default_mode: SessionMode,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let default_mode = std::env::var("ASTAPA_DEFAULT_MODE")
            .ok()
            .and_then(|s| SessionMode::from_str(&s))
            .unwrap_or_default();

        Ok(Self {
            proxy_addr: std::env::var("PROXY_ADDR").unwrap_or_else(|_| "0.0.0.0:5433".into()),
            api_addr: std::env::var("API_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgresql://postgres@localhost:5432/astapa".into()),
            session_ttl_seconds: std::env::var("SESSION_TTL_SECONDS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3600),
            default_mode,
        })
    }
}
