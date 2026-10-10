//! Port inspection: who is listening on a port, whether it is free, finding a free one.
//!
//! * Linux: `/proc/net/{tcp,tcp6,udp,udp6}` → socket inode → `/proc/<pid>/fd` → `/proc/<pid>/comm`
//!   (processes of other users are only visible when we may read their `fd` dir; the port is
//!   still reported as taken, with `pid: None`).
//! * macOS: `lsof -nP -iTCP:<port> -sTCP:LISTEN -Fpc`.
//! * Windows: `GetExtendedTcpTable` / `GetExtendedUdpTable` (IPv4 + IPv6, owner pid) and
//!   `QueryFullProcessImageNameW` — no text parsing, so it works on every display language
//!   (P1-12). Fallback: `netstat -ano` read by its numeric columns + `tasklist`.
//!
//! Parsers are pure functions, tested on every OS.

use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, UdpSocket};
use std::ops::RangeInclusive;

/// Transport protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Proto {
    /// TCP (listening sockets).
    Tcp,
    /// UDP (bound sockets).
    Udp,
}

/// Who holds a port. Serialises as the API `PortOwner` type (plus `in_use`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortOwner {
    /// Port number.
    pub port: u16,
    /// Owning process id, when known.
    pub pid: Option<u32>,
    /// Owning process name, when known.
    pub process: Option<String>,
    /// True when the owner is the current process.
    pub ours: bool,
    /// True when something is bound to the port (even if pid/process could not be read).
    #[serde(default)]
    pub in_use: bool,
}

impl PortOwner {
    fn free(port: u16) -> Self {
        PortOwner {
            port,
            pid: None,
            process: None,
            ours: false,
            in_use: false,
        }
    }
    fn found(port: u16, pid: Option<u32>, process: Option<String>) -> Self {
        PortOwner {
            port,
            ours: pid == Some(std::process::id()),
            pid,
            process,
            in_use: true,
        }
    }
}

/// Is TCP `port` free on both 127.0.0.1 and 0.0.0.0?
pub fn is_free(port: u16) -> bool {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok()
        && TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).is_ok()
}

/// Is UDP `port` free on both 127.0.0.1 and 0.0.0.0?
pub fn is_udp_free(port: u16) -> bool {
    UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok()
        && UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).is_ok()
}

/// First port in `range` that is free for TCP.
pub fn find_free_port(range: RangeInclusive<u16>) -> Option<u16> {
    range.into_iter().find(|p| *p != 0 && is_free(*p))
}

/// Who listens on TCP `port`.
pub fn port_owner(port: u16) -> PortOwner {
    owner(port, Proto::Tcp)
}

/// Who is bound to UDP `port`.
pub fn udp_port_owner(port: u16) -> PortOwner {
    owner(port, Proto::Udp)
}

/// Who holds `port` for `proto`.
pub fn owner(port: u16, proto: Proto) -> PortOwner {
    // All back-ends compile on every OS (they only use std + external commands), so CI on one
    // OS type-checks and lints all of them; dispatch happens at runtime.
    if cfg!(target_os = "linux") {
        linux_owner(port, proto)
    } else if cfg!(target_os = "macos") {
        macos_owner(port, proto)
    } else if cfg!(windows) {
        windows_owner(port, proto)
    } else {
        fallback_owner(port, proto)
    }
}

fn fallback_owner(port: u16, proto: Proto) -> PortOwner {
    let free = match proto {
        Proto::Tcp => is_free(port),
        Proto::Udp => is_udp_free(port),
    };
    if free {
        PortOwner::free(port)
    } else {
        PortOwner::found(port, None, None)
    }
}

// ---------------------------------------------------------------- Linux

