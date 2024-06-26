use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use dashmap::DashMap;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
// use tokio::time::sleep;
use tracing::{debug, info, trace, warn};

// The maximum transmission unit (MTU) of an Ethernet frame is 1518 bytes with the normal untagged
// Ethernet frame overhead of 18 bytes and the 1500-byte payload.
const BUFFER_SIZE: usize = 1500;

// type Clients = Arc<RwLock<HashMap<SocketAddr, Client>>>;
type Clients = Arc<DashMap<SocketAddr, Client>>;

struct Client {
    addr: SocketAddr,
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
    let _guard = shared::init()?;

    let rengarde_official_build = option_env!("RENGARDE_OFFICIAL_BUILD").unwrap_or("false").parse::<bool>()?;
    let cargo_pkg_name = env!("CARGO_PKG_NAME");
    let cargo_pkg_version = env!("CARGO_PKG_VERSION");
    let vergen_git_describe = env!("VERGEN_GIT_DESCRIBE");
    let vergen_git_dirty = env!("VERGEN_GIT_DIRTY");
    let vergen_build_timestamp = env!("VERGEN_BUILD_TIMESTAMP");
    let vergen_cargo_target_triple = env!("VERGEN_CARGO_TARGET_TRIPLE");
    let rust_runtime = if cfg!(feature = "rt-rayon") {
        "rayon"
    } else if cfg!(feature = "rt-tokio") {
        "tokio"
    } else {
        unimplemented!("No runtime feature enabled");
    };

    shared::print_header(
        rengarde_official_build,
        cargo_pkg_name,
        cargo_pkg_version,
        vergen_git_describe,
        vergen_git_dirty,
        vergen_build_timestamp,
        vergen_cargo_target_triple,
        rust_runtime,
    );

    let config_path = std::env::args().nth(1).unwrap_or_else(|| {
        String::from("engarde.yml")
    });

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
    let clients: Clients = Arc::new(DashMap::new());

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
            if let Err(err) = receive_from_client(clients, client_socket, wireguard_socket, &settings.server.dst_addr).await {
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
            if let Err(err) = receive_from_wireguard(clients, wireguard_socket, client_socket, settings.server.client_timeout.unwrap(), settings.server.write_timeout.unwrap()).await {
                panic!("receive_from_wireguard thread failed: {:?}", err);
            }
        }
    });

    join_receive_from_client.await.unwrap();
    join_receive_from_wireguard.await.unwrap();
    warn!("All threads joined; exiting...");

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn receive_from_client(clients: Clients, client_socket: Arc<UdpSocket>, wireguard_socket: Arc<UdpSocket>, wireguard_addr: &str) -> Result<()> {
    // tracing::info!(histogram.baz = 10, "histogram example",);

    let mut buf = [0; BUFFER_SIZE];
    loop {
        let (received_bytes, src_addr) = client_socket.recv_from(&mut buf).await?;
        let received_at = Instant::now();

        trace!(
            received_bytes = received_bytes,
            src_addr = src_addr.to_string(),
            "Received {} bytes from client '{:?}'", received_bytes, src_addr
        );

        // update the client last received timestamp
        clients.entry(src_addr).and_modify(|client| {
            client.last_received_at = received_at;
            client.total_received_bytes += received_bytes;
        }).or_insert_with(|| {
            info!("New client connected: '{:?}'", src_addr);
            Client {
                addr: src_addr,
                last_received_at: received_at,
                total_received_bytes: received_bytes,
            }
        });

        // send to wireguard
        wireguard_socket.send_to(&buf[..received_bytes], wireguard_addr).await?;
        trace!(
            // sent_bytes = received_bytes,
            // dst_addr = wireguard_addr,
            "\tSent {} bytes to wireguard on '{:?}'", received_bytes, wireguard_addr
        );
    }
}

#[tracing::instrument(skip_all)]
async fn receive_from_wireguard(clients: Clients, wireguard_socket: Arc<UdpSocket>, client_socket: Arc<UdpSocket>, client_timeout: u64, _write_timeout: u64) -> Result<()> {
    let mut buf = [0; BUFFER_SIZE];
    loop {
        let received_bytes = wireguard_socket.recv(&mut buf).await?;
        let received_at = Instant::now();

        debug!(
            // received_bytes = received_bytes,
            // src_addr = ,
            "Received {} bytes from wireguard", received_bytes
        );

        // send to clients
        let drop_list: Vec<_> = futures::stream::iter(clients.iter())
            .filter_map(|client| {
                let client_socket = client_socket.clone();
                async move {
                    // check if the client has timed out
                    if received_at.duration_since(client.last_received_at).as_secs() > client_timeout {
                        warn!("Client '{:?}' timed out", client.addr);
                        return Some(client.addr);
                    }
                    // implement a write timeout
                    // if write_timeout > 0 {
                    //
                    // }
                    // send to client
                    if client_socket.send_to(&buf[..received_bytes], &client.addr).await.is_err() {
                        warn!("Error writing to client '{:?}', terminating it", client.addr);
                        return Some(client.addr);
                    }

                    trace!(
                        sent_bytes = received_bytes,
                        dst_addr = client.addr.to_string(),
                        "\tSent {} bytes to client '{:?}'", received_bytes, client.addr
                    );
                    None
                }
            })
            .collect::<Vec<_>>()
            .await;

        // drop the clients that have timed out
        if !drop_list.is_empty() {
            // TODO: can we do this with DashMap without locking each iteration?
            // let mut clients_locked = clients.write().unwrap();
            drop_list.into_iter().for_each(|addr| {
                clients.remove(&addr);
            });
            // drop(clients_locked);
        }
    }
}
