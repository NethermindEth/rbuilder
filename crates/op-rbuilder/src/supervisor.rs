use std::time::Duration;
use jsonrpsee::http_client::HttpClient;
use kona_rpc::ExecutingMessageValidator;

pub struct SupervisorValidator;

impl ExecutingMessageValidator for SupervisorValidator {
    type SupervisorClient = HttpClient;
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);
}