/// Parse a `/proc/net/{tcp,tcp6,udp,udp6}` table: socket inodes bound to `port`
/// (TCP: LISTEN state `0A`; UDP: unconnected state `07`).
pub fn parse_proc_net(table: &str, port: u16, proto: Proto) -> Vec<u64> {
    let want_state = match proto {
        Proto::Tcp => "0A",
        Proto::Udp => "07",
    };
    let mut out = Vec::new();
    for line in table.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || f[3] != want_state {
            continue;
        }
        let Some((_, p)) = f[1].rsplit_once(':') else {
            continue;
        };
        if u16::from_str_radix(p, 16).ok() != Some(port) {
            continue;
        }
        if let Ok(inode) = f[9].parse::<u64>() {
            if inode != 0 {
                out.push(inode);
            }
        }
    }
    out
}

fn linux_owner(port: u16, proto: Proto) -> PortOwner {
    let files: &[&str] = match proto {
        Proto::Tcp => &["/proc/net/tcp", "/proc/net/tcp6"],
        Proto::Udp => &["/proc/net/udp", "/proc/net/udp6"],
    };
    let mut inodes = Vec::new();
    for f in files {
        if let Ok(t) = std::fs::read_to_string(f) {
            inodes.extend(parse_proc_net(&t, port, proto));
        }
    }
    if inodes.is_empty() {
        return PortOwner::free(port);
    }
    let wanted: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
    // Check ourselves first (cheap and the common "ours" case), then everyone else.
    let me = std::process::id();
    let mut pids: Vec<u32> = vec![me];
    if let Ok(rd) = std::fs::read_dir("/proc") {
        pids.extend(
            rd.flatten()
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
                .filter(|p| *p != me),
        );
    }
    for pid in pids {
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(link) = std::fs::read_link(fd.path()) {
                if wanted.iter().any(|w| link.as_os_str() == w.as_str()) {
                    let name = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                        .ok()
                        .map(|s| s.trim().to_string());
                    return PortOwner::found(port, Some(pid), name);
                }
            }
        }
    }
    PortOwner::found(port, None, None)
}

// ---------------------------------------------------------------- macOS

/// Parse `lsof -F pc` output → first (pid, command).
pub fn parse_lsof(out: &str) -> Option<(u32, Option<String>)> {
    let mut pid = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            if pid.is_some() {
                break;
            }
            pid = p.trim().parse::<u32>().ok();
        } else if let Some(c) = line.strip_prefix('c') {
            if let Some(p) = pid {
                return Some((p, Some(c.trim().to_string())));
            }
        }
    }
    pid.map(|p| (p, None))
}

fn macos_owner(port: u16, proto: Proto) -> PortOwner {
    let arg = match proto {
        Proto::Tcp => format!("-iTCP:{port}"),
        Proto::Udp => format!("-iUDP:{port}"),
    };
    let mut args = vec!["-nP", arg.as_str()];
    if proto == Proto::Tcp {
        args.push("-sTCP:LISTEN");
    }
    args.push("-Fpc");
    match crate::elevation::try_output("lsof", &args).and_then(|(_, o)| parse_lsof(&o)) {
        Some((pid, name)) => PortOwner::found(port, Some(pid), name),
        None => fallback_owner(port, proto),
    }
}

// ---------------------------------------------------------------- Windows

/// `MIB_TCP_STATE_LISTEN`.
const TCP_STATE_LISTEN: u32 = 2;

/// Read the native-endian `u32` at byte `off`.
fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// `dwLocalPort` holds the port in network byte order in its low 16 bits.
fn row_port(raw: u32) -> u16 {
    u16::from_be((raw & 0xffff) as u16)
}

