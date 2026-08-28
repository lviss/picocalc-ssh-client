use crate::fixed_str::FixedString;
use crate::screen::SCREEN;
use embassy_rp::flash::{ERASE_SIZE, FLASH_BASE, PAGE_SIZE, WRITE_SIZE};
use embassy_rp::multicore::{pause_core1, resume_core1};
use embassy_rp::peripherals::FLASH;
use embassy_rp::rom_data;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::lazy_lock::LazyLock;
use embassy_sync::mutex::Mutex;
use embedded_storage_async::nor_flash::{
    ErrorType, MultiwriteNorFlash, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
};
use heapless::FnvIndexMap;
use sequential_storage::cache::NoCache;
use sequential_storage::erase_all;
use sequential_storage::map::{fetch_all_items, fetch_item, remove_item, store_item};

// Used only when no RP2350 resident partition table is present (plain
// BOOTSEL flashing straight to flash offset 0, no uf2loader-style
// bootloader in front of us) - must match the flash chip size assumed by
// the matching linker script (memory.x / pico2w.x / pimoroni2w.x), which
// reserves the top `CONFIG_SIZE` bytes of this same total for config
// storage. When a partition table *is* present, `query_app_partition_size`
// below gives the real, per-device answer instead of this compile-time
// guess.
#[cfg(feature = "pico2w")]
const FALLBACK_FLASH_SIZE: u32 = 4 * 1024 * 1024;
#[cfg(feature = "pimoroni2w")]
const FALLBACK_FLASH_SIZE: u32 = 16 * 1024 * 1024;
#[cfg(not(any(feature = "pico2w", feature = "pimoroni2w")))]
const FALLBACK_FLASH_SIZE: u32 = 2 * 1024 * 1024;

// sequential_storage requires at least 2 erase-size pages to operate
// (see sequential_storage::map's internal assert on flash_range length),
// so this can't be shrunk to a single erase sector.
pub const CONFIG_SIZE: u32 = ERASE_SIZE as u32 * 2;
const SCRATCH_SIZE: usize = PAGE_SIZE * 2;

pub static CONFIG: LazyLock<Mutex<CriticalSectionRawMutex, Configuration>> =
    LazyLock::new(|| Mutex::new(Configuration::default()));

#[derive(Debug, Default)]
pub struct Configuration {
    flash: Option<Flash>,
}

pub type StrKey = FixedString<32>;
pub type StrValue = FixedString<128>;

impl Configuration {
    pub fn assign_flash(&mut self, flash: Flash) {
        self.flash.replace(flash);
    }

    pub async fn fetch(
        &mut self,
        key: &str,
    ) -> Result<Option<StrValue>, sequential_storage::Error<RomFlashError>> {
        match &mut self.flash {
            Some(flash) => {
                let key: StrKey = key.try_into()?;
                let mut buf = [0u8; SCRATCH_SIZE];
                let range = flash.config_range();
                fetch_item(flash, range, &mut NoCache::new(), &mut buf, &key).await
            }
            None => {
                todo!();
            }
        }
    }

    pub async fn remove(
        &mut self,
        key: &str,
    ) -> Result<(), sequential_storage::Error<RomFlashError>> {
        match &mut self.flash {
            Some(flash) => {
                let key: StrKey = key.try_into()?;
                let mut buf = [0u8; SCRATCH_SIZE];
                let range = flash.config_range();
                remove_item(flash, range, &mut NoCache::new(), &mut buf, &key).await
            }
            None => {
                todo!();
            }
        }
    }

    pub async fn store(
        &mut self,
        key: &str,
        value: StrValue,
    ) -> Result<(), sequential_storage::Error<RomFlashError>> {
        match &mut self.flash {
            Some(flash) => {
                let key: StrKey = key.try_into()?;
                let mut buf = [0u8; SCRATCH_SIZE];
                let range = flash.config_range();
                store_item(flash, range, &mut NoCache::new(), &mut buf, &key, &value).await
            }
            None => {
                todo!();
            }
        }
    }

