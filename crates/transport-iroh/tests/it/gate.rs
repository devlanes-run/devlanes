//! The direct-path gate against a box only a relay can reach, with the relay
//! running in this process on loopback over plain HTTP.

use std::time::{Duration, Instant};

use devlanes_transport::{Alpn, BoxError, PeerAddr, PeerEndpoint, PeerError, PeerId, TUNNEL_ALPN};
use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, RelayUrl};
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use tokio::sync::mpsc;

use super::{Home, TestResult, bounded};

/// A relay on a free loopback port, without TLS: iroh's relay client takes
/// `http` URLs, so the tests need no certificate.
async fn relay() -> Result<(Server, String), BoxError> {
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new(([127, 0, 0, 1], 0)));
    let server = Server::spawn(config).await?;
    let addr = server.http_addr().ok_or("the relay has no HTTP address")?;
    Ok((server, format!("http://{addr}")))
}

/// A box with no IP transport, so the relay is its only path, and a dial
/// from a client that knows its relay. The handshake crosses the relay and
/// the box accepts the connection, but the dial fails as transient after the
/// gate's wait, and the box never gets a stream: the client closes the
/// connection instead of using the relay for data.
#[tokio::test]
async fn the_direct_gate_fails_cleanly_on_a_relay_only_box() -> TestResult {
    bounded(Duration::from_secs(30), async {
        let (_relay, url) = relay().await?;
        let boxed = Endpoint::builder(presets::Minimal)
            .clear_ip_transports()
            .relay_mode(RelayMode::Custom(url.parse::<RelayUrl>()?.into()))
            .alpns(vec![TUNNEL_ALPN.to_vec()])
            .bind()
            .await?;
        boxed.online().await;
        let (accepted_tx, mut accepted) = mpsc::unbounded_channel();
        let accepting = boxed.clone();
        tokio::spawn(async move {
            while let Some(incoming) = accepting.accept().await {
                if let Ok(connection) = incoming.await {
                    let _ = accepted_tx.send(connection);
                }
            }
        });

        let home = Home::new()?;
        let client = home.endpoint("client", vec![url.clone()]);
        let addr = PeerAddr {
            id: PeerId(*boxed.id().as_bytes()),
            relay: Some(url),
            direct: Vec::new(),
        };
        let started = Instant::now();
        let Err(error) = client.connect(&addr, Alpn::Tunnel).await else {
            return Err("a relay-only box was dialed".into());
        };
        assert!(!error.is_permanent(), "{error}");
        assert!(
            matches!(
                error.downcast_ref::<PeerError>(),
                Some(PeerError::NoDirectPath { .. })
            ),
            "{error}"
        );
        assert!(
            started.elapsed() >= Duration::from_secs(15),
            "gave up early"
        );

        let connection = accepted.recv().await.ok_or("the box got no connection")?;
        assert!(
            connection.accept_bi().await.is_err(),
            "a stream reached the box over the relay"
        );
        boxed.close().await;
        Ok(())
    })
    .await
}
