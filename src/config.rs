use std::{env};

/// Runtime configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub github_token: Option<String>,
    pub github_api_url: String,
    pub frontend_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        let port = env::var("PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3000);
        let github_token = env::var("GITHUB_TOKEN")
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        let github_api_url = env::var("GITHUB_API_URL")
            .unwrap_or_else(|_| "https://api.github.com".to_string())
            .trim_end_matches('/')
            .to_string();
        let frontend_url = env::var("FRONTEND_URL")
            .unwrap_or_else(|_| "http://localhost:4200".to_string())
            .trim_end_matches('/')
            .to_string();

        Self {
            port,
            github_token,
            github_api_url,
            frontend_url
        }
    }
}
 