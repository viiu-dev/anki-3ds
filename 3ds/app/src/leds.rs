// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::sync::Mutex;

#[cfg(target_os = "horizon")]
use std::ffi::c_void;

#[cfg(target_os = "horizon")]
const LED_REGISTER: u8 = 0x28;

static ORIGINAL: Mutex<Option<u8>> = Mutex::new(None);

pub fn init() {
    apply(crate::i18n::console_leds());
}

pub fn apply(enabled: bool) {
    if enabled {
        restore();
    } else {
        disable();
    }
}

pub fn restore() {
    #[cfg(target_os = "horizon")]
    {
        let mut original = ORIGINAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(value) = original.take() {
            let _ = write_register(value);
        }
    }
}

fn disable() {
    #[cfg(target_os = "horizon")]
    {
        let mut original = ORIGINAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if original.is_some() {
            return;
        }
        let Some(value) = read_register() else {
            return;
        };
        if write_register(0).is_ok() {
            *original = Some(value);
        }
    }
}

#[cfg(target_os = "horizon")]
fn with_service<T>(action: impl FnOnce() -> T) -> Option<T> {
    if unsafe { mcu_hwc_init() } != 0 {
        return None;
    }
    let value = action();
    unsafe { mcu_hwc_exit() };
    Some(value)
}

#[cfg(target_os = "horizon")]
fn read_register() -> Option<u8> {
    with_service(|| {
        let mut value: u8 = 0;
        let result = unsafe {
            mcu_hwc_read(
                LED_REGISTER,
                std::ptr::addr_of_mut!(value).cast::<c_void>(),
                1,
            )
        };
        (result == 0).then_some(value)
    })
    .flatten()
}

#[cfg(target_os = "horizon")]
fn write_register(value: u8) -> Result<(), ()> {
    let result = with_service(|| unsafe {
        mcu_hwc_write(
            LED_REGISTER,
            std::ptr::addr_of!(value).cast::<c_void>(),
            1,
        )
    });
    match result {
        Some(0) => Ok(()),
        _ => Err(()),
    }
}

#[cfg(target_os = "horizon")]
extern "C" {
    #[link_name = "mcuHwcInit"]
    fn mcu_hwc_init() -> i32;
    #[link_name = "mcuHwcExit"]
    fn mcu_hwc_exit();
    #[link_name = "MCUHWC_ReadRegister"]
    fn mcu_hwc_read(reg: u8, data: *mut c_void, size: u32) -> i32;
    #[link_name = "MCUHWC_WriteRegister"]
    fn mcu_hwc_write(reg: u8, data: *const c_void, size: u32) -> i32;
}
