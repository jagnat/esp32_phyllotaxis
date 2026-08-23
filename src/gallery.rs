use alloc::vec;
use alloc::vec::Vec;

use defmt::{info, warn};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embedded_storage::{ReadStorage, Storage};
use esp_bootloader_esp_idf::partitions;
use esp_storage::FlashStorage;

pub const SLOTS: usize = 16;
pub const SLOT_SIZE: usize = 32 * 1024;
const HDR_LEN: usize = 64;
pub const MAX_SKETCH: usize = SLOT_SIZE - HDR_LEN;
const NAME_LEN: usize = 24;
const MAGIC: u32 = u32::from_le_bytes(*b"PHYW");

// Slot header: magic u32, data len u32, crc32 u32, name bytes (zero-padded),
// rest of HDR_LEN reserved. Module bytes start at slot + HDR_LEN.

#[derive(Clone, Copy)]
pub struct SlotInfo {
    name: [u8; NAME_LEN],
    pub len: u32,
}

impl SlotInfo {
    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }
}

pub struct Gallery {
    flash: FlashStorage<'static>,
    base: u32,
}

static GALLERY: Mutex<CriticalSectionRawMutex, Option<Gallery>> = Mutex::new(None);

pub fn init(flash: esp_hal::peripherals::FLASH<'static>) {
    let mut flash = FlashStorage::new(flash);
    let mut buf = vec![0u8; partitions::PARTITION_TABLE_MAX_LEN];
    let base = match partitions::read_partition_table(&mut flash, &mut buf) {
        Ok(table) => {
            let mut found = None;
            for i in 0..table.len() {
                if let Ok(p) = table.get_partition(i) {
                    if p.label_as_str().trim_end_matches('\0') == "sketches" {
                        found = Some((p.offset(), p.len()));
                    }
                }
            }
            found
        }
        Err(e) => {
            warn!("gallery: partition table unreadable: {}", e);
            None
        }
    };
    let Some((offset, len)) = base else {
        warn!("gallery: no 'sketches' partition; uploads disabled (reflash with partitions.csv)");
        return;
    };
    if (len as usize) < SLOTS * SLOT_SIZE {
        warn!("gallery: sketches partition too small: {} bytes", len);
        return;
    }
    info!("gallery: sketches partition at {:#x} ({} slots)", offset, SLOTS);
    let gallery = Gallery { flash, base: offset };
    // init runs before the executor, so try_lock cannot fail
    if let Ok(mut g) = GALLERY.try_lock() {
        *g = Some(gallery);
    }
}

pub async fn with<R>(f: impl FnOnce(&mut Gallery) -> R) -> Option<R> {
    let mut g = GALLERY.lock().await;
    g.as_mut().map(f)
}

#[derive(Clone, Copy, Debug, defmt::Format)]
pub enum Error {
    BadSlot,
    TooBig,
    Flash,
}

impl Gallery {
    fn slot_off(&self, slot: usize) -> u32 {
        self.base + (slot * SLOT_SIZE) as u32
    }

    fn header(&mut self, slot: usize) -> Option<SlotInfo> {
        let mut hdr = [0u8; HDR_LEN];
        self.flash.read(self.slot_off(slot), &mut hdr).ok()?;
        let magic = u32::from_le_bytes(hdr[0..4].try_into().unwrap());
        let len = u32::from_le_bytes(hdr[4..8].try_into().unwrap());
        if magic != MAGIC || len as usize > MAX_SKETCH {
            return None;
        }
        let mut name = [0u8; NAME_LEN];
        name.copy_from_slice(&hdr[12..12 + NAME_LEN]);
        Some(SlotInfo { name, len })
    }

    pub fn list(&mut self) -> [Option<SlotInfo>; SLOTS] {
        let mut out = [None; SLOTS];
        for (slot, entry) in out.iter_mut().enumerate() {
            *entry = self.header(slot);
        }
        out
    }

    pub fn read(&mut self, slot: usize) -> Option<Vec<u8>> {
        if slot >= SLOTS {
            return None;
        }
        let info = self.header(slot)?;
        let mut hdr = [0u8; HDR_LEN];
        self.flash.read(self.slot_off(slot), &mut hdr).ok()?;
        let crc = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
        let mut data = vec![0u8; info.len as usize];
        self.flash
            .read(self.slot_off(slot) + HDR_LEN as u32, &mut data)
            .ok()?;
        if crc32(&data) != crc {
            warn!("gallery: slot {} crc mismatch", slot);
            return None;
        }
        Some(data)
    }

    pub fn write(&mut self, slot: usize, name: &str, data: &[u8]) -> Result<(), Error> {
        if slot >= SLOTS {
            return Err(Error::BadSlot);
        }
        if data.len() > MAX_SKETCH {
            return Err(Error::TooBig);
        }
        let mut hdr = [0u8; HDR_LEN];
        hdr[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        hdr[4..8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        hdr[8..12].copy_from_slice(&crc32(data).to_le_bytes());
        let name = name.as_bytes();
        let n = name.len().min(NAME_LEN);
        hdr[12..12 + n].copy_from_slice(&name[..n]);
        let off = self.slot_off(slot);
        self.flash.write(off, &hdr).map_err(|_| Error::Flash)?;
        self.flash
            .write(off + HDR_LEN as u32, data)
            .map_err(|_| Error::Flash)?;
        Ok(())
    }

    pub fn erase(&mut self, slot: usize) -> Result<(), Error> {
        if slot >= SLOTS {
            return Err(Error::BadSlot);
        }
        let zeros = [0u8; HDR_LEN];
        self.flash
            .write(self.slot_off(slot), &zeros)
            .map_err(|_| Error::Flash)
    }
}

fn crc32(data: &[u8]) -> u32 {
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
