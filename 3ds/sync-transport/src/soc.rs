// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Explicit ownership of libctru's caller-allocated SOC buffer.
//!
//! The 1 MiB, page-aligned SOC buffer is static storage that lives for the
//! whole app. It is zero-initialised, so it sits in .bss and does not grow
//! the 3DSX. Only the reference count is protected by the mutex; socExit
//! still runs when the last handle closes, but the memory is never freed,
//! so repeated syncs cannot fragment the heap with 1 MiB aligned
//! allocations.
use std::cell::UnsafeCell;
use std::io;
use std::sync::Mutex;

const SOC_BUFFER_SIZE: usize = 1024 * 1024;

#[repr(C, align(4096))]
struct SocBuffer(UnsafeCell<[u8; SOC_BUFFER_SIZE]>);
// Only ever handed to socInit while the SOC_SERVICE mutex is held.
unsafe impl Sync for SocBuffer {}

static SOC_BUFFER: SocBuffer = SocBuffer(UnsafeCell::new([0; SOC_BUFFER_SIZE]));

struct SocService {
    refs: usize,
}

static SOC_SERVICE: Mutex<Option<SocService>> = Mutex::new(None);

extern "C" {
    fn socInit(buffer: *mut u32, size: u32) -> i32;
    fn socExit() -> i32;
}

fn lock_service() -> std::sync::MutexGuard<'static, Option<SocService>> {
    SOC_SERVICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct NativeSoc {
    active: bool,
}

// Cloned handles share the single SOC service; the service shuts down when
// the last handle closes instead of failing with "already active".
unsafe impl Send for NativeSoc {}

impl Clone for NativeSoc {
    fn clone(&self) -> Self {
        if self.active {
            if let Some(state) = lock_service().as_mut() {
                state.refs += 1;
            }
        }
        Self {
            active: self.active,
        }
    }
}

impl NativeSoc {
    pub fn new() -> io::Result<Self> {
        let mut guard = lock_service();
        if let Some(state) = guard.as_mut() {
            state.refs += 1;
            return Ok(Self { active: true });
        }
        let ptr = SOC_BUFFER.0.get().cast::<u32>();
        let result = unsafe { socInit(ptr, SOC_BUFFER_SIZE as u32) };
        if result < 0 {
            return Err(io::Error::other(format!("socInit failed: {result}")));
        }
        *guard = Some(SocService { refs: 1 });
        Ok(Self { active: true })
    }

    pub fn close(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let shutdown = {
            let mut guard = lock_service();
            let Some(state) = guard.as_mut() else {
                return Ok(());
            };
            state.refs = state.refs.saturating_sub(1);
            if state.refs > 0 {
                return Ok(());
            }
            guard.take()
        };
        if shutdown.is_some() {
            let result = unsafe { socExit() };
            if result < 0 {
                return Err(io::Error::other(format!("socExit failed: {result}")));
            }
        }
        Ok(())
    }
}

impl Drop for NativeSoc {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            tracing::error!("{error}");
        }
    }
}
