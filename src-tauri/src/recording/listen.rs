//! Where the service listens, and its accept loop.
//!
//! With network access off it listens on `127.0.0.1` only. On, it listens on
//! every interface (`0.0.0.0`, and `::` where the machine has IPv6), on the
//! same port, so the app keeps reaching it at `127.0.0.1` either way.
//!
//! Its own accept loop rather than `axum::serve`, for three things that one
//! cannot do:
//!
//! - **Plain HTTP and TLS on one port.** The first byte of a connection says
//!   which it is (a TLS handshake starts with `0x16`), so this computer's
//!   app keeps plain HTTP while other devices get HTTPS. Whether a plain
//!   request from another machine is then answered is `recording::access`'s
//!   rule, not the socket's.
//! - **Who is calling.** Each request carries a [`Peer`]: its address and
//!   whether it came over TLS, which is what the access rules go by.
//! - **Restarting without restarting the service.** [`Listening::stop`]
//!   closes the listening sockets and returns once they are closed, so the
//!   same port can be bound again at once with other settings, while the
//!   connections already open finish what they are doing (the request that
//!   changed the settings included) and then close. Recordings carry on
//!   throughout: they are not the listener's.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use axum::Router;
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tower::ServiceExt;

pub use super::access::Peer;

/// How long a connection may take to say anything, and a TLS handshake to
/// finish: a client that opens a connection and waits does not keep it.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a request's headers may take to arrive.
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);

/// The addresses to listen on.
pub fn wanted(network: bool, port: u16) -> Vec<SocketAddr> {
    if network {
        vec![
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        ]
    } else {
        vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))]
    }
}

/// Binds every address in `addrs`, or none: an error closes those already
/// bound. `::` is the exception, left out when the machine has no IPv6 so
/// long as an IPv4 address was bound.
pub fn bind(addrs: &[SocketAddr]) -> io::Result<Vec<TcpListener>> {
    let mut out = Vec::new();
    for &addr in addrs {
        match bind_one(addr) {
            Ok(l) => out.push(l),
            Err(e)
                if addr.ip() == IpAddr::V6(Ipv6Addr::UNSPECIFIED)
                    && !out.is_empty()
                    && e.kind() != io::ErrorKind::AddrInUse =>
            {
                tracing::info!("not listening on {addr} ({e}): no IPv6 here");
            }
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    if e.kind() == io::ErrorKind::AddrInUse {
                        format!("port {} is in use by another program", addr.port())
                    } else {
                        format!("cannot listen on {addr}: {e}")
                    },
                ))
            }
        }
    }
    Ok(out)
}

fn bind_one(addr: SocketAddr) -> io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if addr.is_ipv6() {
        // `::` alone takes IPv4 too on most systems, which `0.0.0.0` then
        // finds taken: each family gets its own socket.
        s.set_only_v6(true)?;
    }
    // As tokio's own bind does: on Unix, a port whose last connections are
    // still in TIME_WAIT can be listened on again at once (a restart with
    // other settings). Not on Windows, where the same option would let
    // another program take a port that is in use.
    #[cfg(unix)]
    s.set_reuse_address(true)?;
    s.set_nonblocking(true)?;
    s.bind(&addr.into())?;
    s.listen(1024)?;
    TcpListener::from_std(s.into())
}

/// The listeners running, and how to stop them.
pub struct Listening {
    stop: watch::Sender<bool>,
    loops: Vec<JoinHandle<()>>,
    /// Where they listen, ports resolved (a port 0 asked for is the one the
    /// system chose).
    pub addrs: Vec<SocketAddr>,
    pub tls: bool,
}

/// Serves `app` on every one of `listeners`; with `tls`, a connection that
/// opens with a TLS handshake is served HTTPS.
pub fn start(
    listeners: Vec<TcpListener>,
    app: Router,
    tls: Option<tokio_rustls::TlsAcceptor>,
) -> Listening {
    let (stop, rx) = watch::channel(false);
    let addrs = listeners
        .iter()
        .filter_map(|l| l.local_addr().ok())
        .collect();
    let loops = listeners
        .into_iter()
        .map(|l| tokio::spawn(accept_loop(l, app.clone(), tls.clone(), rx.clone())))
        .collect();
    Listening {
        stop,
        loops,
        addrs,
        tls: tls.is_some(),
    }
}

