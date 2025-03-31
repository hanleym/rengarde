use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{debug, info, trace, warn};

// The maximum transmission unit (MTU) of an Ethernet frame is 1518 bytes with the normal untagged
// Ethernet frame overhead of 18 bytes and the 1500-byte payload.
const BUFFER_SIZE: usize = 1500;

// type Clients = Arc<RwLock<HashMap<SocketAddr, Client>>>;
type Clients = Arc<RwLock<HashMap<SocketAddr, Client>>>;

struct Client {
    last_received_at: Instant,
    total_received_bytes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    server: Server,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    description: Option<String>,
    listen_addr: String,
    dst_addr: String,
    // Client timeout in seconds. If a client doesn't send any packet for n seconds, engarde stops sending it packets.
    // You will need to set it to a slightly higher value than the PersistentKeepalive option in WireGuard clients.
    client_timeout: Option<u64>,
    // Write timeout in milliseconds for socket writes. You can try to lower it if you're experiencing latency peaks, or raising it if the connection is unstable.
    // You can disable write timeout by setting to 0; but it's easy to have issues if you need low latency.
    write_timeout: Option<u64>,
    web_manager: Option<WebManager>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebManager {
    listen_addr: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cargo_pkg_name = env!("CARGO_PKG_NAME");
    let cargo_pkg_version = env!("CARGO_PKG_VERSION");
    let git_rev = option_env!("GIT_REV");
    shared::print_header(cargo_pkg_name, cargo_pkg_version, git_rev);

    let guard = shared::init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("engarde.yml"));

    let settings = std::fs::read_to_string(&config_path)?;
    let mut settings: Settings = serde_yaml::from_str(&settings)?;
    if let Some(description) = &settings.server.description {
        info!("{}", description);
    }

    if matches!(settings.server.client_timeout, None | Some(0)) {
        info!("Client timeout not set; setting to 30s.");
        settings.server.client_timeout = Some(30);
    }

    // default to 10; but allow 0 for disabling write timeout
    if settings.server.write_timeout.is_none() {
        info!("Write timeout not set; setting to 10ms.");
        settings.server.write_timeout = Some(10);
    }
    // warn if write timeout is enabled; it's not implemented yet
    if !matches!(settings.server.write_timeout, Some(0)) {
        warn!("Write timeout is not implemented yet: setting to 0 to disable!");
        settings.server.write_timeout = Some(0);
    }

    let settings = settings;
    // dbg!(&settings);

    // let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
    let clients: Clients = Arc::new(RwLock::new(HashMap::new()));

    // wireguard_addr:  = settings.server.dst_addr.parse()?;
    let wireguard_socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
    let client_socket = Arc::new(UdpSocket::bind(&settings.server.listen_addr).await?);

    info!("Listening on: {}", &settings.server.listen_addr);

    if let Some(web_manager) = &settings.server.web_manager {
        warn!("Web manager is not implemented yet: {:?}", web_manager);
    }

    let join_receive_from_client = tokio::spawn({
        // let service = service.clone();
        let clients = clients.clone();
        let client_socket = client_socket.clone();
        let wireguard_socket = wireguard_socket.clone();
        async move {
            if let Err(err) = receive_from_client(
                clients,
                client_socket,
                wireguard_socket,
                &settings.server.dst_addr,
            )
            .await
            {
                warn!("receive_from_client failed: {:?}", err);
            }
        }
    });

    let join_receive_from_wireguard = tokio::spawn({
        // let service = service.clone();
        let clients = clients.clone();
        let wireguard_socket = wireguard_socket.clone();
        let client_socket = client_socket.clone();
        async move {
            if let Err(err) = receive_from_wireguard(
                clients,
                wireguard_socket,
                client_socket,
                settings.server.client_timeout.unwrap(),
                settings.server.write_timeout.unwrap(),
            )
            .await
            {
                panic!("receive_from_wireguard thread failed: {:?}", err);
            }
        }
    });

    join_receive_from_client.await?;
    join_receive_from_wireguard.await?;
    warn!("All threads joined; exiting...");

    drop(guard);
    Ok(())
}

#[tracing::instrument(skip_all)]
async fn receive_from_client(
    clients: Clients,
    client_socket: Arc<UdpSocket>,
    wireguard_socket: Arc<UdpSocket>,
    wireguard_addr: &str,
) -> Result<()> {
    // tracing::info!(histogram.baz = 10, "histogram example",);

    let mut buf = [0; BUFFER_SIZE];
    loop {
        let (received_bytes, src_addr) = client_socket.recv_from(&mut buf).await?;
        let received_at = Instant::now();

        trace!(
            received_bytes = received_bytes,
            src_addr = src_addr.to_string(),
            "Received {} bytes from client '{:?}'",
            received_bytes,
            src_addr
        );

        // update the client last received timestamp
        let mut clients_locked = clients.write().await;
        clients_locked
            .entry(src_addr)
            .and_modify(|client| {
                client.last_received_at = received_at;
                client.total_received_bytes += received_bytes;
            })
            .or_insert_with(|| {
                info!("New client connected: '{:?}'", src_addr);
                Client {
                    last_received_at: received_at,
                    total_received_bytes: received_bytes,
                }
            });
        drop(clients_locked);

        // send to wireguard
        wireguard_socket
            .send_to(&buf[..received_bytes], wireguard_addr)
            .await?;
        trace!(
            // sent_bytes = received_bytes,
            // dst_addr = wireguard_addr,
            "\tSent {} bytes to wireguard on '{:?}'",
            received_bytes,
            wireguard_addr
        );
    }
}

#[tracing::instrument(skip_all)]
async fn receive_from_wireguard(
    clients: Clients,
    wireguard_socket: Arc<UdpSocket>,
    client_socket: Arc<UdpSocket>,
    client_timeout: u64,
    _write_timeout: u64,
) -> Result<()> {
    let mut buf = [0; BUFFER_SIZE];
    loop {
        let received_bytes = wireguard_socket.recv(&mut buf).await?;
        let received_at = Instant::now();

        debug!(
            // received_bytes = received_bytes,
            // src_addr = ,
            "Received {} bytes from wireguard",
            received_bytes
        );

        let clients_locked = clients.read().await;
        let (mut drop_list, send_list) = clients_locked.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut drop_addrs, mut send_addrs), (addr, client)| {
                if received_at
                    .duration_since(client.last_received_at)
                    .as_secs()
                    > client_timeout
                {
                    drop_addrs.push(addr.clone());
                } else {
                    send_addrs.push(addr.clone());
                }
                (drop_addrs, send_addrs)
            },
        );
        drop(clients_locked);

        drop_list.append(
            futures::stream::iter(send_list.into_iter())
                .filter_map(|addr| {
                    let client_socket = client_socket.clone();
                    async move {
                        if client_socket
                            .send_to(&buf[..received_bytes], &addr)
                            .await
                            .is_err()
                        {
                            warn!("Error writing to client '{:?}', terminating it", addr);
                            return Some(addr);
                        }

                        trace!(
                            sent_bytes = received_bytes,
                            dst_addr = addr.to_string(),
                            "\tSent {} bytes to client '{:?}'",
                            received_bytes,
                            addr
                        );
                        None
                    }
                })
                .collect::<Vec<_>>()
                .await
                .as_mut(),
        );

        // drop the clients that have timed out
        if !drop_list.is_empty() {
            let mut clients_locked = clients.write().await;
            drop_list.into_iter().for_each(|addr| {
                clients_locked.remove(&addr);
            });
            drop(clients_locked);
        }
    }
}
