//! 同一個網路直接互傳（Android 版）：和電腦版用同一套協定（lan_proto.rs），
//! 手機和同一個 Wi-Fi 裡、登入同一個帳號的電腦直接互傳，不繞到網際網路。
//! 使用者在 App 裡開啟才會執行；手機要拿著 MulticastLock 才收得到廣播信標。

use crate::{android, cloud::file_proto, cloud::lan_proto::*, cloud::Cloud, files};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream, UdpSocket},
};

/// 這支手機在信標裡宣告支援的功能：可以收檔案。
const CAPS: u32 = CAP_FILES;

fn enabled(cloud: &Cloud) -> bool {
    cloud.signed_in() && cloud.prefs().lan
}

pub async fn run(app: AppHandle) {
    let cloud = app.state::<Cloud>();
    loop {
        if enabled(&cloud) {
            android::set_multicast(true);
            if serve(&app, &cloud).await.is_err() {
                // 例如還沒有區域網路權限、沒有連 Wi-Fi，或金鑰還沒下載：稍後再試
                if cloud.lan.clear() {
                    cloud.changed();
                }
                tokio::select! {
                    _ = tokio::time::sleep(RETRY) => {}
                    _ = cloud.lan_wake.notified() => {}
                }
            }
            continue;
        }
        android::set_multicast(false);
        if cloud.lan.clear() {
            cloud.changed();
        }
        cloud.lan_wake.notified().await;
    }
}

async fn serve(app: &AppHandle, cloud: &Cloud) -> io::Result<()> {
    if cloud.key().await.is_err() {
        return Err(io::Error::other("no key yet"));
    }
    let tcp = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    let port = tcp.local_addr()?.port();
    let udp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT)).await?;
    udp.set_broadcast(true)?;

    let me = Me { id: cloud.device_id.clone(), name: cloud.device_name.clone(), port };
    broadcast(&udp, cloud, &me, true).await;
    let mut tick = tokio::time::interval(BEACON_EVERY);
    tick.tick().await;
    let mut buf = vec![0u8; 2048];
    loop {
        tokio::select! {
            _ = tick.tick() => {
                broadcast(&udp, cloud, &me, false).await;
                if cloud.lan.prune() {
                    cloud.changed();
                }
            }
            got = udp.recv_from(&mut buf) => {
                let Ok((n, from)) = got else { continue };
                let Some(key) = cloud.key.lock().clone() else { continue };
                if let Some(beacon) = open_beacon(&key, &buf[..n]) {
                    if beacon.d == me.id {
                        continue;
                    }
                    if cloud.lan.saw_beacon(beacon.d, beacon.n, beacon.c, SocketAddr::new(from.ip(), beacon.p)) {
                        cloud.changed();
                        let _ = tauri::Emitter::emit(app, "files", ());
                    }
                    if beacon.r {
                        let _ = udp.send_to(&seal_beacon_with(&key, &me, false, CAPS), from).await;
                    }
                }
            }
            conn = tcp.accept() => {
                let Ok((stream, _)) = conn else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                };
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = accept(&app, stream).await;
                });
            }
            _ = cloud.lan_wake.notified() => {
                if !enabled(cloud) {
                    return Ok(());
                }
                // 回到前景：馬上請大家回報，換了 Wi-Fi 也能很快找到電腦
                broadcast(&udp, cloud, &me, true).await;
            }
        }
    }
}

async fn broadcast(udp: &UdpSocket, cloud: &Cloud, me: &Me, reply: bool) {
    let Some(key) = cloud.key.lock().clone() else { return };
    let packet = seal_beacon_with(&key, me, reply, CAPS);
    for target in broadcast_targets() {
        let _ = udp.send_to(&packet, SocketAddrV4::new(target, PORT)).await;
    }
}

/// 收一個連線：`CPF1` 開頭是檔案，其他是一則即時同步的內容。
async fn accept(app: &AppHandle, mut stream: TcpStream) -> io::Result<()> {
    let mut head = [0u8; 4];
    tokio::time::timeout(IO_TIMEOUT, stream.read_exact(&mut head)).await.map_err(|_| io::ErrorKind::TimedOut)??;
    if &head == file_proto::MAGIC {
        return files::receive_lan(app, stream).await;
    }
    tokio::time::timeout(IO_TIMEOUT, async {
        let msg = read_frame_body(&mut stream, head).await?;
        let cloud = app.state::<Cloud>();
        let ok = cloud.key.lock().clone().is_some_and(|key| cloud.receive(&key, &msg, Some(MAX_AGE)));
        write_ack(&mut stream, ok).await
    })
    .await
    .map_err(|_| io::ErrorKind::TimedOut)?
}