impl Listening {
    /// Closes the listening sockets, returning once they are closed. The
    /// connections already open finish their request and close.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        for l in self.loops {
            let _ = l.await;
        }
    }
}

async fn accept_loop(
    listener: TcpListener,
    app: Router,
    tls: Option<tokio_rustls::TlsAcceptor>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let accepted = tokio::select! {
            r = listener.accept() => r,
            _ = stop.changed() => break,
        };
        match accepted {
            Ok((tcp, addr)) => {
                let _ = tcp.set_nodelay(true);
                tokio::spawn(connection(
                    tcp,
                    addr,
                    app.clone(),
                    tls.clone(),
                    stop.clone(),
                ));
            }
            Err(e) => {
                // Out of file descriptors and the like: wait a little
                // rather than spin.
                tracing::warn!("accepting a connection: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    // `listener` is dropped here, which closes the socket.
}

async fn connection(
    tcp: TcpStream,
    addr: SocketAddr,
    app: Router,
    tls: Option<tokio_rustls::TlsAcceptor>,
    stop: watch::Receiver<bool>,
) {
    let mut first = [0u8; 1];
    let tls = match tls {
        Some(t) => t,
        None => {
            serve(tcp, Peer { addr, tls: false }, app, stop).await;
            return;
        }
    };
    match tokio::time::timeout(HELLO_TIMEOUT, tcp.peek(&mut first)).await {
        Ok(Ok(1)) => {}
        _ => return,
    }
    // 0x16: a TLS record of type "handshake", which no HTTP request starts
    // with.
    if first[0] != 0x16 {
        serve(tcp, Peer { addr, tls: false }, app, stop).await;
        return;
    }
    match tokio::time::timeout(HELLO_TIMEOUT, tls.accept(tcp)).await {
        Ok(Ok(stream)) => serve(stream, Peer { addr, tls: true }, app, stop).await,
        Ok(Err(e)) => tracing::debug!("TLS handshake with {addr} failed: {e}"),
        Err(_) => tracing::debug!("TLS handshake with {addr} timed out"),
    }
}

/// One HTTP/1.1 connection, every request tagged with `peer`.
async fn serve<I>(io: I, peer: Peer, app: Router, mut stop: watch::Receiver<bool>)
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let svc = hyper::service::service_fn(move |mut req: hyper::Request<Incoming>| {
        req.extensions_mut().insert(peer);
        app.clone().oneshot(req)
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .serve_connection(TokioIo::new(io), svc);
    tokio::pin!(conn);
    tokio::select! {
        r = conn.as_mut() => {
            if let Err(e) = r {
                tracing::debug!("connection from {}: {e}", peer.addr);
            }
        }
        _ = stop.changed() => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.await;
        }
    }
}

/// Where other devices reach the service: each address of this computer's
/// network interfaces that is neither loopback nor link-local (an IPv6
/// `fe80::` address needs a zone that browsers do not take), as a URL.
pub fn addresses(port: u16, https: bool) -> Vec<String> {
    let scheme = if https { "https" } else { "http" };
    interface_ips()
        .into_iter()
        .map(|ip| match ip {
            IpAddr::V4(v4) => format!("{scheme}://{v4}:{port}"),
            IpAddr::V6(v6) => format!("{scheme}://[{v6}]:{port}"),
        })
        .collect()
}

/// This computer's addresses on its networks, the likeliest first.
pub fn interface_ips() -> Vec<IpAddr> {
    let found: Vec<(String, IpAddr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| (i.name.clone(), i.ip()))
        .collect();
    usable_ips(&found)
}

/// Of `(interface, address)` pairs, those another device could use, the
/// likeliest first: home-network IPv4 (`192.168.…`, then `10.…`, then
/// `172.16–31.…`), other IPv4, then IPv6. Loopback and link-local addresses
/// go, and so do the bridges containers and virtual machines make, which
/// only this computer reaches (Docker alone can add a dozen).
pub fn usable_ips(found: &[(String, IpAddr)]) -> Vec<IpAddr> {
    const VIRTUAL: [&str; 10] = [
        "docker", "br-", "veth", "virbr", "vmnet", "vboxnet", "cni", "flannel", "podman", "lxc",
    ];
    let mut ips: Vec<IpAddr> = found
        .iter()
        .filter(|(name, _)| !VIRTUAL.iter().any(|v| name.starts_with(v)))
        .map(|(_, ip)| ip.to_canonical())
        .filter(|ip| match ip {
            IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
            IpAddr::V6(v6) => {
                !v6.is_loopback() && !v6.is_unicast_link_local() && !v6.is_unspecified()
            }
        })
        .collect();
    let rank = |ip: &IpAddr| match ip {
        IpAddr::V4(v4) => match v4.octets() {
            [192, 168, ..] => 0,
            [10, ..] => 1,
            [172, b, ..] if (16..32).contains(&b) => 2,
            _ => 3,
        },
        IpAddr::V6(_) => 4,
    };
    ips.sort_by_key(|ip| (rank(ip), *ip));
    ips.dedup();
    ips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_loopback_only_and_on_is_every_interface() {
        assert_eq!(wanted(false, 8796), vec!["127.0.0.1:8796".parse().unwrap()]);
        assert_eq!(
            wanted(true, 8796),
            vec![
                "0.0.0.0:8796".parse().unwrap(),
                "[::]:8796".parse().unwrap()
            ]
        );
    }

    #[tokio::test]
    async fn a_port_in_use_is_said_so() {
        let first = bind(&["127.0.0.1:0".parse().unwrap()]).unwrap();
        let port = first[0].local_addr().unwrap().port();
        let err = bind(&[SocketAddr::from((Ipv4Addr::LOCALHOST, port))]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert_eq!(
            err.to_string(),
            format!("port {port} is in use by another program")
        );
    }

    #[tokio::test]
    async fn stopping_frees_the_port_at_once() {
        let l = bind(&["127.0.0.1:0".parse().unwrap()]).unwrap();
        let addr = l[0].local_addr().unwrap();
        let running = start(l, Router::new(), None);
        assert_eq!(running.addrs, vec![addr]);
        running.stop().await;
        let again = bind(&[addr]).unwrap();
        assert_eq!(again[0].local_addr().unwrap(), addr);
    }

    #[test]
    fn the_addresses_offered_are_ones_another_device_can_use() {
        let found: Vec<(String, IpAddr)> = [
            ("lo", "127.0.0.1"),
            ("lo", "::1"),
            ("docker0", "172.17.0.1"),
            ("br-442fd2bad859", "172.27.4.1"),
            ("tun0", "10.1.0.5"),
            ("wlp1s0", "192.168.2.99"),
            ("wlp1s0", "fe80::1c2d:3eff:fe4f:5a6b"),
            ("wlp1s0", "fd00::99"),
            ("enx", "169.254.10.1"),
            ("eth0", "172.20.0.5"),
            ("eth1", "203.0.113.5"),
        ]
        .into_iter()
        .map(|(n, ip)| (n.to_string(), ip.parse().unwrap()))
        .collect();
        let got: Vec<String> = usable_ips(&found).iter().map(|ip| ip.to_string()).collect();
        assert_eq!(
            got,
            [
                "192.168.2.99",
                "10.1.0.5",
                "172.20.0.5",
                "203.0.113.5",
                "fd00::99"
            ]
        );
    }

    #[test]
    fn addresses_are_urls_without_loopback() {
        for a in addresses(8796, true) {
            assert!(a.starts_with("https://") && a.ends_with(":8796"), "{a}");
            assert!(!a.contains("127.0.0.1") && !a.contains("[::1]"), "{a}");
        }
        for a in addresses(9000, false) {
            assert!(a.starts_with("http://"), "{a}");
        }
    }
}
