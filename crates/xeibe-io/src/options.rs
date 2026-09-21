use std::time::Duration;

#[derive(Debug, Clone)]
pub struct IoOptions {
    pub http: HttpOptions,
    /// Size of the blocks handed from async object-store bodies to the splitter (default 8 MiB).
    pub block_size: usize,
    /// Blocks buffered ahead of the splitter (default 4).
    pub read_ahead: usize,
}

impl Default for IoOptions {
    fn default() -> Self {
        Self {
            http: HttpOptions::default(),
            block_size: 8 << 20,
            read_ahead: 4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpOptions {
    /// Limit for connecting and for each read or write, not for the whole body
    /// (default 30 s): a large file takes as long as it takes.
    pub timeout: Duration,
    /// Retries before the first byte of a body is handed on (5xx, 429, network errors).
    pub max_retries: u32,
    pub auth: Auth,
    pub headers: Vec<(String, String)>,
    pub user_agent: String,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_retries: 3,
            auth: Auth::None,
            headers: Vec::new(),
            user_agent: concat!("xeibe/", env!("CARGO_PKG_VERSION")).to_string(),
        }
    }
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
