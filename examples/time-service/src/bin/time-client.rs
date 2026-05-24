use onc_rpc_runtime::{AsyncClient, ClientConfig, TokioAsyncClientTransport};
use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;
use time_service_example::time_service_stubs::time_service::time_service_v1::async_client::TIME_SERVICE_V1Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let remote_addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4000".to_string())
        .parse::<SocketAddr>()?;

    let config = ClientConfig::new(remote_addr)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(30));
    let transport = TokioAsyncClientTransport::connect(&config).await?;
    let client = AsyncClient::new(config, transport);
    let stub = TIME_SERVICE_V1Client::new(client);

    println!("{}", stub.get_time().await?);
    Ok(())
}
