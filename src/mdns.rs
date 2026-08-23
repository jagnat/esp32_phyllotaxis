//! Minimal IPv4 mDNS responder for `phyllotaxis.local`.

use defmt::{info, warn};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpEndpoint, Ipv4Address, Stack};
use embassy_time::{Duration, Timer};

use crate::mdns_packet::{parse_query, response};

pub const HOSTNAME: &str = "phyllotaxis.local";

const PORT: u16 = 5353;
const GROUP: Ipv4Address = Ipv4Address::new(224, 0, 0, 251);

#[embassy_executor::task]
pub async fn responder(stack: Stack<'static>) {
    stack.wait_config_up().await;
    if stack.join_multicast_group(GROUP).is_err() {
        warn!("mdns: failed to join multicast group");
        return;
    }

    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0u8; 768];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_buf = [0u8; 256];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    if socket.bind(PORT).is_err() {
        warn!("mdns: failed to bind udp 5353");
        return;
    }

    let multicast = IpEndpoint::new(GROUP.into(), PORT);
    // Two unsolicited answers make the name available before the first query.
    for _ in 0..2 {
        if let Some(config) = stack.config_v4() {
            let packet = response(0, config.address.address().octets());
            let _ = socket.send_to(&packet, multicast).await;
        }
        Timer::after(Duration::from_secs(1)).await;
    }
    info!("mdns: announcing {}", HOSTNAME);

    let mut packet = [0u8; 512];
    loop {
        let (len, metadata) = match socket.recv_from(&mut packet).await {
            Ok(received) => received,
            Err(_) => continue,
        };
        let Some(query) = parse_query(&packet[..len], metadata.endpoint.port) else {
            continue;
        };
        let Some(config) = stack.config_v4() else {
            continue;
        };
        let destination = if query.unicast_response {
            metadata.endpoint
        } else {
            multicast
        };
        let id = if query.unicast_response { query.id } else { 0 };
        let answer = response(id, config.address.address().octets());
        let _ = socket.send_to(&answer, destination).await;
    }
}
