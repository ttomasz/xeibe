use std::time::Duration;

#[derive(Debug, Clone)]
pub struct IoOptions {
    pub http: HttpOptions,
    /// Size of the blocks handed from async object-store bodies to the splitter (default 8 MiB).
    pub block_size: usize,
    /// Blocks buffered ahead of the splitter (default 4).
    pub read_ahead: usize,
}

#[derive(Debug, Clone)]
pub struct HttpOptions {
    pub timeout: Duration,
    /// Retries before the first byte of a body is handed on (5xx, 429, network errors).
    pub max_retries: u32,
    pub auth: Auth,
    pub headers: Vec<(String, String)>,
    pub user_agent: String,
}

#[derive(Debug, Clone, Default)]
pub enum Auth {
    #[default]
    None,
    Basic {
        user: String,
        password: String,
    },
    Bearer(String),
}