/// Find the owner pid of `port` in a raw `MIB_{TCP,TCP6,UDP,UDP6}TABLE_OWNER_PID` buffer
/// as returned by `GetExtendedTcpTable` / `GetExtendedUdpTable` (P1-12).
///
/// Layout: `dwNumEntries: u32`, then rows. Row sizes / offsets (bytes):
/// TCP4 24 (port 8, state 0, pid 20), TCP6 56 (port 20, state 48, pid 52),
/// UDP4 12 (port 4, pid 8), UDP6 28 (port 20, pid 24). TCP rows must be LISTEN.
pub fn parse_owner_pid_table(buf: &[u8], port: u16, proto: Proto, v6: bool) -> Option<u32> {
    let (row, port_off, state_off, pid_off) = match (proto, v6) {
        (Proto::Tcp, false) => (24, 8, Some(0), 20),
        (Proto::Tcp, true) => (56, 20, Some(48), 52),
        (Proto::Udp, false) => (12, 4, None, 8),
        (Proto::Udp, true) => (28, 20, None, 24),
    };
    let n = u32_at(buf, 0)? as usize;
    (0..n).find_map(|i| {
        let r = buf.get(4 + i * row..4 + (i + 1) * row)?;
        if row_port(u32_at(r, port_off)?) != port {
            return None;
        }
        if let Some(so) = state_off {
            if u32_at(r, so)? != TCP_STATE_LISTEN {
                return None;
            }
        }
        u32_at(r, pid_off)
    })
}

/// Is a netstat address column a wildcard remote address (`0.0.0.0:0`, `[::]:0`, `*:*`)?
fn netstat_wildcard(addr: &str) -> bool {
    matches!(addr, "0.0.0.0:0" | "[::]:0" | "*:*")
}

/// Parse `netstat -ano` output → pid holding `port` (TCP: listening rows; UDP: any bound
/// row). The state column is localized ("LISTENING", "ABHÖREN", "DINLEME", …), so a TCP
/// row counts as listening when its foreign address is the wildcard `0.0.0.0:0` /
/// `[::]:0` — only listening sockets have that (P1-12). Protocol names, addresses and
/// the PID column are not localized.
pub fn parse_netstat(out: &str, port: u16, proto: Proto) -> Option<u32> {
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let ok_proto = match proto {
            Proto::Tcp => {
                f[0].eq_ignore_ascii_case("TCP") && f.len() >= 5 && netstat_wildcard(f[2])
            }
            Proto::Udp => f[0].eq_ignore_ascii_case("UDP"),
        };
        if !ok_proto {
            continue;
        }
        let Some((_, p)) = f[1].rsplit_once(':') else {
            continue;
        };
        if p.parse::<u16>().ok() == Some(port) {
            return f.last().and_then(|s| s.parse().ok());
        }
    }
    None
}

/// Parse `tasklist /FO CSV /NH` output → image name.
pub fn parse_tasklist(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.starts_with('"'))?;
    let name = line.trim_start_matches('"').split('"').next()?;
    Some(name.to_string())
}

fn windows_owner(port: u16, proto: Proto) -> PortOwner {
    #[cfg(windows)]
    if let Some(found) = win::owner(port, proto) {
        return found;
    }
    let Some((_, out)) = crate::elevation::try_output("netstat", &["-ano"]) else {
        return fallback_owner(port, proto);
    };
    match parse_netstat(&out, port, proto) {
        Some(pid) => {
            let filter = format!("PID eq {pid}");
            let name =
                crate::elevation::try_output("tasklist", &["/FI", &filter, "/FO", "CSV", "/NH"])
                    .and_then(|(_, o)| parse_tasklist(&o));
            PortOwner::found(port, Some(pid), name)
        }
        None => fallback_owner(port, proto),
    }
}

