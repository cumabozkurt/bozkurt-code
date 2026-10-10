//! End-to-end: start the responder on a random high port and query it with hickory-resolver
//! over UDP and over TCP.

use bz_net::NetService;
use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolveHosts, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::Resolver;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

fn resolver(port: u16, tcp: bool) -> Resolver<TokioRuntimeProvider> {
    let mut conn = if tcp {
        ConnectionConfig::tcp()
    } else {
        ConnectionConfig::udp()
    };
    conn.port = port;
    let ns = NameServerConfig::new(IpAddr::V4(Ipv4Addr::LOCALHOST), true, vec![conn]);
    let mut b = Resolver::builder_with_config(
        ResolverConfig::from_name_servers(vec![ns]),
        TokioRuntimeProvider::default(),
    );
    let o = b.options_mut();
    o.cache_size = 0;
    o.attempts = 1;
    o.timeout = Duration::from_secs(2);
    o.use_hosts_file = ResolveHosts::Never;
    b.build().expect("resolver")
}

async fn check(port: u16, tcp: bool) {
    let r = resolver(port, tcp);
    for name in [
        "shop.test.",
        "a.b.c.deep.shop.test.",
        "test.",
        "x.local.dev.",
    ] {
        let a = r
            .lookup(name, RecordType::A)
            .await
            .unwrap_or_else(|e| panic!("{name} A ({tcp}): {e}"));
        let ips: Vec<_> = a.answers().iter().map(|r| r.data.clone()).collect();
        assert_eq!(ips.len(), 1, "{name}");
        assert_eq!(
            format!("{:?}", ips[0]),
            format!(
                "{:?}",
                hickory_resolver::proto::rr::RData::A(Ipv4Addr::LOCALHOST.into())
            )
        );
        let ip = r.lookup_ip(name).await.unwrap();
        let all: Vec<IpAddr> = ip.iter().collect();
        assert!(
            all.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)),
            "{name}: {all:?}"
        );
        assert!(
            all.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST)),
            "{name}: {all:?}"
        );
    }
    assert!(
        r.lookup("example.com.", RecordType::A).await.is_err(),
        "outside TLD must not resolve"
    );
    assert!(r.lookup("contest.", RecordType::A).await.is_err());
}

#[tokio::test]
async fn responder_udp_and_tcp() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = NetService::with_dirs(
        tmp.path().join("ca"),
        tmp.path().join("certs"),
        tmp.path().join("backups"),
    );
    let h = svc
        .start_dns(0, &["test".into(), "local.dev".into()])
        .await
        .unwrap();
    let port = h.local_addr().port();
    assert!(port > 1024);
    assert!(h.is_running());
    check(port, false).await;
    check(port, true).await;

    // Live TLD change.
    h.set_tlds(["dev2"]).unwrap();
    let r = resolver(port, false);
    assert!(r.lookup("shop.test.", RecordType::A).await.is_err());
    assert!(r.lookup("shop.dev2.", RecordType::A).await.is_ok());

    // UDP port owner is us.
    let owner = svc.udp_port_owner(port);
    assert!(owner.in_use);
    if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        assert!(owner.ours, "{owner:?}");
    }

    h.stop().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(bz_net::ports::is_udp_free(port));
}
