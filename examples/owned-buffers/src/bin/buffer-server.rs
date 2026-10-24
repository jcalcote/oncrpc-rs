use onc_rpc_server::{Program, ServerBuilder, TokioAsyncServerTransport};
use owned_buffers_example::buffer_echo;
use owned_buffers_example::buffer_echo_stubs::buffer_echo::buffer_echo_v1::async_server::{
    BUFFER_ECHO_V1Dispatch, BUFFER_ECHO_V1Service,
};
use std::{
    env,
    error::Error,
    io::{self, Write},
    net::SocketAddr,
};

struct Echo;

#[onc_rpc_server::async_trait]
impl BUFFER_ECHO_V1Service for Echo {
    async fn echo(
        &self,
        _request: &onc_rpc_server::RequestContext,
        data: buffer_echo::echo_data,
    ) -> Result<buffer_echo::echo_data, onc_rpc_server::DispatchError> {
        let maximum = usize::try_from(buffer_echo::ECHO_MAX)
            .map_err(|_| onc_rpc_server::DispatchError::SystemError)?;
        if data.len() > maximum {
            return Err(onc_rpc_server::DispatchError::GarbageArgs);
        }
        Ok(data)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4001".into())
        .parse::<SocketAddr>()?;
    let mut server = ServerBuilder::new().with_bind_addr(address).build_async();
    server.register(
        Program {
            number: buffer_echo::buffer_echo::PROGRAM,
            version: buffer_echo::buffer_echo::buffer_echo_v1::VERSION,
        },
        BUFFER_ECHO_V1Dispatch::new(Echo),
    )?;
    let transport = TokioAsyncServerTransport::bind(server).await?;
    println!("buffer-server listening on {}", transport.local_addr()?);
    io::stdout().flush()?;
    transport.serve().await?;
    Ok(())
}
