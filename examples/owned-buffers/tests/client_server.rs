//! Exercise the two runnable programs over an actual loopback TCP connection.
use std::{net::SocketAddr, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    time::timeout,
};

#[tokio::test]
async fn client_and_server_binaries_exchange_owned_payloads() {
    let mut server = Command::new(env!("CARGO_BIN_EXE_buffer-server"))
        .arg("127.0.0.1:0")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(server.stdout.take().unwrap());
    let mut listening = String::new();
    let bytes = timeout(Duration::from_secs(10), output.read_line(&mut listening))
        .await
        .expect("server did not announce readiness")
        .unwrap();
    assert_ne!(bytes, 0, "server exited before listening");
    let address = listening
        .trim()
        .strip_prefix("buffer-server listening on ")
        .expect("unexpected server announcement")
        .parse::<SocketAddr>()
        .unwrap();
    assert_ne!(address.port(), 0);
    for message in [
        "ownedRPC".to_string(),
        "hello".to_string(),
        String::new(),
        "x".repeat(64),
    ] {
        let output = timeout(
            Duration::from_secs(15),
            Command::new(env!("CARGO_BIN_EXE_buffer-client"))
                .arg(address.to_string())
                .arg(&message)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("client exceeded hang bound")
        .unwrap();
        assert!(
            output.status.success(),
            "client failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("admission refused an unsent call"),
            "{stdout}"
        );
        assert!(
            stdout.contains(&format!(
                "received '{message}' in the original caller allocation"
            )),
            "{stdout}"
        );
        assert!(
            stdout.contains("retried the refused request with its original receiver"),
            "{stdout}"
        );
    }
    server.start_kill().unwrap();
    timeout(Duration::from_secs(5), server.wait())
        .await
        .expect("server did not exit")
        .unwrap();
}

#[tokio::test]
async fn oversized_input_is_rejected_before_connecting() {
    let output = timeout(
        Duration::from_secs(10),
        Command::new(env!("CARGO_BIN_EXE_buffer-client"))
            .arg("127.0.0.1:0")
            .arg("x".repeat(65))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("client exceeded hang bound")
    .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("at most 64 bytes"));
}
