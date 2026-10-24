//! Run `buffer-server` first, then `buffer-client [address] [message]`.
//! Protocol encoding and direct receive placement come from generated interfaces.
use bytes::Bytes;
use onc_rpc_runtime::{
    AdmissionError, AdmissionLimits, AsyncClient, CallOptions, ClientConfig, OwnedRpcTransport,
    TypedReceiveOutcome,
};
use owned_buffers_example::{
    buffer_echo, buffer_echo_stubs::buffer_echo::buffer_echo_v1::async_client::BUFFER_ECHO_V1Client,
};
use std::{env, net::SocketAddr, time::Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(10), run()).await??;
    Ok(())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let address = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4001".into())
        .parse::<SocketAddr>()?;
    let bytes = env::args()
        .nth(2)
        .unwrap_or_else(|| "ownedRPC".into())
        .into_bytes();
    let maximum = usize::try_from(buffer_echo::ECHO_MAX)?;
    if bytes.len() > maximum {
        return Err(format!("message must contain at most {maximum} bytes").into());
    }
    let config = ClientConfig::new(address)
        .with_connect_timeout(Duration::from_secs(5))
        .with_default_call_timeout(Duration::from_secs(5));
    let transport = OwnedRpcTransport::connect_with_admission(
        &config,
        AdmissionLimits {
            max_calls: 1,
            max_bytes: 4096,
        },
    )
    .await?;
    let client = BUFFER_ECHO_V1Client::new(AsyncClient::new(config, transport.clone()));
    let original_send = bytes.as_ptr();
    let data = Bytes::from_owner(bytes);
    if !data.is_empty() {
        assert_eq!(data.as_ptr(), original_send);
    }
    let buffer = vec![0; maximum];
    let original_receive = buffer.as_ptr();
    let options = CallOptions::default();
    // Generated code supplies the XDR prefix, segmented body, padding and result
    // metadata parser. The application supplies only typed values and storage.
    let first = client.try_echo_into(data.clone(), buffer, &options)?;
    let retry_buffer = vec![0; maximum];
    let retry_original = retry_buffer.as_ptr();
    let refused = match client.try_echo_into(data.clone(), retry_buffer, &options) {
        Err(refused) => refused,
        Ok(_) => return Err("second call should have been refused".into()),
    };
    assert_eq!(refused.reason, AdmissionError::AtCapacity);
    assert!(!transport.is_closed());
    println!("PASS: admission refused an unsent call and returned both owners");
    check_echo(first.await, original_receive, &data)?;
    println!(
        "PASS: received '{}' in the original caller allocation",
        String::from_utf8_lossy(&data)
    );
    transport.wait_for_capacity(refused.required_bytes).await?;
    // Refusal preserves the typed request and original destination. Retry through
    // the same generated interface after capacity becomes available.
    let outcome = client
        .try_echo_into(refused.request, refused.receiver, &options)?
        .await;
    check_echo(outcome, retry_original, &data)?;
    println!("PASS: retried the refused request with its original receiver");
    transport.close();
    Ok(())
}

fn check_echo(
    outcome: TypedReceiveOutcome<buffer_echo::echo_dataMetadata, Vec<u8>>,
    original: *const u8,
    expected: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(outcome.buffer.as_ptr(), original);
    let metadata = outcome.result?;
    assert_eq!(metadata.len(), expected.len());
    assert_eq!(outcome.payload_len, expected.len());
    assert_eq!(&outcome.buffer[..outcome.payload_len], expected);
    Ok(())
}
