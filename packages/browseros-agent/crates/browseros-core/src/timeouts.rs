use std::time::Duration;

pub const CDP_CONNECT: Duration = Duration::from_secs(10);
pub const CDP_CONNECT_RETRY_DELAY: Duration = Duration::from_secs(1);
pub const CDP_RECONNECT_DELAY: Duration = Duration::from_secs(5);
pub const CDP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
pub const CDP_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);
pub const CDP_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const CONNECT_MAX_RETRIES: usize = 3;
pub const RECONNECT_MAX_RETRIES: usize = 3;
pub const WAIT_FOR_CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
pub const WAIT_FOR_CONNECTION_POLL: Duration = Duration::from_millis(50);
pub const WAIT_FOR_LOAD_TIMEOUT: Duration = Duration::from_secs(30);
pub const WAIT_FOR_LOAD_POLL: Duration = Duration::from_millis(150);
pub const NEW_PAGE_READY_ATTEMPTS: usize = 30;
pub const NEW_PAGE_READY_POLL: Duration = Duration::from_millis(100);
