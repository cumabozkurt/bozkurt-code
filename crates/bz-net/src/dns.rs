//! In-process authoritative DNS responder for the local development TLDs.
//!
//! Any name under a configured TLD (any depth, including the TLD apex) gets `A 127.0.0.1` and
//! `AAAA ::1`; other record types get an empty NOERROR answer with an SOA. Names outside the
//! configured TLDs get `REFUSED` (default) or `NXDOMAIN` — the responder never recurses.
//! Serves UDP and TCP on the same address.

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA, SOA};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::error::{IoContext, NetError, Result};
use crate::resolver::normalize_tld;

/// What to answer for names outside the configured TLDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownPolicy {
    /// `REFUSED` (we are not a recursive resolver). Lets OS resolvers fall through quickly.
    #[default]
    Refused,
    /// `NXDOMAIN`.
    NxDomain,
}

/// Responder configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsConfig {
    /// TLDs answered (normalised, e.g. `["test"]`).
    pub tlds: Vec<String>,
    /// TTL of answers in seconds.
    pub ttl: u32,
    /// Behaviour for names outside `tlds`.
    pub unknown: UnknownPolicy,
}

impl DnsConfig {
    /// Config for `tlds` with TTL 60 and REFUSED for others. Invalid TLDs are an error.
    pub fn new<I, S>(tlds: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let tlds = tlds
            .into_iter()
            .map(|t| normalize_tld(t.as_ref()))
            .collect::<Result<Vec<_>>>()?;
        Ok(DnsConfig {
            tlds,
            ttl: 60,
            unknown: UnknownPolicy::Refused,
        })
    }

    /// The configured TLD that `name` falls under (`"a.b.test."` → `"test"`).
    pub fn matching_tld(&self, name: &str) -> Option<&str> {
        let n = name.trim_end_matches('.').to_ascii_lowercase();
        self.tlds
            .iter()
            .filter(|t| n == **t || n.ends_with(&format!(".{t}")))
            .max_by_key(|t| t.len())
            .map(|s| s.as_str())
    }
}

fn soa_record(apex: &Name, ttl: u32) -> Record {
    let mname = Name::from_ascii("ns.bozkurt.invalid.").unwrap_or_else(|_| Name::root());
    let rname = Name::from_ascii("hostmaster.bozkurt.invalid.").unwrap_or_else(|_| Name::root());
    Record::from_rdata(
        apex.clone(),
        ttl,
        RData::SOA(SOA::new(mname, rname, 1, 3600, 600, 86400, ttl)),
    )
}

/// Build the wire-format response for a wire-format query. `None` when the packet is not a
/// parseable query (it is then silently dropped, like any authoritative server would).
pub fn answer(config: &DnsConfig, packet: &[u8]) -> Option<Vec<u8>> {
    let req = Message::from_vec(packet).ok()?;
    if req.metadata.message_type != MessageType::Query {
        return None;
    }
    let mut resp = Message::response(req.metadata.id, req.metadata.op_code);
    resp.metadata.recursion_desired = req.metadata.recursion_desired;
    resp.metadata.checking_disabled = req.metadata.checking_disabled;
    resp.queries = req.queries.clone();

    if req.metadata.op_code != OpCode::Query {
        resp.metadata.response_code = ResponseCode::NotImp;
        return resp.to_vec().ok();
    }
    let [q]: &[Query; 1] = match req.queries.as_slice().try_into() {
        Ok(q) => q,
        Err(_) => {
            resp.metadata.response_code = ResponseCode::FormErr;
            return resp.to_vec().ok();
        }
    };
    let name = q.name();
    let ascii = name.to_ascii();
    match config.matching_tld(&ascii) {
        Some(tld) => {
            resp.metadata.authoritative = true;
            let ttl = config.ttl;
            let a = || Record::from_rdata(name.clone(), ttl, RData::A(A(Ipv4Addr::LOCALHOST)));
            let aaaa =
                || Record::from_rdata(name.clone(), ttl, RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)));
            match q.query_type() {
                RecordType::A => {
                    resp.add_answer(a());
                }
                RecordType::AAAA => {
                    resp.add_answer(aaaa());
                }
                RecordType::ANY => {
                    resp.add_answer(a());
                    resp.add_answer(aaaa());
                }
                _ => {
                    let apex = Name::from_ascii(format!("{tld}.")).unwrap_or_else(|_| Name::root());
                    resp.authorities.push(soa_record(&apex, ttl));
                }
            }
        }
        None => {
            resp.metadata.response_code = match config.unknown {
                UnknownPolicy::Refused => ResponseCode::Refused,
                UnknownPolicy::NxDomain => ResponseCode::NXDomain,
            };
        }
    }
    resp.to_vec().ok()
}

