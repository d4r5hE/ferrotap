//! Which local process owns a client connection: the Windows TCP tables (GetExtendedTcpTable)
//! and the process image name (QueryFullProcessImageNameW), no child processes. Results are
//! cached per client address for a short while (a keep-alive connection keeps its owner).
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetExtendedTcpTable(table: *mut c_void, size: *mut u32, order: i32, af: u32, class: u32, reserved: u32) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(h: *mut c_void) -> i32;
        fn QueryFullProcessImageNameW(h: *mut c_void, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    }

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    const TCP_TABLE_OWNER_PID_ALL: u32 = 5;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    /// Raw table bytes for one address family.
    fn table(af: u32) -> Option<Vec<u8>> {
        let mut size: u32 = 0;
        unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, af, TCP_TABLE_OWNER_PID_ALL, 0) };
        for _ in 0..4 {
            size = size.max(4096) + 4096;
            let mut buf = vec![0u8; size as usize];
            let rc = unsafe { GetExtendedTcpTable(buf.as_mut_ptr() as *mut c_void, &mut size, 0, af, TCP_TABLE_OWNER_PID_ALL, 0) };
            if rc == 0 {
                buf.truncate(size as usize);
                return Some(buf);
            }
            if rc != 122 {
                return None; // not ERROR_INSUFFICIENT_BUFFER
            }
        }
        None
    }

    fn u32_at(b: &[u8], at: usize) -> Option<u32> {
        b.get(at..at + 4).map(|x| u32::from_ne_bytes([x[0], x[1], x[2], x[3]]))
    }

    /// Port stored in network byte order in the low 16 bits of a DWORD.
    fn port(dw: u32) -> u16 {
        u16::from_be(dw as u16)
    }

    pub fn pid_for(addr: &std::net::SocketAddr) -> Option<u32> {
        match addr {
            std::net::SocketAddr::V4(a) => pid_v4(a.ip().octets(), a.port()),
            std::net::SocketAddr::V6(a) => match a.ip().to_ipv4_mapped() {
                Some(v4) => pid_v4(v4.octets(), a.port()),
                None => pid_v6(a.ip().octets(), a.port()),
            },
        }
    }

    fn pid_v4(ip: [u8; 4], p: u16) -> Option<u32> {
        let b = table(AF_INET)?;
        let n = u32_at(&b, 0)? as usize;
        // MIB_TCPROW_OWNER_PID: state, localAddr, localPort, remoteAddr, remotePort, pid (6 DWORDs)
        for i in 0..n {
            let at = 4 + i * 24;
            let la = b.get(at + 4..at + 8)?;
            if la == ip && port(u32_at(&b, at + 8)?) == p {
                return u32_at(&b, at + 20).filter(|&pid| pid != 0);
            }
        }
        None
    }

    fn pid_v6(ip: [u8; 16], p: u16) -> Option<u32> {
        let b = table(AF_INET6)?;
        let n = u32_at(&b, 0)? as usize;
        // MIB_TCP6ROW_OWNER_PID: localAddr[16], scope, localPort, remoteAddr[16], scope, remotePort, state, pid
        for i in 0..n {
            let at = 4 + i * 56;
            if b.get(at..at + 16)? == ip && port(u32_at(&b, at + 20)?) == p {
                return u32_at(&b, at + 52).filter(|&pid| pid != 0);
            }
        }
        None
    }

    pub fn image_name(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
            CloseHandle(h);
            if ok == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit(['\\', '/']).next().map(str::to_string)
        }
    }
}

const TTL: Duration = Duration::from_secs(20);

fn cache() -> &'static Mutex<HashMap<SocketAddr, (Instant, Option<(u32, String)>)>> {
    static C: OnceLock<Mutex<HashMap<SocketAddr, (Instant, Option<(u32, String)>)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// (pid, image name) of the local process connected from `addr`; None for remote clients.
pub fn lookup(addr: &SocketAddr) -> Option<(u32, String)> {
    if !addr.ip().is_loopback() && !addr.ip().to_canonical().is_loopback() {
        return None;
    }
    if let Ok(c) = cache().lock() {
        if let Some((t, v)) = c.get(addr) {
            if t.elapsed() < TTL {
                return v.clone();
            }
        }
    }
    let v = resolve(addr);
    if let Ok(mut c) = cache().lock() {
        if c.len() > 4096 {
            c.retain(|_, (t, _)| t.elapsed() < TTL);
        }
        c.insert(*addr, (Instant::now(), v.clone()));
    }
    v
}

#[cfg(windows)]
fn resolve(addr: &SocketAddr) -> Option<(u32, String)> {
    let pid = win::pid_for(addr)?;
    let name = win::image_name(pid).or_else(|| crate::rules_ext::process_name(pid))?;
    Some((pid, name))
}

#[cfg(not(windows))]
fn resolve(addr: &SocketAddr) -> Option<(u32, String)> {
    let pid = crate::rules_ext::pid_for_local_addr(addr)?;
    Some((pid, crate::rules_ext::process_name(pid)?))
}
