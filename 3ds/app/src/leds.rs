// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

#[cfg(target_os = "horizon")]
use std::ffi::c_void;

#[cfg(target_os = "horizon")]
const LED_REGISTER: u8 = 0x28;

#[cfg(target_os = "horizon")]
const APTHOOK_ONRESTORE: i32 = 1;
#[cfg(target_os = "horizon")]
const APTHOOK_ONWAKEUP: i32 = 3;

static ORIGINAL: Mutex<Option<u8>> = Mutex::new(None);
static WOKE: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "horizon")]
#[repr(C)]
struct AptHookCookie {
    next: *mut AptHookCookie,
    callback: Option<unsafe extern "C" fn(i32, *mut c_void)>,
    param: *mut c_void,
}

#[cfg(target_os = "horizon")]
static mut HOOK_COOKIE: AptHookCookie = AptHookCookie {
    next: std::ptr::null_mut(),
    callback: None,
    param: std::ptr::null_mut(),
};

#[cfg(target_os = "horizon")]
extern "C" {
    fn aptHook(
        cookie: *mut AptHookCookie,
        callback: Option<unsafe extern "C" fn(i32, *mut c_void)>,
        param: *mut c_void,
    );
    fn aptUnhook(cookie: *mut AptHookCookie);
}

#[cfg(target_os = "horizon")]
unsafe extern "C" fn apt_hook(hook: i32, _param: *mut c_void) {
    if hook == APTHOOK_ONWAKEUP || hook == APTHOOK_ONRESTORE {
        // No IPC inside the hook; defer to the main loop.
        WOKE.store(true, Ordering::Release);
    }
}

pub fn init() {
    #[cfg(target_os = "horizon")]
    unsafe {
        aptHook(
            std::ptr::addr_of_mut!(HOOK_COOKIE),
            Some(apt_hook),
            std::ptr::null_mut(),
        )
    };
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
            // Already off, but the MCU resets the register on wake; force the
            // LEDs off again so apply(false) stays idempotent.
            let _ = write_register(0);
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

/// Re-apply LEDs-off after the MCU reset them on wake. Call once per frame.
pub fn poll_wakeup() {
    if WOKE.swap(false, Ordering::AcqRel) && !crate::i18n::console_leds() {
        #[cfg(target_os = "horizon")]
        {
            // ORIGINAL already captured; force rewrite.
            let _ = write_register(0);
        }
    }
}

pub fn shutdown() {
    restore();
    #[cfg(target_os = "horizon")]
    unsafe {
        aptUnhook(std::ptr::addr_of_mut!(HOOK_COOKIE))
    };
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
        mcu_hwc_write(LED_REGISTER, std::ptr::addr_of!(value).cast::<c_void>(), 1)
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