/// A running responder. Dropping it stops the server (tasks are aborted); prefer
/// [`DnsHandle::stop`] for a graceful shutdown.
#[derive(Debug)]
pub struct DnsHandle {
    addr: SocketAddr,
    config: Arc<RwLock<DnsConfig>>,
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl DnsHandle {
    /// Address served (UDP and TCP).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Current configuration.
    pub fn config(&self) -> DnsConfig {
        self.config
            .read()
            .map(|c| c.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    /// Replace the set of TLDs answered, without restarting.
    pub fn set_tlds<I, S>(&self, tlds: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let new = DnsConfig::new(tlds)?;
        let mut guard = self.config.write().unwrap_or_else(|e| e.into_inner());
        guard.tlds = new.tlds;
        Ok(())
    }

    /// Stop serving and wait for the server tasks to finish.
    pub async fn stop(mut self) {
        let _ = self.shutdown.send(true);
        for t in self.tasks.drain(..) {
            let _ = t.await;
        }
    }

    /// True while the server tasks are running.
    pub fn is_running(&self) -> bool {
        self.tasks.iter().all(|t| !t.is_finished())
    }
}

impl Drop for DnsHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        for t in &self.tasks {
            t.abort();
        }
    }
}

async fn bind_pair(addr: SocketAddr) -> Result<(UdpSocket, TcpListener)> {
    // With port 0, pick the UDP port first and retry until TCP can take the same one.
    let attempts = if addr.port() == 0 { 20 } else { 1 };
    let mut last_err = None;
    for _ in 0..attempts {
        let udp = UdpSocket::bind(addr)
            .await
            .ctx(|| format!("binding DNS UDP {addr}"))?;
        let actual = udp.local_addr().ctx(|| "udp local_addr".to_string())?;
        match TcpListener::bind(actual).await {
            Ok(tcp) => return Ok((udp, tcp)),
            Err(e) => last_err = Some(NetError::io(format!("binding DNS TCP {actual}"), e)),
        }
    }
    Err(last_err.unwrap_or_else(|| NetError::Dns("could not bind".into())))
}

/// Start the responder on `addr` (use `127.0.0.1:<port>`; port 0 picks a free port).
/// Must be called inside a Tokio runtime.
pub async fn start(addr: SocketAddr, config: DnsConfig) -> Result<DnsHandle> {
    let (udp, tcp) = bind_pair(addr).await?;
    let local = udp.local_addr().ctx(|| "udp local_addr".to_string())?;
    let config = Arc::new(RwLock::new(config));
    let (tx, rx) = watch::channel(false);

    let udp_task = {
        let config = config.clone();
        let mut rx = rx.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                tokio::select! {
                    _ = rx.changed() => break,
                    r = udp.recv_from(&mut buf) => {
                        let Ok((n, peer)) = r else { continue };
                        let cfg = config.read().map(|c| c.clone()).unwrap_or_else(|e| e.into_inner().clone());
                        if let Some(resp) = answer(&cfg, &buf[..n]) {
                            let _ = udp.send_to(&resp, peer).await;
                        }
                    }
                }
            }
        })
    };
    let tcp_task = {
        let config = config.clone();
        let mut rx = rx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.changed() => break,
                    r = tcp.accept() => {
                        let Ok((stream, _)) = r else { continue };
                        let config = config.clone();
                        let rx = rx.clone();
                        tokio::spawn(serve_tcp(stream, config, rx));
                    }
                }
            }
        })
    };
    tracing::info!(%local, "DNS responder started");
    Ok(DnsHandle {
        addr: local,
        config,
        shutdown: tx,
        tasks: vec![udp_task, tcp_task],
    })
}

