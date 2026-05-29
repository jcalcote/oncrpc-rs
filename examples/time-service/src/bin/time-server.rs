use onc_rpc_server::{Program, ServerBuilder, TokioAsyncServerTransport};
use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use time_service_example::time_service;
use time_service_example::time_service_stubs::time_service::time_service_v1::async_server::{
    TIME_SERVICE_V1Dispatch, TIME_SERVICE_V1Service,
};

struct TimeService;

#[onc_rpc_server::async_trait]
impl TIME_SERVICE_V1Service for TimeService {
    async fn get_time(
        &self,
        _request: &onc_rpc_server::RequestContext,
    ) -> Result<time_service::time_string, onc_rpc_server::DispatchError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| onc_rpc_server::DispatchError::SystemError)?;
        Ok(format!("unix-seconds: {}", now.as_secs()))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind_addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4000".to_string())
        .parse::<SocketAddr>()?;

    let mut server = ServerBuilder::new().with_bind_addr(bind_addr).build_async();
    server.register(
        Program {
            number: time_service::time_service::PROGRAM,
            version: time_service::time_service::time_service_v1::VERSION,
        },
        TIME_SERVICE_V1Dispatch::new(TimeService),
    )?;

    let transport = TokioAsyncServerTransport::bind(server).await?;
    eprintln!("time-server listening on {}", transport.local_addr()?);
    transport.serve().await?;
    Ok(())
}
