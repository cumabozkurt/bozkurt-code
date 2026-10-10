//! NetService façade smoke tests that do not touch the system.

use bz_net::{NetService, Os, ResolverBackend};

#[test]
fn plans_are_generated_without_side_effects() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = NetService::with_dirs(
        tmp.path().join("ca"),
        tmp.path().join("certs"),
        tmp.path().join("backups"),
    );

    let st = svc.ca_status();
    assert!(!st.exists && !st.ca_trusted && st.ca_expires.is_none());

    let plan = svc.trust_ca(false).unwrap(); // creates the CA, only plans trust
    assert_eq!(plan.os, Os::current());
    assert!(svc.paths().ca_dir.join("rootCA.pem").is_file());
    let st = svc.ca_status();
    assert!(st.exists && st.ca_expires.is_some());
    assert!(!st.stores.is_empty());
    println!("{plan}");

    let setup = svc.dns_configure_plan("test", 5390).unwrap();
    assert!(!setup.plan.title.is_empty());
    if setup.backend != ResolverBackend::HostsFile {
        assert!(setup.wildcard);
        assert!(setup.plan.requires_elevation());
    }
    println!("{:?} {}", setup.backend, setup.plan);
    assert!(svc.dns_status("test", 5390).is_ok());
    assert!(svc.dns_configure_plan("bad tld!", 5390).is_err());

    let p = svc.hosts_add_plan(&["shop.test".into()]);
    assert!(p.requires_elevation() || p.is_empty());

    let owner = svc.port_owner(1);
    assert_eq!(owner.port, 1);
    assert!(svc.find_free_port(41000..=41100).is_some());

    let json = serde_json::to_value(svc.port_owner(1)).unwrap();
    assert!(json.get("pid").is_some() && json.get("ours").is_some());
}