    pub async fn format(&mut self) -> Result<(), sequential_storage::Error<RomFlashError>> {
        match &mut self.flash {
            Some(flash) => {
                let range = flash.config_range();
                erase_all(flash, range).await
            }
            None => {
                todo!();
            }
        }
    }

    pub async fn get_all(
        &mut self,
    ) -> Result<FnvIndexMap<StrKey, StrValue, 32>, sequential_storage::Error<RomFlashError>> {
        match &mut self.flash {
            Some(flash) => {
                let mut buf = [0u8; SCRATCH_SIZE];
                let mut cache = NoCache::new();
                let range = flash.config_range();
                let mut iter =
                    fetch_all_items::<StrKey, _, _>(flash, range, &mut cache, &mut buf).await?;

                let mut map = FnvIndexMap::new();

                while let Some((key, value)) = iter.next::<StrKey, StrValue>(&mut buf).await? {
                    if let Err((k, v)) = map.insert(key, value) {
                        print!("Configuration::get_all: too many keys. Ignoring {k} -> {v}\r\n");
                    }
                }

                Ok(map)
            }
            None => {
                todo!();
            }
        }
    }
}

/// Bit layout for a `rom_flash_op` flags word (RP2350 datasheet section
/// 5.5.8.2 / pico-sdk's `boot/bootrom_constants.h` CFLASH_* constants,
/// which aren't exposed by `embassy_rp`).
mod cflash {
    pub const ASPACE_LSB: u32 = 0;
    pub const ASPACE_VALUE_RUNTIME: u32 = 1;
    pub const SECLEVEL_LSB: u32 = 8;
    pub const SECLEVEL_VALUE_SECURE: u32 = 1;
    pub const OP_LSB: u32 = 16;
    pub const OP_VALUE_ERASE: u32 = 0;
    pub const OP_VALUE_PROGRAM: u32 = 1;

    /// This firmware always builds and links as an ARM-Secure image
    /// (confirmed via `picotool info` on release builds), so every
    /// `flash_op` call here uses `SECLEVEL_VALUE_SECURE` - matching what
    /// uf2loader's own bootloader does for the same reason (see
    /// `FLASH_ERASE`/`FLASH_PROG` in `ui/uf2.c` of
    /// https://github.com/pelrun/uf2loader). `ASPACE_VALUE_RUNTIME`
    /// addresses are relative to this image's own linked flash origin
    /// (the same convention `config_base` already uses); the bootrom
    /// transparently translates them to the real physical location via
    /// the QMI address-translation registers, which is what lets
    /// `config_base` stay partition-relative without this code ever
    /// needing to know the partition's physical offset.
    pub fn flags(op: u32) -> u32 {
        (ASPACE_VALUE_RUNTIME << ASPACE_LSB)
            | (SECLEVEL_VALUE_SECURE << SECLEVEL_LSB)
            | (op << OP_LSB)
    }
}

// pico-sdk's pico/bootrom.h documents 3264 bytes as the current minimum
// work area size for `load_partition_table`; rounded up slightly here.
const PARTITION_WORKAREA_BYTES: usize = 3328;

