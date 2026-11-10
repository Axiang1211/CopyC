//! 區域網路直傳的協定（電腦版與 Android 版共用；Android 版以 #[path] 引用）。
//!
//! - **探索**：每 20 秒往 UDP 47321 廣播一個信標，內容（裝置 ID、名稱、TCP 埠、時間）用帳號金鑰加密。
//!   解得開的才是同一個帳號的裝置；同網路的其他人只看到亂碼。剛啟動時的信標會請對方立刻回覆。
//! - **傳送**：連到對方的 TCP 埠，送出和中繼完全相同的密文；對方解開後回一個位元組確認。
//!   所有內容都經過驗證加密，偽造或竄改的封包解不開，錄下來重播的封包也會因為時間太舊被丟掉。

use super::crypto::Key;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub const PORT: u16 = 47321;
/// 信標開頭的標記與版本。
pub const MAGIC: &[u8; 5] = b"CPCB1";
pub const BEACON_EVERY: Duration = Duration::from_secs(20);
/// 這麼久沒聽到信標就當作離開了網路。
pub const PEER_TTL: Duration = Duration::from_secs(65);
/// 傳送失敗的裝置先改走中繼，這段時間內不再嘗試直連（例如對方的防火牆擋住了連入）。
pub const FAIL_BACKOFF: Duration = Duration::from_secs(120);
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(700);
pub const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// 信標與直傳內容的時間和現在差太多就不採用（防重播）；同一帳號的電腦時鐘通常差不到這麼多。
pub const MAX_AGE: Duration = Duration::from_secs(10 * 60);
/// 單則直傳上限：和中繼相同。
pub const MAX_FRAME: usize = 200_000;
/// 啟動失敗（例如埠被占用）時，隔這麼久再試。
pub const RETRY: Duration = Duration::from_secs(60);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// 信標裡加密的內容。
#[derive(Serialize, Deserialize)]
pub struct Beacon {
    /// 裝置 ID
    pub d: String,
    /// 裝置名稱
    pub n: String,
    /// TCP 埠
    pub p: u16,
    /// 送出時間（毫秒）
    pub t: u64,
    /// 請對方馬上回一個信標（剛啟動時）
    #[serde(default)]
    pub r: bool,
}

struct Peer {
    addr: SocketAddr,
    seen: Instant,
    failed: Option<Instant>,
}

/// 目前在同一個網路裡、同一個帳號的其他電腦。
#[derive(Default)]
pub struct Lan {
    peers: Mutex<HashMap<String, Peer>>,
}

impl Lan {
    /// 現在可以直連的裝置數（設定頁顯示用）。
    pub fn count(&self) -> usize {
        self.peers.lock().values().filter(|p| p.seen.elapsed() < PEER_TTL).count()
    }

    /// 這次複製要直傳的對象。
    pub fn targets(&self) -> Vec<(String, SocketAddr)> {
        self.peers
            .lock()
            .iter()
            .filter(|(_, p)| p.seen.elapsed() < PEER_TTL && !p.failed.is_some_and(|f| f.elapsed() <= FAIL_BACKOFF))
            .map(|(id, p)| (id.clone(), p.addr))
            .collect()
    }

    /// 記下一個信標；回傳是不是新出現的裝置。
    pub fn saw(&self, id: String, addr: SocketAddr) -> bool {
        let mut peers = self.peers.lock();
        match peers.get_mut(&id) {
            Some(p) => {
                let back = p.seen.elapsed() >= PEER_TTL;
                if p.addr != addr {
                    // 換了 IP（重新連上 Wi-Fi）：之前的失敗紀錄不算數
                    p.addr = addr;
                    p.failed = None;
                }
                p.seen = Instant::now();
                back
            }
            None => {
                peers.insert(id, Peer { addr, seen: Instant::now(), failed: None });
                true
            }
        }
    }

    pub fn failed(&self, id: &str) {
        if let Some(p) = self.peers.lock().get_mut(id) {
            p.failed = Some(Instant::now());
        }
    }

    /// 清掉過期的裝置；回傳有沒有變化。
    pub fn prune(&self) -> bool {
        let mut peers = self.peers.lock();
        let before = peers.len();
        peers.retain(|_, p| p.seen.elapsed() < PEER_TTL);
        peers.len() != before
    }

    pub fn clear(&self) -> bool {
        let mut peers = self.peers.lock();
        let had = !peers.is_empty();
        peers.clear();
        had
    }
}

/// 把同一份線路訊息直接送給區域網路裡的裝置，回傳確認收到的裝置 ID；送不到的先改走中繼一陣子。
pub async fn deliver(lan: &Lan, targets: Vec<(String, SocketAddr)>, msg: String) -> Vec<String> {
    let sends = targets.into_iter().map(|(id, addr)| {
        let msg = msg.as_bytes();
        async move {
            let ok = matches!(tokio::time::timeout(CONNECT_TIMEOUT + IO_TIMEOUT, send(addr, msg)).await, Ok(Ok(())));
            (id, ok)
        }
    });
    let mut delivered = Vec::new();
    for (id, ok) in futures_util::future::join_all(sends).await {
        if ok {
            delivered.push(id);
        } else {
            lan.failed(&id);
        }
    }
    delivered
}

