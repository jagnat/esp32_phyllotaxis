pub const SLOTS: usize = 100;
pub const SLOT_SIZE: usize = 32 * 1024;
pub const HEADER_LEN: usize = 64;
pub const MAX_SKETCH: usize = SLOT_SIZE - HEADER_LEN;
pub const NAME_LEN: usize = 32;

const MAGIC: u32 = u32::from_le_bytes(*b"PHYW");
const FORMAT_VERSION: u16 = 2;
const ABI_VERSION: u32 = 2;
const LEGACY_NAME_LEN: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub name: [u8; NAME_LEN],
    pub len: u32,
    pub crc: u32,
    pub abi_version: u32,
    pub caps: u32,
    pub color: [u8; 3],
    pub format_version: u16,
}

impl Metadata {
    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }
}

pub fn encode_header(
    name: &str,
    len: usize,
    crc: u32,
    caps: u32,
    color: [u8; 3],
) -> Option<[u8; HEADER_LEN]> {
    if !crate::api_security::valid_name(name) || len > MAX_SKETCH {
        return None;
    }
    let mut header = [0u8; HEADER_LEN];
    header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    header[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
    header[8..12].copy_from_slice(&(len as u32).to_le_bytes());
    header[12..16].copy_from_slice(&crc.to_le_bytes());
    header[16..20].copy_from_slice(&ABI_VERSION.to_le_bytes());
    header[20..24].copy_from_slice(&caps.to_le_bytes());
    header[24..27].copy_from_slice(&color);
    header[28] = name.len() as u8;
    header[32..32 + name.len()].copy_from_slice(name.as_bytes());
    Some(header)
}

pub fn decode_header(header: &[u8; HEADER_LEN]) -> Option<Metadata> {
    if u32::from_le_bytes(header[0..4].try_into().ok()?) != MAGIC {
        return None;
    }

    let version = u16::from_le_bytes(header[4..6].try_into().ok()?);
    let header_len = u16::from_le_bytes(header[6..8].try_into().ok()?);
    if version == FORMAT_VERSION && header_len as usize == HEADER_LEN {
        let len = u32::from_le_bytes(header[8..12].try_into().ok()?);
        let crc = u32::from_le_bytes(header[12..16].try_into().ok()?);
        let abi_version = u32::from_le_bytes(header[16..20].try_into().ok()?);
        let caps = u32::from_le_bytes(header[20..24].try_into().ok()?);
        let name_len = header[28] as usize;
        if len as usize > MAX_SKETCH || abi_version != ABI_VERSION || name_len == 0 || name_len > NAME_LEN {
            return None;
        }
        let name_bytes = &header[32..32 + name_len];
        let name = core::str::from_utf8(name_bytes).ok()?;
        if !crate::api_security::valid_name(name) {
            return None;
        }
        let mut stored_name = [0u8; NAME_LEN];
        stored_name[..name_len].copy_from_slice(name_bytes);
        return Some(Metadata {
            name: stored_name,
            len,
            crc,
            abi_version,
            caps,
            color: [header[24], header[25], header[26]],
            format_version: version,
        });
    }

    // Legacy v1 layout: magic, len, crc, 24-byte zero-padded name. New v2's
    // version/header-length word cannot be mistaken for a valid v1 length.
    let len = u32::from_le_bytes(header[4..8].try_into().ok()?);
    if len as usize > MAX_SKETCH {
        return None;
    }
    let crc = u32::from_le_bytes(header[8..12].try_into().ok()?);
    let legacy_name = &header[12..12 + LEGACY_NAME_LEN];
    let name_len = legacy_name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(LEGACY_NAME_LEN);
    let name_str = core::str::from_utf8(&legacy_name[..name_len]).ok()?;
    if !crate::api_security::valid_name(name_str) {
        return None;
    }
    let mut name = [0u8; NAME_LEN];
    name[..name_len].copy_from_slice(&legacy_name[..name_len]);
    Some(Metadata {
        name,
        len,
        crc,
        abi_version: ABI_VERSION,
        caps: 0,
        color: crate::api_security::DEFAULT_MENU_COLOR,
        format_version: 1,
    })
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_hundred_slots_use_exactly_3_125_mib() {
        assert_eq!(SLOTS * SLOT_SIZE, 3_276_800);
    }

    #[test]
    fn v2_header_round_trips_metadata() {
        let header = encode_header("rainbow-2", 6_347, 0x1234_5678, 5, [1, 2, 3]).unwrap();
        let metadata = decode_header(&header).unwrap();
        assert_eq!(metadata.name(), "rainbow-2");
        assert_eq!(metadata.len, 6_347);
        assert_eq!(metadata.crc, 0x1234_5678);
        assert_eq!(metadata.abi_version, 2);
        assert_eq!(metadata.caps, 5);
        assert_eq!(metadata.color, [1, 2, 3]);
        assert_eq!(metadata.format_version, 2);
    }

    #[test]
    fn incomplete_or_corrupt_headers_fail_closed() {
        assert_eq!(decode_header(&[0u8; HEADER_LEN]), None);
        let mut header = encode_header("valid", 100, 1, 0, [1, 2, 3]).unwrap();
        header[6] = 0;
        assert_eq!(decode_header(&header), None);
    }

    #[test]
    fn legacy_headers_remain_readable() {
        let mut header = [0u8; HEADER_LEN];
        header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        header[4..8].copy_from_slice(&6_347u32.to_le_bytes());
        header[8..12].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        header[12..18].copy_from_slice(b"legacy");
        let metadata = decode_header(&header).unwrap();
        assert_eq!(metadata.name(), "legacy");
        assert_eq!(metadata.format_version, 1);
    }

    #[test]
    fn crc_detects_changes() {
        let crc = crc32(b"sketch");
        assert_ne!(crc, crc32(b"sketci"));
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}