/// Queries the size of this image's own flash partition from the RP2350
/// bootrom's resident partition table (set up by e.g. uf2loader), instead
/// of assuming this image owns the whole flash chip.
///
/// Mirrors uf2loader's own `bl_app_partition_get_info()`
/// (`common/bootloader/proginfo.c` in https://github.com/pelrun/uf2loader):
/// it assumes partition index 0 is this image's own app partition, which
/// matches uf2loader's single-partition `stage3/partitions.json.in`
/// layout. Returns `None` if there is no resident partition table (plain
/// BOOTSEL flashing straight to flash offset 0) or the bootrom call fails
/// for any other reason; the caller falls back to `FALLBACK_FLASH_SIZE`.
fn query_app_partition_size() -> Option<u32> {
    const PT_INFO_SINGLE_PARTITION: u32 = 0x8000;
    const PT_INFO_PARTITION_LOCATION_AND_FLAGS: u32 = 0x0010;
    const PICOBIN_PARTITION_LOCATION_FIRST_SECTOR_LSB: u32 = 0;
    const PICOBIN_PARTITION_LOCATION_FIRST_SECTOR_BITS: u32 = 0x0000_1fff;
    const PICOBIN_PARTITION_LOCATION_LAST_SECTOR_LSB: u32 = 13;
    const PICOBIN_PARTITION_LOCATION_LAST_SECTOR_BITS: u32 = 0x03ff_e000;
    let flash_sector_size = ERASE_SIZE as u32;

    let workarea: &'static mut [u8; PARTITION_WORKAREA_BYTES] = crate::mk_static!(
        [u8; PARTITION_WORKAREA_BYTES],
        [0u8; PARTITION_WORKAREA_BYTES]
    );

    // SAFETY: `load_partition_table`/`get_partition_table_info` are raw
    // RP2350 bootrom calls (see `embassy_rp::rom_data`), only valid to
    // call because this firmware always builds and links as an ARM-Secure
    // image. `workarea` is `'static` and sized above the 3264-byte
    // minimum the bootrom documents for `load_partition_table`. This
    // function is only ever called once, from single-threaded core0
    // startup code before the async executor (and thus any concurrent
    // caller) is running, so there is no reentrancy hazard.
    let rc =
        unsafe { rom_data::load_partition_table(workarea.as_mut_ptr(), workarea.len(), false) };
    if rc != 0 {
        // No resident partition table (e.g. BOOTROM_ERROR_NOT_FOUND under
        // plain BOOTSEL flashing), or some other bootrom error.
        return None;
    }

    let mut info = [0u32; 3];
    let flags_and_partition =
        PT_INFO_PARTITION_LOCATION_AND_FLAGS | PT_INFO_SINGLE_PARTITION | (0u32 << 24);
    // SAFETY: `info` is a 3-word buffer, matching the documented output
    // shape for PT_INFO_PARTITION_LOCATION_AND_FLAGS with SINGLE_PARTITION
    // set (1 "supported flags" word + 2 words of location/permission
    // data) - see pico-sdk's `pico/bootrom.h` doc comment on
    // `rom_get_partition_table_info`.
    let rc = unsafe {
        rom_data::get_partition_table_info(info.as_mut_ptr(), info.len(), flags_and_partition)
    };
    if rc < 0 {
        return None;
    }

    let location = info[1];
    let first_sector = (location & PICOBIN_PARTITION_LOCATION_FIRST_SECTOR_BITS)
        >> PICOBIN_PARTITION_LOCATION_FIRST_SECTOR_LSB;
    let last_sector = (location & PICOBIN_PARTITION_LOCATION_LAST_SECTOR_BITS)
        >> PICOBIN_PARTITION_LOCATION_LAST_SECTOR_LSB;

    let partition_start = first_sector * flash_sector_size;
    let partition_end = (last_sector + 1) * flash_sector_size;
    Some(partition_end - partition_start)
}

#[derive(Debug)]
pub struct RomFlashError {
    /// Raw RP2350 bootrom return code; see BOOTROM_ERROR_* in pico-sdk's
    /// `boot/bootrom_constants.h` (e.g. -4 = not permitted, outside this
    /// image's partition permissions; -10 = invalid/out-of-bounds address;
    /// -11 = bad alignment).
    code: i32,
}

impl NorFlashError for RomFlashError {
    fn kind(&self) -> NorFlashErrorKind {
        match self.code {
            -11 => NorFlashErrorKind::NotAligned,
            -10 | -4 => NorFlashErrorKind::OutOfBounds,
            _ => NorFlashErrorKind::Other,
        }
    }
}