#[cfg(windows)]
mod win {
    use super::{parse_owner_pid_table, PortOwner, Proto};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, TCP_TABLE_OWNER_PID_LISTENER, UDP_TABLE_OWNER_PID,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// Raw owner-pid table for one protocol / address family.
    fn table(proto: Proto, af: u16) -> Option<Vec<u8>> {
        let mut size: u32 = 0;
        for _ in 0..4 {
            // u32-aligned backing store
            let mut buf = vec![0u32; (size as usize).div_ceil(4).max(1)];
            let mut len = (buf.len() * 4) as u32;
            // SAFETY: `buf` is valid for `len` bytes; the API writes at most `len`.
            let rc = unsafe {
                match proto {
                    Proto::Tcp => GetExtendedTcpTable(
                        buf.as_mut_ptr().cast(),
                        &mut len,
                        0,
                        af as u32,
                        TCP_TABLE_OWNER_PID_LISTENER,
                        0,
                    ),
                    Proto::Udp => GetExtendedUdpTable(
                        buf.as_mut_ptr().cast(),
                        &mut len,
                        0,
                        af as u32,
                        UDP_TABLE_OWNER_PID,
                        0,
                    ),
                }
            };
            if rc == NO_ERROR {
                let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_ne_bytes()).collect();
                return Some(bytes[..(len as usize).min(bytes.len())].to_vec());
            }
            if rc != ERROR_INSUFFICIENT_BUFFER {
                return None;
            }
            // the table may grow between calls: add headroom
            size = len + 1024;
        }
        None
    }

    fn process_name(pid: u32) -> Option<String> {
        match pid {
            0 => return Some("System Idle Process".into()),
            4 => return Some("System".into()),
            _ => {}
        }
        // SAFETY: FFI; null means failure. The handle is closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: valid handle and buffer of `len` u16s.
        let ok = unsafe {
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
        } != 0;
        // SAFETY: handle from OpenProcess.
        unsafe {
            CloseHandle(h);
        }
        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Some(
            full.rsplit(['\\', '/'])
                .next()
                .unwrap_or(full.as_str())
                .to_string(),
        )
    }

    /// `None` when the tables could not be read (caller falls back to netstat).
    pub(super) fn owner(port: u16, proto: Proto) -> Option<PortOwner> {
        let v4 = table(proto, AF_INET)?;
        let v6 = table(proto, AF_INET6).unwrap_or_default();
        let pid = parse_owner_pid_table(&v4, port, proto, false)
            .or_else(|| parse_owner_pid_table(&v6, port, proto, true));
        Some(match pid {
            Some(pid) => PortOwner::found(port, Some(pid), process_name(pid)),
            None => {
                // nothing in the tables; a bind check still catches what we cannot see
                super::fallback_owner(port, proto)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_net_parsing() {
        let t = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0\n\
   1: 0100007F:1F90 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 9999 1 0000000000000000 100 0 0 10 0\n\
   2: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 77 1 0000000000000000 100 0 0 10 0\n";
        assert_eq!(parse_proc_net(t, 8080, Proto::Tcp), vec![4242]);
        assert_eq!(parse_proc_net(t, 80, Proto::Tcp), vec![77]);
        assert!(parse_proc_net(t, 81, Proto::Tcp).is_empty());
        let t6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000000000000000000001000000:150E 00000000000000000000000000000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 555 2 0000000000000000 0\n";
        assert_eq!(parse_proc_net(t6, 5390, Proto::Udp), vec![555]);
    }

    #[test]
    fn lsof_parsing() {
        assert_eq!(
            parse_lsof("p123\ncnginx\nf6\n"),
            Some((123, Some("nginx".into())))
        );
        assert_eq!(parse_lsof(""), None);
    }

    #[test]
    fn owner_pid_table_parsing() {
        fn port_raw(p: u16) -> u32 {
            u32::from(p.to_be())
        }
        fn put(b: &mut Vec<u8>, v: u32) {
            b.extend_from_slice(&v.to_ne_bytes());
        }
        // TCP4: 2 rows (one ESTABLISHED on 8080, one LISTEN on 8080)
        let mut t = vec![];
        put(&mut t, 2);
        for (state, pid) in [(5u32, 55u32), (2, 4321)] {
            put(&mut t, state);
            put(&mut t, 0x0100_007f);
            put(&mut t, port_raw(8080));
            put(&mut t, 0);
            put(&mut t, 0);
            put(&mut t, pid);
        }
        assert_eq!(
            parse_owner_pid_table(&t, 8080, Proto::Tcp, false),
            Some(4321)
        );
        assert_eq!(parse_owner_pid_table(&t, 80, Proto::Tcp, false), None);
        // TCP6: one LISTEN row on 443
        let mut t6 = vec![];
        put(&mut t6, 1);
        t6.extend_from_slice(&[0u8; 16]);
        put(&mut t6, 0);
        put(&mut t6, port_raw(443));
        t6.extend_from_slice(&[0u8; 16]);
        put(&mut t6, 0);
        put(&mut t6, 0);
        put(&mut t6, 2);
        put(&mut t6, 777);
        assert_eq!(parse_owner_pid_table(&t6, 443, Proto::Tcp, true), Some(777));
        // UDP4 / UDP6
        let mut u = vec![];
        put(&mut u, 1);
        put(&mut u, 0x0100_007f);
        put(&mut u, port_raw(53));
        put(&mut u, 888);
        assert_eq!(parse_owner_pid_table(&u, 53, Proto::Udp, false), Some(888));
        let mut u6 = vec![];
        put(&mut u6, 1);
        u6.extend_from_slice(&[0u8; 16]);
        put(&mut u6, 0);
        put(&mut u6, port_raw(53));
        put(&mut u6, 999);
        assert_eq!(parse_owner_pid_table(&u6, 53, Proto::Udp, true), Some(999));
        // truncated buffers never panic
        assert_eq!(
            parse_owner_pid_table(&t[..30], 8080, Proto::Tcp, false),
            None
        );
        assert_eq!(parse_owner_pid_table(&[], 53, Proto::Udp, false), None);
    }

    #[test]
    fn netstat_parsing_is_locale_independent() {
        // Turkish / German state names: the foreign-address column decides
        let out = "  TCP    0.0.0.0:80             0.0.0.0:0              DINLEME         4\n\
  TCP    [::]:443               [::]:0                 ABHÖREN         2020\n\
  TCP    127.0.0.1:80           127.0.0.1:50000        KURULDU         66\n";
        assert_eq!(parse_netstat(out, 80, Proto::Tcp), Some(4));
        assert_eq!(parse_netstat(out, 443, Proto::Tcp), Some(2020));
    }

    #[test]
    fn netstat_parsing() {
        let out = "\nActive Connections\n\n  Proto  Local Address          Foreign Address        State           PID\n\
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1012\n\
  TCP    127.0.0.1:8080         127.0.0.1:50000        ESTABLISHED     55\n\
  TCP    [::]:8080              [::]:0                 LISTENING       4321\n\
  UDP    127.0.0.1:53           *:*                                    888\n";
        assert_eq!(parse_netstat(out, 8080, Proto::Tcp), Some(4321));
        assert_eq!(parse_netstat(out, 135, Proto::Tcp), Some(1012));
        assert_eq!(parse_netstat(out, 53, Proto::Udp), Some(888));
        assert_eq!(parse_netstat(out, 53, Proto::Tcp), None);
        assert_eq!(
            parse_tasklist("\"nginx.exe\",\"4321\",\"Console\",\"1\",\"10,000 K\"\r\n"),
            Some("nginx.exe".into())
        );
        assert_eq!(parse_tasklist("INFO: No tasks are running"), None);
    }

    #[test]
    fn live_owner_and_free() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(!is_free(port));
        let o = port_owner(port);
        assert!(o.in_use);
        if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
            assert_eq!(o.pid, Some(std::process::id()), "{o:?}");
            assert!(o.ours);
            assert!(o.process.is_some());
        }
        drop(l);
        // The release is not always visible at once (seen on macOS runners); poll briefly.
        let freed = (0..40).any(|_| {
            is_free(port) || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                false
            }
        });
        assert!(freed, "port {port} still busy after drop");
        assert!(!port_owner(port).in_use);
        let u = UdpSocket::bind("127.0.0.1:0").unwrap();
        let up = u.local_addr().unwrap().port();
        assert!(!is_udp_free(up));
        assert!(udp_port_owner(up).in_use);
        let f = find_free_port(40000..=40100).unwrap();
        assert!(is_free(f));
    }
}
