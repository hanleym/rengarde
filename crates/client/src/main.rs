#![feature(ip)]
#![feature(test)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use futures::StreamExt;
use network_interface::{NetworkInterface, NetworkInterfaceConfig};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::select;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, info_span, Instrument, trace, warn};

// The maximum transmission unit (MTU) of an Ethernet frame is 1518 bytes with the normal untagged
// Ethernet frame overhead of 18 bytes and the 1500-byte payload.
const BUFFER_SIZE: usize = 1500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    client: ClientSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSettings {
    description: Option<String>,
    listen_addr: String,
    dst_addr: String,
    // Write timeout in milliseconds for socket writes. You can try to lower it if you're experiencing latency peaks, or raising it if the connection is unstable.
    // You can disable write timeout by setting to 0; but it's easy to have issues if you need low latency.
    write_timeout: Option<u64>,
    excluded_interfaces: Vec<String>,
    // dst_overrides: HashMap<String, String>,
    web_manager: Option<WebManager>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebManager {
    listen_addr: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

type SendingRoutines = Arc<DashMap<String, SendingRoutine>>;

struct SendingRoutine {
    ifname: String,
    src_socket: Arc<UdpSocket>,
    src_addr: SocketAddr,
    dst_addr: SocketAddr,
    last_received_at: Instant,
    total_received_bytes: usize,
    is_closing: bool,
}

impl SendingRoutine {
    fn new(ifname: String, src_socket: Arc<UdpSocket>, src_addr: SocketAddr, dst_addr: SocketAddr) -> Self {
        info!(
            event = "added",
            iface_name = ifname,
            src_addr = src_addr.to_string(),
            dst_addr = dst_addr.to_string(),
            "\tAdded interface '{}' to sending routines", ifname
        );
        Self {
            ifname,
            src_socket,
            src_addr,
            dst_addr,
            last_received_at: Instant::now(),
            total_received_bytes: 0,
            is_closing: false,
        }
    }

    // returns key if the routine needs to be closed
    #[tracing::instrument(skip_all)]
    async fn send_to(&mut self, buf: &[u8]) -> Option<String> {
        // TODO: implement a write timeout
        // debug!("\tSending {} bytes on iface {} to client '{:?}'", buf.len(), self.ifname, self.dst_addr);
        match self.src_socket.send_to(buf, self.dst_addr).await {
            Ok(sent_bytes) => {
                trace!(
                    sent_bytes = sent_bytes,
                    dst_ifname = self.ifname,
                    dst_addr = self.dst_addr.to_string(),
                    "\tSent {} bytes on iface {} to client '{:?}'", sent_bytes, self.ifname, self.dst_addr
                );
                // self.last_sent_at = Instant::now();
                // self.total_sent_bytes += sent_bytes;
                None
            }
            Err(err) => {
                warn!(
                    event = "disconnect",
                    dst_addr = self.dst_addr.to_string(),
                    "Error writing to client '{:?}', terminating it: {:?}", self.dst_addr, err
                );
                Some(self.ifname.clone())
            }
        }
    }
}

impl Drop for SendingRoutine {
    fn drop(&mut self) {
        debug!(
            event = "removed",
            iface_name = self.ifname,
            src_addr = self.src_addr.to_string(),
            dst_addr = self.dst_addr.to_string(),
            "\tRemoved interface '{}' from sending routines", self.ifname
        );
    }
}

#[derive(Clone)]
struct Service {
    shutdown: CancellationToken,
    settings: ClientSettings,
    routines: SendingRoutines,
    // wireguard_socket: Arc<UdpSocket>,
    source_addr: Arc<Mutex<SocketAddr>>,
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

    if config_path == "list-interfaces" {
        return list_interfaces();
    }

    let settings = std::fs::read_to_string(&config_path)?;
    let mut settings: Settings = serde_yaml::from_str(&settings)?;
    if let Some(description) = &settings.client.description {
        info!("{}", description);
    }

    // default to 10; but allow 0 for disabling write timeout
    if settings.client.write_timeout.is_none() {
        info!("Write timeout not set; setting to 10ms.");
        settings.client.write_timeout = Some(10);
    }
    // warn if write timeout is enabled; it's not implemented yet
    if !matches!(settings.client.write_timeout, Some(0)) {
        warn!("Write timeout is not implemented yet: setting to 0 to disable!");
        settings.client.write_timeout = Some(0);
    }

    let settings = settings;
    // dbg!(&settings);

    let service = Service::new(settings.client);
    service.run().await?;
    Ok(())
}

fn list_interfaces() -> Result<()> {
    let interfaces = NetworkInterface::show()?;
    for iface in interfaces {
        println!();
        println!("{}", iface.name);
        let if_addr = get_address_by_interface(&iface)
            .map(|ip| ip.to_string())
            .unwrap_or_default();
        println!("  Address: {}", if_addr);
    }
    Ok(())
}

fn get_address_by_interface(iface: &NetworkInterface) -> Option<IpAddr> {
    iface.addr.iter().find_map(|addr| {
        let ip = addr.ip();
        match ip {
            IpAddr::V4(v4) => {
                // include shared ips; used by starlink
                if v4.is_shared() {
                    return Some(ip);
                }
                // exclude everything else that's global
                if !v4.is_global() {
                    return None;
                }
                Some(ip)
            }
            IpAddr::V6(_) => {
                // TODO: handle IPv6
                None
            }
        }
    })
}

impl Service {
    fn new(settings: ClientSettings) -> Self {
        Self {
            shutdown: CancellationToken::new(),
            settings,
            routines: Arc::new(Default::default()),
            source_addr: Arc::new(Mutex::new(
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0)
            )),
        }
    }

    //#[tracing::instrument(skip_all)]
    async fn run(&self) -> Result<()> {
        let settings = &self.settings;

        // let clients: Clients = Arc::new(DashMap::new());

        // let wireguard_addr = settings.client.dst_addr.parse()?;
        let wireguard_socket = Arc::new(UdpSocket::bind(&settings.listen_addr).await?);

        info!("Listening on: {}", &settings.listen_addr);

        if let Some(web_manager) = &settings.web_manager {
            warn!("Web manager is not implemented yet: {:?}", web_manager);
        }

        let join_update_available_interfaces = tokio::spawn({
            let service = self.clone();
            let wireguard_socket = wireguard_socket.clone();
            async move {
                if let Err(err) = service.update_available_interfaces(wireguard_socket).await {
                    warn!("update_available_interfaces thread failed: {:?}", err);
                }
            }
        });

        let join_receive_from_wireguard = tokio::spawn({
            let service = self.clone();
            async move {
                if let Err(err) = service.receive_from_wireguard(wireguard_socket).await {
                    warn!("receive_from_wireguard thread failed: {:?}", err);
                }
            }
        });

        select! {
            _ = tokio::signal::ctrl_c() => {
                info!("ctrl + c received; shutting down...");
            }
            _ = join_update_available_interfaces => {
                warn!("update_available_interfaces thread closed");
            }
            _ = join_receive_from_wireguard => {
                warn!("receive_from_wireguard thread closed");
            }
        }

        debug!("shutdown starting; sending cancel");
        self.shutdown.cancel();
        debug!("shutdown finished; cancel returned");
        Ok(())
    }

    //#[tracing::instrument(skip_all)]
    async fn update_available_interfaces(&self, wireguard_socket: Arc<UdpSocket>) -> Result<()> {
        loop {
            // if self.shutdown.cancelled() {
            //     debug!("Shutdown signal received; closing thread");
            //     return Ok(());
            // }

            debug!("Checking available interfaces...");
            let interfaces = NetworkInterface::show()?;
            // TODO: retry?

            // delete unavailable interfaces
            let drop_list: Vec<_> = self.routines.iter().filter_map(|routine| {
                if self.settings.excluded_interfaces.contains(routine.key()) {
                    warn!("Interface '{}' is excluded; removing it", routine.key());
                    return Some(routine.key().clone());
                }
                match interfaces.iter().find(|interface| &interface.name == routine.key()) {
                    Some(iface) => {
                        match get_address_by_interface(iface) {
                            Some(addr) => {
                                if addr != routine.value().src_addr.ip() {
                                    info!("Interface '{}' address changed; re-creating it", routine.key());
                                    Some(routine.key().clone())
                                } else {
                                    // all good
                                    None
                                }
                            }
                            None => {
                                warn!("Interface '{}' has no address; removing it", routine.key());
                                Some(routine.key().clone())
                            }
                        }
                    }
                    None => {
                        warn!("Interface '{}' no longer exists; removing it", routine.key());
                        Some(routine.key().clone())
                    }
                }
            }).collect();

            // drop the interfaces that are no longer available
            if !drop_list.is_empty() {
                drop_list.into_iter().for_each(|key| {
                    self.routines.remove(&key);
                });
            }

            // create new interfaces
            for iface in interfaces {
                // skip excluded interfaces
                if self.settings.excluded_interfaces.contains(&iface.name) {
                    continue;
                }
                // skip if the interface already exists
                if self.routines.contains_key(&iface.name) {
                    continue;
                }

                if let Some(source_addr) = get_address_by_interface(&iface) {
                    if let Err(err) = self.create_send_thread(&iface, source_addr, wireguard_socket.clone()).await {
                        warn!("Failed to create send thread for interface '{}': {:?}", iface.name, err);
                    }
                    debug!("Created send thread for interface '{}'", iface.name);
                }
            }

            debug!("Checking available interfaces finished; sleeping...");
            select! {
                _ = self.shutdown.cancelled() => {
                    debug!("Shutdown signal received; closing update_available_interfaces thread");
                    return Ok(());
                }
                _ = sleep(std::time::Duration::from_secs(1)) => {}
            }
        }
    }

    //#[tracing::instrument(skip_all)]
    async fn create_send_thread(&self, iface: &NetworkInterface, source_addr: IpAddr, wireguard_socket: Arc<UdpSocket>) -> Result<()> {
        info!("New interface '{}' with IP '{}', adding it", iface.name, source_addr);

        // TODO: allow destination overrides
        let dst_addr = tokio::net::lookup_host(&self.settings.dst_addr)
            .await
            .map_err(|err| anyhow!("Failed to resolve destination address '{}': {:?}", self.settings.dst_addr, err))
            .and_then(|mut addrs| {
                addrs.next().ok_or_else(|| anyhow!("No address found for destination address '{}'", self.settings.dst_addr))
            })?;
        debug!("\tDestination address: '{:?}'", dst_addr);

        let src_addr = SocketAddr::new(source_addr, 0);
        debug!("\tSource address: '{:?}'", src_addr);

        let src_socket = UdpSocket::bind(src_addr).await?;
        debug!("\tBound udp socket to '{}'", src_addr);

        // let src_socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
        // src_socket.set_reuse_address(true)?;
        if !iface.name.is_empty() {
            src_socket.bind_device(Some(iface.name.as_bytes()))?;
            debug!("\tBound udp socket to interface '{}'", iface.name);
        }

        // TODO: should we be using the above, or below src_addr, or maybe even lookup_host?
        // let src_addr = src_socket.local_addr()?;

        let src_socket = Arc::new(src_socket);

        let routine = SendingRoutine::new(
            iface.name.to_owned(),
            src_socket,
            src_addr,
            dst_addr,
        );

        if let Some(routine) = self.routines.insert(iface.name.to_owned(), routine) {
            // TODO: handle this case...
            panic!("Interface '{}' already existed when we tried to add it", routine.ifname);
        };

        tokio::spawn({
            let this = self.clone();
            let ifname = iface.name.to_owned();
            let wireguard_socket = wireguard_socket.clone();
            async move {
                if let Err(err) = this.wireguard_write_back(ifname.clone(), wireguard_socket).await {
                    warn!("wireguard_write_back thread failed: {:?}", err);
                };
                debug!("wireguard_write_back thread closed: '{}'", ifname);
            }
        });
        debug!("\tStarted wireguard_write_back thread for interface '{}'", iface.name);

        Ok(())
    }

    //#[tracing::instrument(skip_all)]
    async fn wireguard_write_back(&self, ifname: String, wireguard_socket: Arc<UdpSocket>) -> Result<()> {
        let mut buf = [0; BUFFER_SIZE];
        loop {
            // if self.shutdown.is_cancelled() {
            //     debug!("Shutdown signal received; closing thread");
            //     return Ok(());
            // }

            let routine = self.routines.get(&ifname).ok_or_else(|| anyhow!("Interface '{}' not found", ifname))?;
            debug!("Got interface {} from routines", ifname);
            if routine.is_closing {
                warn!("Interface '{}' is closing; closing thread", ifname);
                return Ok(());
            }
            // clone the socket
            let socket = routine.src_socket.clone();
            // drop the lock so we can receive from the interface
            drop(routine);

            debug!("Waiting for data from interface '{}'", ifname);
            select! {
                t = socket.recv_from(&mut buf) => {
                    match t {
                        Ok((received_bytes, _)) => {
                            debug!("Received {} bytes from interface '{}'", received_bytes, ifname);
                            let mut routine = self.routines.get_mut(&ifname).ok_or_else(|| anyhow!("Interface '{}' not found", ifname))?;
                            routine.last_received_at = Instant::now();
                            routine.total_received_bytes += received_bytes;
                            // drop the lock so we can send to wireguard
                            drop(routine);

                            // send to wireguard
                            let wg_addr = *self.source_addr.lock().unwrap();
                            wireguard_socket.send_to(&buf[..received_bytes], wg_addr).await?;
                            trace!("\tSent {} bytes to wireguard", received_bytes);
                        }
                        Err(err) => {
                            warn!("Error receiving from interface '{}': {:?}", ifname, err);
                            let mut routine = self.routines.get_mut(&ifname).ok_or_else(|| anyhow!("Interface '{}' not found", ifname))?;
                            routine.is_closing = true;
                        }
                    }
                }
                _ = self.shutdown.cancelled() => {
                    debug!("Shutdown signal received; closing thread");
                    return Ok(());
                }
            }
        }
    }

    //#[tracing::instrument(skip_all)]
    async fn receive_from_wireguard(&self, wireguard_socket: Arc<UdpSocket>) -> Result<()> {
        let mut buf = [0; BUFFER_SIZE];
        loop {
            let span = info_span!("receive_from_wireguard_loop");
            select! {
                _ = self.shutdown.cancelled() => {
                    debug!("Shutdown signal received; closing thread");
                    return Ok(());
                }
                result = wireguard_socket.recv_from(&mut buf).instrument(span) => {
                    match result {
                        Ok((received_bytes, src_addr)) => {
                            *self.source_addr.lock().unwrap() = src_addr;
                            trace!(
                                received_bytes = received_bytes,
                                src_addr = src_addr.to_string(),
                                "Received {} bytes from wireguard on '{:?}'", received_bytes, src_addr
                            );
                            trace!("\tSending to {} clients", self.routines.len());

                            // send to interfaces in parallel using streams
                            let drop_list = futures::stream::iter(self.routines.iter_mut())
                                .filter_map(|mut routine| async move {
                                    routine.send_to(&buf[..received_bytes]).await
                                })
                                .collect::<Vec<String>>()
                                .await;

                            // drop the clients that have timed out
                            if !drop_list.is_empty() {
                                // TODO: can we do this with DashMap without locking each iteration?
                                // let mut routines = self.routines.write().unwrap();
                                drop_list.into_iter().for_each(|ifname| {
                                    self.routines.remove(&ifname);
                                });
                                // drop(routines);
                            }

                            trace!("Sent to {} clients", self.routines.len());
                        }
                        Err(err) => {
                            warn!("Error receiving from wireguard: {:?}", err);
                        }
                    }
                }
            }
        }
    }
}
