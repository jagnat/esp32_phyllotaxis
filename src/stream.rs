use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpEndpoint, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Instant};

use defmt::{info, unwrap};

use crate::audio::AudioFeatures;
use crate::input;
use crate::util::NUM_LEDS;

pub const DDP_PORT: u16 = 4048;
const SESSION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Udp(IpEndpoint),
    Ws(u32),
}

static SESSION: Mutex<CriticalSectionRawMutex, RefCell<Option<(Source, Instant)>>> =
    Mutex::new(RefCell::new(None));

/// One streamer at a time: the first source to send owns the display until it
/// stays silent for SESSION_TIMEOUT.
fn try_claim(src: Source) -> bool {
    SESSION.lock(|s| {
        let mut s = s.borrow_mut();
        let now = Instant::now();
        match *s {
            Some((owner, last)) if owner != src && now - last < SESSION_TIMEOUT => false,
            _ => {
                *s = Some((src, now));
                true
            }
        }
    })
}

fn accept_frame(offset: usize, data: &[u8]) {
    FRAME.lock(|f| {
        let mut f = f.borrow_mut();
        let total = f.len();
        if offset < total {
            let end = (offset + data.len()).min(total);
            f[offset..end].copy_from_slice(&data[..end - offset]);
        }
    });
    LAST_FRAME_MS.store(Instant::now().as_millis() as u32 | 1, Ordering::Relaxed);
}

pub static FRAME: Mutex<CriticalSectionRawMutex, RefCell<[u8; NUM_LEDS * 3]>> =
    Mutex::new(RefCell::new([0; NUM_LEDS * 3]));

/// Milliseconds timestamp of the last accepted frame; 0 = never.
pub static LAST_FRAME_MS: AtomicU32 = AtomicU32::new(0);

pub static AUDIO_SNAPSHOT: Mutex<CriticalSectionRawMutex, RefCell<AudioFeatures>> =
    Mutex::new(RefCell::new(AudioFeatures {
        bands: [0.0; 6],
        transients: [0.0; 6],
        sustain: [0.0; 6],
        super_sustain: [0.0; 6],
    }));

/// Telemetry reply, little-endian: b"PHY1", seq u32, encoder_pos i32,
/// button u8 (bit0 = pressed), 3 pad bytes, then bands/transients/sustain/
/// super_sustain as 4 x 6 f32.
const TELEMETRY_LEN: usize = 16 + 4 * 6 * 4;

fn build_telemetry(seq: u32, out: &mut [u8; TELEMETRY_LEN]) {
    out[0..4].copy_from_slice(b"PHY1");
    out[4..8].copy_from_slice(&seq.to_le_bytes());
    let pos = input::ENCODER_POS.load(Ordering::Relaxed);
    out[8..12].copy_from_slice(&pos.to_le_bytes());
    out[12] = input::BUTTON_PRESSED.load(Ordering::Relaxed) as u8;
    out[13..16].fill(0);
    AUDIO_SNAPSHOT.lock(|a| {
        let a = a.borrow();
        let mut i = 16;
        for arr in [&a.bands, &a.transients, &a.sustain, &a.super_sustain] {
            for v in arr {
                out[i..i + 4].copy_from_slice(&v.to_le_bytes());
                i += 4;
            }
        }
    });
}

#[embassy_executor::task]
pub async fn ddp_task(stack: Stack<'static>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 8];
    let mut rx_buf = [0u8; 1536];
    let mut tx_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_buf = [0u8; 512];
    let mut sock = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buf,
        &mut tx_meta,
        &mut tx_buf,
    );
    unwrap!(sock.bind(DDP_PORT));
    info!("ddp listening on udp {}", DDP_PORT);

    let mut seq: u32 = 0;
    let mut pkt = [0u8; 600];
    let mut telemetry = [0u8; TELEMETRY_LEN];
    loop {
        let (n, meta) = match sock.recv_from(&mut pkt).await {
            Ok(x) => x,
            Err(_) => continue,
        };
        if !try_claim(Source::Udp(meta.endpoint)) {
            continue;
        }
        if n < 10 || pkt[0] & 0xc0 != 0x40 {
            continue;
        }
        let off = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]) as usize;
        let len = u16::from_be_bytes([pkt[8], pkt[9]]) as usize;
        if 10 + len > n {
            continue;
        }
        accept_frame(off, &pkt[10..10 + len]);

        seq = seq.wrapping_add(1);
        build_telemetry(seq, &mut telemetry);
        let _ = sock.send_to(&telemetry, meta.endpoint).await;
    }
}

pub struct WsStream;

static WS_IDS: AtomicU32 = AtomicU32::new(0);

impl picoserve::response::ws::WebSocketCallback for WsStream {
    async fn run<R: picoserve::io::Read, W: picoserve::io::Write<Error = R::Error>>(
        self,
        mut rx: picoserve::response::ws::SocketRx<R>,
        mut tx: picoserve::response::ws::SocketTx<W>,
    ) -> Result<(), W::Error> {
        use picoserve::futures::Either;
        use picoserve::response::ws::Message;

        let id = Source::Ws(WS_IDS.fetch_add(1, Ordering::Relaxed));
        let mut seq: u32 = 0;
        let mut buf = [0u8; 600];
        let mut telemetry = [0u8; TELEMETRY_LEN];
        loop {
            let msg = match rx.next_message(&mut buf, core::future::pending::<()>()).await? {
                Either::First(Ok(m)) => m,
                Either::First(Err(_)) => return tx.close(None).await,
                Either::Second(()) => continue,
            };
            match msg {
                Message::Binary(data) => {
                    if !try_claim(id) {
                        continue;
                    }
                    accept_frame(0, data);
                    seq = seq.wrapping_add(1);
                    build_telemetry(seq, &mut telemetry);
                    tx.send_binary(&telemetry).await?;
                }
                Message::Ping(data) => tx.send_pong(data).await?,
                Message::Close(_) => return tx.close(None).await,
                _ => {}
            }
        }
    }
}

pub fn stream_fresh() -> bool {
    let last = LAST_FRAME_MS.load(Ordering::Relaxed);
    if last == 0 {
        return false;
    }
    (Instant::now().as_millis() as u32).wrapping_sub(last) < 1000
}