pub struct Flash {
    /// Offset (relative to this image's own linked flash origin, i.e. the
    /// same address space `FLASH_BASE` starts at) of the config store's
    /// first byte. Computed once in `Flash::new` from the real partition
    /// size if a uf2loader-style RP2350 partition table is present, or
    /// from `FALLBACK_FLASH_SIZE` otherwise.
    config_base: u32,
}

impl core::fmt::Debug for Flash {
    fn fmt(&self, fmt: &mut core::fmt::Formatter) -> core::fmt::Result {
        fmt.debug_struct("Flash")
            .field("config_base", &self.config_base)
            .finish()
    }
}

impl Flash {
    /// Takes ownership of the `FLASH` peripheral purely to guarantee this
    /// is the only thing in the firmware that touches internal flash -
    /// the actual reads/erases/writes below are raw RP2350 bootrom calls
    /// (`embassy_rp::rom_data`), not peripheral register access, so no
    /// DMA channel is needed here (unlike `embassy_rp::flash::Flash`,
    /// which this type replaces for the config store specifically, so
    /// that erase/write go through the RP2350's partition-aware
    /// `rom_flash_op` instead of the legacy `flash_range_erase`/
    /// `flash_range_program` bootrom calls `embassy_rp::flash::Flash`
    /// uses, which uf2loader's own README documents as unsafe under a
    /// resident partition table).
    ///
    /// Requires core1 to already be running (see the trivial core1 spawn
    /// in `main.rs`) before any erase/write happens: `pause_core1`/
    /// `resume_core1` below hang forever waiting for a core1 that was
    /// never started.
    pub fn new(_flash: FLASH) -> Self {
        let config_base = match query_app_partition_size() {
            Some(partition_size) => {
                log::info!(
                    "flash: resident partition table found, app partition size={partition_size}"
                );
                partition_size.saturating_sub(CONFIG_SIZE)
            }
            None => {
                log::info!(
                    "flash: no resident partition table, assuming full {FALLBACK_FLASH_SIZE}-byte chip"
                );
                FALLBACK_FLASH_SIZE - CONFIG_SIZE
            }
        };
        log::info!(
            "config store: base=0x{config_base:x} size={CONFIG_SIZE} write={WRITE_SIZE} erase={ERASE_SIZE}"
        );
        Self { config_base }
    }

    fn config_range(&self) -> core::ops::Range<u32> {
        self.config_base..self.region_end()
    }

    fn region_end(&self) -> u32 {
        self.config_base + CONFIG_SIZE
    }

    fn check_bounds(&self, offset: u32, end: u32) -> Result<(), RomFlashError> {
        if offset < self.config_base || end < offset || end > self.region_end() {
            return Err(RomFlashError { code: -10 }); // BOOTROM_ERROR_INVALID_ADDRESS
        }
        Ok(())
    }
}

impl ErrorType for Flash {
    type Error = RomFlashError;
}

impl ReadNorFlash for Flash {
    const READ_SIZE: usize = 1;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.check_bounds(offset, offset.saturating_add(bytes.len() as u32))?;
        // SAFETY: under a uf2loader-style RP2350 partition table, the QMI
        // address translation registers already remap this image's own
        // partition to appear at FLASH_BASE for XIP purposes (the same
        // "flash runtime address" space `rom_flash_op`'s ASPACE_RUNTIME
        // documents), so a direct XIP read at FLASH_BASE + offset is
        // already partition-correct without going through `flash_op`.
        // `check_bounds` above confirmed offset..offset+bytes.len() lies
        // within this image's own config region.
        let addr = (FLASH_BASE as u32 + offset) as *const u8;
        let src = unsafe { core::slice::from_raw_parts(addr, bytes.len()) };
        bytes.copy_from_slice(src);
        Ok(())
    }

    fn capacity(&self) -> usize {
        self.region_end() as usize
    }
}