async fn send(addr: SocketAddr, msg: &[u8]) -> io::Result<()> {
    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| io::ErrorKind::TimedOut)??;
    stream.set_nodelay(true)?;
    stream.write_all(&(msg.len() as u32).to_be_bytes()).await?;
    stream.write_all(msg).await?;
    let mut ack = [0u8; 1];
    stream.read_exact(&mut ack).await?;
    if ack[0] == 1 {
        Ok(())
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

/// 讀一則直傳：長度（4 位元組）+ 線路訊息。
pub async fn read_frame(stream: &mut TcpStream) -> io::Result<String> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut msg = vec![0u8; len];
    stream.read_exact(&mut msg).await?;
    String::from_utf8(msg).map_err(|_| io::ErrorKind::InvalidData.into())
}

/// 回覆傳送端：1 表示解得開、收下了。
pub async fn write_ack(stream: &mut TcpStream, ok: bool) -> io::Result<()> {
    stream.write_all(&[u8::from(ok)]).await
}

/// 這台裝置的信標內容。
pub struct Me {
    pub id: String,
    pub name: String,
    pub port: u16,
}

pub fn seal_beacon(key: &Key, me: &Me, reply: bool) -> Vec<u8> {
    let beacon = Beacon { d: me.id.clone(), n: me.name.clone(), p: me.port, t: now_ms(), r: reply };
    let mut out = MAGIC.to_vec();
    out.extend(key.seal(&serde_json::to_vec(&beacon).unwrap_or_default()));
    out
}

/// 用帳號金鑰解開信標；不是同一個帳號、格式不對或時間太舊都回傳 None。
pub fn open_beacon(key: &Key, packet: &[u8]) -> Option<Beacon> {
    let sealed = packet.strip_prefix(MAGIC)?;
    let beacon: Beacon = serde_json::from_slice(&key.open(sealed)?).ok()?;
    let fresh = now_ms().abs_diff(beacon.t) <= MAX_AGE.as_millis() as u64;
    (fresh && beacon.p != 0).then_some(beacon)
}

/// 每張網卡的廣播位址（只送 255.255.255.255 的話，有多張網卡時可能走錯網卡）。
pub fn broadcast_targets() -> Vec<Ipv4Addr> {
    let mut out = vec![Ipv4Addr::BROADCAST];
    for iface in if_addrs::get_if_addrs().unwrap_or_default() {
        let if_addrs::IfAddr::V4(v4) = &iface.addr else { continue };
        if v4.ip.is_loopback() || v4.ip.is_link_local() || !iface.is_oper_up() {
            continue;
        }
        let target = v4.broadcast.unwrap_or_else(|| directed_broadcast(v4.ip, v4.netmask));
        if !out.contains(&target) {
            out.push(target);
        }
    }
    out
}

pub fn directed_broadcast(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(ip) | !u32::from(netmask))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn directed_broadcast_address() {
        let b = directed_broadcast(Ipv4Addr::new(192, 168, 1, 23), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(b, Ipv4Addr::new(192, 168, 1, 255));
        let b = directed_broadcast(Ipv4Addr::new(10, 1, 2, 3), Ipv4Addr::new(255, 255, 0, 0));
        assert_eq!(b, Ipv4Addr::new(10, 1, 255, 255));
    }

    #[test]
    fn beacon_only_opens_with_the_same_key() {
        let key = Key::generate();
        let me = Me { id: "0123456789abcdef".into(), name: "MY-PC".into(), port: 50123 };
        let packet = seal_beacon(&key, &me, true);
        assert!(packet.starts_with(MAGIC));
        // 名稱與 ID 不會以明文出現在網路上
        let raw = String::from_utf8_lossy(&packet);
        assert!(!raw.contains("MY-PC") && !raw.contains("0123456789abcdef"));
        let beacon = open_beacon(&key, &packet).unwrap();
        assert_eq!((beacon.d.as_str(), beacon.p, beacon.r), ("0123456789abcdef", 50123, true));
        assert!(open_beacon(&Key::generate(), &packet).is_none());
    }

    #[test]
    fn peers_expire_and_back_off_after_failures() {
        let lan = Lan::default();
        let addr: SocketAddr = "192.168.1.5:50000".parse().unwrap();
        assert!(lan.saw("a".into(), addr));
        assert!(!lan.saw("a".into(), addr));
        assert_eq!(lan.targets().len(), 1);
        lan.failed("a");
        assert!(lan.targets().is_empty());
        assert_eq!(lan.count(), 1);
        // 換了 IP 就重新嘗試
        lan.saw("a".into(), "192.168.1.6:50000".parse().unwrap());
        assert_eq!(lan.targets(), vec![("a".to_string(), "192.168.1.6:50000".parse().unwrap())]);
        lan.peers.lock().get_mut("a").unwrap().seen = Instant::now() - PEER_TTL;
        assert!(lan.prune());
        assert_eq!(lan.count(), 0);
    }

    #[tokio::test]
    async fn direct_send_waits_for_acknowledgement() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for ack in [1u8, 0u8] {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut len = [0u8; 4];
                s.read_exact(&mut len).await.unwrap();
                let mut msg = vec![0u8; u32::from_be_bytes(len) as usize];
                s.read_exact(&mut msg).await.unwrap();
                assert_eq!(msg, b"hello");
                s.write_all(&[ack]).await.unwrap();
            }
        });
        assert!(send(addr, b"hello").await.is_ok());
        assert!(send(addr, b"hello").await.is_err());
        server.await.unwrap();
        // 沒有人在聽：很快失敗，不會卡住
        let closed = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap().local_addr().unwrap();
        assert!(send(closed, b"x").await.is_err());
    }
}