async fn serve_tcp(
    mut stream: TcpStream,
    config: Arc<RwLock<DnsConfig>>,
    mut rx: watch::Receiver<bool>,
) {
    loop {
        let mut len = [0u8; 2];
        let read = tokio::select! {
            _ = rx.changed() => return,
            r = tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut len)) => r,
        };
        if !matches!(read, Ok(Ok(_))) {
            return;
        }
        let n = u16::from_be_bytes(len) as usize;
        let mut msg = vec![0u8; n];
        if !matches!(
            tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut msg)).await,
            Ok(Ok(_))
        ) {
            return;
        }
        let cfg = config
            .read()
            .map(|c| c.clone())
            .unwrap_or_else(|e| e.into_inner().clone());
        let Some(resp) = answer(&cfg, &msg) else {
            return;
        };
        let Ok(rlen) = u16::try_from(resp.len()) else {
            return;
        };
        let mut out = Vec::with_capacity(resp.len() + 2);
        out.extend_from_slice(&rlen.to_be_bytes());
        out.extend_from_slice(&resp);
        if stream.write_all(&out).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str, t: RecordType) -> Vec<u8> {
        let mut m = Message::query();
        m.metadata.recursion_desired = true;
        m.add_query(Query::query(Name::from_ascii(name).unwrap(), t));
        m.to_vec().unwrap()
    }

    fn cfg() -> DnsConfig {
        DnsConfig::new(["test", "local.dev"]).unwrap()
    }

    #[test]
    fn matching() {
        let c = cfg();
        assert_eq!(c.matching_tld("shop.test."), Some("test"));
        assert_eq!(c.matching_tld("A.B.C.Shop.TEST"), Some("test"));
        assert_eq!(c.matching_tld("test"), Some("test"));
        assert_eq!(c.matching_tld("x.local.dev"), Some("local.dev"));
        assert_eq!(c.matching_tld("contest"), None);
        assert_eq!(c.matching_tld("example.com"), None);
    }

    #[test]
    fn answers() {
        let c = cfg();
        let r = Message::from_vec(&answer(&c, &query("api.Shop.test.", RecordType::A)).unwrap())
            .unwrap();
        assert_eq!(r.metadata.response_code, ResponseCode::NoError);
        assert!(r.metadata.authoritative);
        assert!(r.metadata.recursion_desired);
        assert_eq!(r.answers.len(), 1);
        assert_eq!(r.answers[0].data, RData::A(A(Ipv4Addr::LOCALHOST)));
        assert_eq!(r.answers[0].name.to_ascii(), "api.Shop.test.");

        let r =
            Message::from_vec(&answer(&c, &query("x.test.", RecordType::AAAA)).unwrap()).unwrap();
        assert_eq!(r.answers[0].data, RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)));

        let r = Message::from_vec(&answer(&c, &query("x.test.", RecordType::MX)).unwrap()).unwrap();
        assert_eq!(r.metadata.response_code, ResponseCode::NoError);
        assert!(r.answers.is_empty());
        assert_eq!(r.authorities.len(), 1);

        let r =
            Message::from_vec(&answer(&c, &query("example.com.", RecordType::A)).unwrap()).unwrap();
        assert_eq!(r.metadata.response_code, ResponseCode::Refused);
        let mut nx = c.clone();
        nx.unknown = UnknownPolicy::NxDomain;
        let r = Message::from_vec(&answer(&nx, &query("example.com.", RecordType::A)).unwrap())
            .unwrap();
        assert_eq!(r.metadata.response_code, ResponseCode::NXDomain);

        assert!(answer(&c, b"garbage").is_none());
    }
}
