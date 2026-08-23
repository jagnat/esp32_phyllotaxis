//! Allocation-free DNS packet parsing/building, kept platform-independent.

const PORT: u16 = 5353;
const TTL_SECONDS: u32 = 120;
const DNS_HEADER_LEN: usize = 12;
const HOST_WIRE: &[u8] = b"\x0bphyllotaxis\x05local\x00";
const RESPONSE_LEN: usize = DNS_HEADER_LEN + HOST_WIRE.len() + 10 + 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Query {
    pub id: u16,
    pub unicast_response: bool,
}

fn eq_ascii_case(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(&a, &b)| a.eq_ignore_ascii_case(&b))
}

/// Match the fixed two-label hostname while safely following DNS pointers.
fn name_matches(packet: &[u8], start: usize) -> Option<(bool, usize)> {
    let expected: [&[u8]; 2] = [b"phyllotaxis", b"local"];
    let mut cursor = start;
    let mut encoded_end = None;
    let mut label = 0;
    let mut jumps = 0;
    let mut matches = true;

    loop {
        let len = *packet.get(cursor)?;
        if len & 0xc0 == 0xc0 {
            let low = *packet.get(cursor + 1)? as usize;
            let pointer = (((len & 0x3f) as usize) << 8) | low;
            encoded_end.get_or_insert(cursor + 2);
            cursor = pointer;
            jumps += 1;
            if jumps > 8 {
                return None;
            }
            continue;
        }
        if len & 0xc0 != 0 {
            return None;
        }
        cursor += 1;
        if len == 0 {
            let end = encoded_end.unwrap_or(cursor);
            return Some((matches && label == expected.len(), end));
        }
        let end = cursor.checked_add(len as usize)?;
        let bytes = packet.get(cursor..end)?;
        if label >= expected.len() || !eq_ascii_case(bytes, expected[label]) {
            matches = false;
        }
        label += 1;
        cursor = end;
    }
}

pub(crate) fn parse_query(packet: &[u8], source_port: u16) -> Option<Query> {
    if packet.len() < DNS_HEADER_LEN || packet[2] & 0x80 != 0 {
        return None;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let questions = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let mut offset = DNS_HEADER_LEN;

    for _ in 0..questions {
        let (name_is_ours, end) = name_matches(packet, offset)?;
        let tail = packet.get(end..end + 4)?;
        let qtype = u16::from_be_bytes([tail[0], tail[1]]);
        let qclass = u16::from_be_bytes([tail[2], tail[3]]);
        offset = end + 4;
        if name_is_ours && matches!(qtype, 1 | 255) && qclass & 0x7fff == 1 {
            return Some(Query {
                id,
                unicast_response: source_port != PORT || qclass & 0x8000 != 0,
            });
        }
    }
    None
}

pub(crate) fn response(id: u16, address: [u8; 4]) -> [u8; RESPONSE_LEN] {
    let mut out = [0u8; RESPONSE_LEN];
    out[0..2].copy_from_slice(&id.to_be_bytes());
    out[2..4].copy_from_slice(&0x8400u16.to_be_bytes()); // response + authoritative
    out[6..8].copy_from_slice(&1u16.to_be_bytes()); // one answer
    let mut offset = DNS_HEADER_LEN;
    out[offset..offset + HOST_WIRE.len()].copy_from_slice(HOST_WIRE);
    offset += HOST_WIRE.len();
    out[offset..offset + 2].copy_from_slice(&1u16.to_be_bytes()); // A
    out[offset + 2..offset + 4].copy_from_slice(&0x8001u16.to_be_bytes()); // IN + cache flush
    out[offset + 4..offset + 8].copy_from_slice(&TTL_SECONDS.to_be_bytes());
    out[offset + 8..offset + 10].copy_from_slice(&4u16.to_be_bytes());
    out[offset + 10..offset + 14].copy_from_slice(&address);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(qtype: u16, qclass: u16) -> [u8; 35] {
        let mut packet = [0u8; 35];
        packet[4..6].copy_from_slice(&1u16.to_be_bytes());
        packet[12..31].copy_from_slice(HOST_WIRE);
        packet[31..33].copy_from_slice(&qtype.to_be_bytes());
        packet[33..35].copy_from_slice(&qclass.to_be_bytes());
        packet
    }

    #[test]
    fn accepts_a_and_any_queries() {
        assert_eq!(
            parse_query(&query(1, 1), PORT),
            Some(Query {
                id: 0,
                unicast_response: false
            })
        );
        assert!(parse_query(&query(255, 1), PORT).is_some());
    }

    #[test]
    fn honors_unicast_response_and_legacy_queries() {
        assert!(
            parse_query(&query(1, 0x8001), PORT)
                .unwrap()
                .unicast_response
        );
        assert!(parse_query(&query(1, 1), 49152).unwrap().unicast_response);
    }

    #[test]
    fn rejects_other_names_and_record_types() {
        let mut other = query(1, 1);
        other[13] = b'x';
        assert_eq!(parse_query(&other, PORT), None);
        assert_eq!(parse_query(&query(28, 1), PORT), None);
    }

    #[test]
    fn response_contains_cache_flushed_a_record() {
        let packet = response(0, [10, 20, 30, 40]);
        assert_eq!(&packet[12..31], HOST_WIRE);
        assert_eq!(&packet[31..33], &1u16.to_be_bytes());
        assert_eq!(&packet[33..35], &0x8001u16.to_be_bytes());
        assert_eq!(&packet[41..45], &[10, 20, 30, 40]);
    }

    #[test]
    fn malformed_packets_fail_closed() {
        assert_eq!(parse_query(&[], PORT), None);
        let mut looped = query(1, 1);
        looped[12] = 0xc0;
        looped[13] = 12;
        assert_eq!(parse_query(&looped, PORT), None);
    }
}
