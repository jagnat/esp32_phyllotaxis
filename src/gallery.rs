use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use embedded_storage::nor_flash::NorFlash;
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
static REVISION: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
pub struct MenuEntry {
    pub slot: usize,
    pub color: [u8; 3],
}

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
        REVISION.store(1, Ordering::Release);
    }
}

pub fn revision() -> u32 {
    REVISION.load(Ordering::Acquire)
}

pub async fn with<R>(f: impl FnOnce(&mut Gallery) -> R) -> Option<R> {
    let mut g = GALLERY.lock().await;
    g.as_mut().map(f)
}

pub async fn write_slot(
    slot: usize,
    name: &str,
    data: &[u8],
    caps: u32,
    color: [u8; 3],
) -> Option<Result<(), Error>> {
    let mut gallery = GALLERY.lock().await;
    let gallery = gallery.as_mut()?;
    let result = gallery.write(slot, name, data, caps, color).await;
    if result.is_ok() {
        REVISION.fetch_add(1, Ordering::AcqRel);
    }
    Some(result)
}

pub async fn erase_slot(slot: usize) -> Option<Result<(), Error>> {
    let mut gallery = GALLERY.lock().await;
    let gallery = gallery.as_mut()?;
    let result = gallery.erase(slot);
    if result.is_ok() {
        REVISION.fetch_add(1, Ordering::AcqRel);
    }
    Some(result)
}

pub async fn menu_entries() -> Option<Vec<MenuEntry>> {
    let mut gallery = GALLERY.lock().await;
    let slots = gallery.as_mut()?.list();
    Some(
        slots
            .iter()
            .enumerate()
            .filter_map(|(slot, info)| {
                info.map(|info| MenuEntry {
                    slot,
                    color: info.color,
                })
            })
            .collect(),
    )
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

    async fn write(
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

        // Erase the complete slot first, then commit the header last. Yielding
        // between short flash operations lets rendering feed its watchdog and
        // lets the audio task drain its DMA ring.
        for sector in (off..off + SLOT_SIZE as u32).step_by(FlashStorage::SECTOR_SIZE as usize) {
            NorFlash::erase(
                &mut self.flash,
                sector,
                sector + FlashStorage::SECTOR_SIZE,
            )
            .map_err(|_| Error::Flash)?;
            Timer::after(Duration::from_millis(1)).await;
        }

        const WRITE_CHUNK: usize = 1024;
        let payload_off = off + HEADER_LEN as u32;
        let aligned_len = data.len() & !3;
        let mut written = 0usize;
        while written < aligned_len {
            let address = payload_off + written as u32;
            let sector_remaining =
                (FlashStorage::SECTOR_SIZE - address % FlashStorage::SECTOR_SIZE) as usize;
            let len = (aligned_len - written).min(WRITE_CHUNK).min(sector_remaining);
            NorFlash::write(
                &mut self.flash,
                address,
                &data[written..written + len],
            )
            .map_err(|_| Error::Flash)?;
            written += len;
            Timer::after(Duration::from_millis(1)).await;
        }
        if aligned_len != data.len() {
            let mut tail = [0xff; 4];
            tail[..data.len() - aligned_len].copy_from_slice(&data[aligned_len..]);
            NorFlash::write(&mut self.flash, payload_off + aligned_len as u32, &tail)
                .map_err(|_| Error::Flash)?;
            Timer::after(Duration::from_millis(1)).await;
        }
        NorFlash::write(&mut self.flash, off, &header).map_err(|_| Error::Flash)?;
        Ok(())
    }

    pub fn erase(&mut self, slot: usize) -> Result<(), Error> {
        if slot >= SLOTS {
            return Err(Error::BadSlot);
        }
        let zeros = [0u8; HEADER_LEN];
        let off = self.slot_off(slot);
        Storage::write(&mut self.flash, off, &zeros).map_err(|_| Error::Flash)
    }
}
