use alloc::vec;
use alloc::vec::Vec;

use defmt::{info, warn};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embedded_storage::{ReadStorage, Storage};
use esp_bootloader_esp_idf::partitions;
use esp_storage::FlashStorage;

use crate::gallery_format::{crc32, decode_header, encode_header, Metadata, HEADER_LEN};

pub use crate::gallery_format::{MAX_SKETCH, SLOT_SIZE, SLOTS};

#[derive(Clone, Copy)]
pub struct SlotInfo {
    metadata: Metadata,
    pub len: u32,
    pub abi_version: u32,
    pub caps: u32,
    pub color: [u8; 3],
    pub format_version: u16,
}

impl SlotInfo {
    pub fn name(&self) -> &str {
        self.metadata.name()
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
    BadName,
    Flash,
}

impl Gallery {
    fn slot_off(&self, slot: usize) -> u32 {
        self.base + (slot * SLOT_SIZE) as u32
    }

    fn header(&mut self, slot: usize) -> Option<SlotInfo> {
        if slot >= SLOTS {
            return None;
        }
        let mut header = [0u8; HEADER_LEN];
        self.flash.read(self.slot_off(slot), &mut header).ok()?;
        let metadata = decode_header(&header)?;
        Some(SlotInfo {
            metadata,
            len: metadata.len,
            abi_version: metadata.abi_version,
            caps: metadata.caps,
            color: metadata.color,
            format_version: metadata.format_version,
        })
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
        let mut data = vec![0u8; info.len as usize];
        self.flash
            .read(self.slot_off(slot) + HEADER_LEN as u32, &mut data)
            .ok()?;
        if crc32(&data) != info.metadata.crc {
            warn!("gallery: slot {} crc mismatch", slot);
            return None;
        }
        Some(data)
    }

    pub fn write(
        &mut self,
        slot: usize,
        name: &str,
        data: &[u8],
        caps: u32,
        color: [u8; 3],
    ) -> Result<(), Error> {
        if slot >= SLOTS {
            return Err(Error::BadSlot);
        }
        if data.len() > MAX_SKETCH {
            return Err(Error::TooBig);
        }
        let header = encode_header(name, data.len(), crc32(data), caps, color)
            .ok_or(Error::BadName)?;
        let off = self.slot_off(slot);

        // Invalidate first and commit the valid header last. A reset during
        // either payload write leaves an unreadable slot instead of a valid
        // header pointing at partial data.
        self.flash
            .write(off, &[0u8; HEADER_LEN])
            .map_err(|_| Error::Flash)?;
        self.flash
            .write(off + HEADER_LEN as u32, data)
            .map_err(|_| Error::Flash)?;
        self.flash
            .write(off, &header)
            .map_err(|_| Error::Flash)?;
        Ok(())
    }

    pub fn erase(&mut self, slot: usize) -> Result<(), Error> {
        if slot >= SLOTS {
            return Err(Error::BadSlot);
        }
        let zeros = [0u8; HEADER_LEN];
        self.flash
            .write(self.slot_off(slot), &zeros)
            .map_err(|_| Error::Flash)
    }
}
