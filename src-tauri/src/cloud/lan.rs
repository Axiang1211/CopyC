//! 區域網路直傳：同一個 Google 帳號（同一把金鑰）的電腦在同一個網路裡時，複製的文字直接用 TCP 傳過去，
//! 不繞到網際網路；沒有直接收到的裝置再交給 Cloudflare 中繼（見 [`super::relay`] 的 `skip`）。
//!
//! - **探索**：每 20 秒往 UDP 47321 廣播一個信標，內容（裝置 ID、名稱、TCP 埠、時間）用帳號金鑰加密。
//!   解得開的才是同一個帳號的裝置；同網路的其他人只看到亂碼。剛啟動時的信標會請對方立刻回覆。
//! - **傳送**：連到對方的 TCP 埠，送出和中繼完全相同的密文；對方解開後回一個位元組確認。
//!   所有內容都經過驗證加密，偽造或竄改的封包解不開，錄下來重播的封包也會因為時間太舊被丟掉。
//!
//! 協定本身（信標、裝置清單、傳送格式）在 [`super::lan_proto`]，和 Android 版共用。

use super::{lan_proto::*, relay, Cloud};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

pub use super::lan_proto::{Lan, MAX_AGE};

/// 把同一份線路訊息直接送給區域網路裡的裝置，回傳確認收到的裝置 ID。
pub async fn deliver(cloud: &Cloud, targets: Vec<(String, SocketAddr)>, msg: String) -> Vec<String> {
    super::lan_proto::deliver(&cloud.lan, targets, msg).await
}

/// 背景工作：登入、開啟即時同步與區域網路直傳時才執行。
pub async fn run(app: AppHandle) {
    let cloud = app.state::<Cloud>();
    let mut config = cloud.config.subscribe();
    loop {
        let c = *config.borrow_and_update();
        if c.signed_in && c.live && c.lan {
            if let Err(e) = serve(&app, &cloud, &mut config).await {
                eprintln!("copyc: 區域網路直傳暫停：{e}");
                if cloud.lan.clear() {
                    cloud.changed();
                }
                tokio::select! {
                    _ = tokio::time::sleep(RETRY) => {}
                    r = config.changed() => if r.is_err() { return },
                }
            }
            continue;
        }
        if cloud.lan.clear() {
            cloud.changed();
        }
        if config.changed().await.is_err() {
            return;
        }
    }
}

/// 開始監聽與廣播，直到設定關閉（回傳 Ok）或網路出錯。
async fn serve(
    app: &AppHandle,
    cloud: &Cloud,
    config: &mut tokio::sync::watch::Receiver<super::Config>,
) -> io::Result<()> {
    // 金鑰在 Google Drive；第一次要先下載
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
                // Windows 在對方埠已關閉時會讓下一次 recv_from 回報 ConnectionReset，略過即可
                let Ok((n, from)) = got else { continue };
                let Some(key) = cloud.key.lock().clone() else { continue };
                if let Some(beacon) = open_beacon(&key, &buf[..n]) {
                    if beacon.d == me.id {
                        continue;
                    }
                    if cloud.lan.saw(beacon.d, SocketAddr::new(from.ip(), beacon.p)) {
                        cloud.changed();
                    }
                    if beacon.r {
                        let _ = udp.send_to(&seal_beacon(&key, &me, false), from).await;
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
                    let _ = tokio::time::timeout(IO_TIMEOUT, accept(&app, stream)).await;
                });
            }
            r = config.changed() => {
                let c = *config.borrow();
                if r.is_err() || !(c.signed_in && c.live && c.lan) {
                    return Ok(());
                }
            }
        }
    }
}

/// 往每張網卡的廣播位址送信標。
async fn broadcast(udp: &UdpSocket, cloud: &Cloud, me: &Me, reply: bool) {
    let Some(key) = cloud.key.lock().clone() else { return };
    let packet = seal_beacon(&key, me, reply);
    for target in broadcast_targets() {
        let _ = udp.send_to(&packet, SocketAddrV4::new(target, PORT)).await;
    }
}

/// 收一則直傳，解得開就回 1。
async fn accept(app: &AppHandle, mut stream: TcpStream) -> io::Result<()> {
    let msg = read_frame(&mut stream).await?;
    let cloud = app.state::<Cloud>();
    let ok = cloud.key.lock().clone().is_some_and(|key| relay::receive(app, &cloud, &key, &msg, Some(MAX_AGE)));
    write_ack(&mut stream, ok).await
}