impl NorFlash for Flash {
    const WRITE_SIZE: usize = WRITE_SIZE;
    const ERASE_SIZE: usize = ERASE_SIZE;

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        self.check_bounds(from, to)?;
        let flags = cflash::flags(cflash::OP_VALUE_ERASE);
        // SAFETY: core1 is paused for the duration of this call, so it
        // cannot be mid-fetch from the XIP flash region being erased.
        // `from`/`to` were just bounds-checked into this image's own
        // config region above, and are additionally bounds- and
        // permission-checked by the bootrom itself against the resident
        // partition table, if any (see `rom_flash_op` in pico-sdk's
        // `pico/bootrom.h`).
        pause_core1();
        let rc = unsafe {
            rom_data::flash_op(
                flags,
                FLASH_BASE as u32 + from,
                to - from,
                core::ptr::null_mut(),
            )
        };
        resume_core1();
        if rc < 0 {
            Err(RomFlashError { code: rc })
        } else {
            Ok(())
        }
    }

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.check_bounds(offset, offset.saturating_add(bytes.len() as u32))?;
        let flags = cflash::flags(cflash::OP_VALUE_PROGRAM);
        // SAFETY: see `erase` above - same core1-pause and bootrom
        // bounds/permission-checking rationale applies to programming.
        pause_core1();
        let rc = unsafe {
            rom_data::flash_op(
                flags,
                FLASH_BASE as u32 + offset,
                bytes.len() as u32,
                bytes.as_ptr() as *mut u8,
            )
        };
        resume_core1();
        if rc < 0 {
            Err(RomFlashError { code: rc })
        } else {
            Ok(())
        }
    }
}

impl MultiwriteNorFlash for Flash {}

pub async fn config_command(args: &[&str]) {
    match args {
        ["config", "format"] => {
            let mut config = CONFIG.get().lock().await;
            let result = config.format().await;
            print!("{result:?}");
        }
        ["config", "list"] => {
            let mut config = CONFIG.get().lock().await;
            match config.get_all().await {
                Ok(map) => {
                    for (k, v) in &map {
                        print!("{k}={v}\r\n");
                    }
                }
                Err(err) => {
                    print!("{err:?}\r\n");
                }
            }
        }
        ["config", "get", key] => {
            if *key == "scroll" {
                let mut config = CONFIG.get().lock().await;
                match config.fetch(key).await {
                    Ok(Some(val)) => print!("{val}\r\n"),
                    Ok(None) => print!("200\r\n"),
                    Err(e) => print!("{e:?}\r\n"),
                }
                return;
            }
            let mut config = CONFIG.get().lock().await;
            let value = config.fetch(key).await;
            print!("{value:?}\r\n");
        }
        ["config", "rm", key] => {
            if *key == "scroll" {
                SCREEN.get().lock().await.set_max_scrollback(200);
            }
            let mut config = CONFIG.get().lock().await;
            let result = config.remove(key).await;
            print!("{result:?}\r\n");
        }
        ["config", "set", key, value] => {
            if *key == "scroll" {
                if let Ok(val) = value.parse::<usize>() {
                    if val <= 500 {
                        SCREEN.get().lock().await.set_max_scrollback(val);
                    } else {
                        print!("scroll value must be <= 500\r\n");
                        return;
                    }
                } else {
                    print!("scroll value must be a number\r\n");
                    return;
                }
            }
            let value: StrValue = match (*value).try_into() {
                Ok(v) => v,
                Err(err) => {
                    print!("value `{value}`: {err:?}\r\n");
                    return;
                }
            };
            let mut config = CONFIG.get().lock().await;
            match config.store(key, value).await {
                Ok(()) => {
                    print!("OK\r\n");
                }
                Err(err) => {
                    print!("{err:?}\r\n");
                }
            }
        }
        _ => {
            print!("invalid arguments\r\n");
        }
    }
}
