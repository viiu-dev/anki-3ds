// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Explicit ownership of libctru's caller-allocated SOC buffer.
//!
//! libctru socExit() closes shared-memory/service handles but does not free the
//! caller's buffer. Keep the allocation alive until shutdown completes.
use std::alloc::alloc_zeroed;
use std::alloc::dealloc;
use std::alloc::Layout;
use std::io;
use std::ptr::NonNull;
use std::sync::Mutex;

struct SocService {
    buffer: NonNull<u8>,
    layout: Layout,
    refs: usize,
}

// The buffer is only handed to socInit/socExit; the service is shared by
// reference-counted handles, never aliased mutably.
unsafe impl Send for SocService {}

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
        let layout = Layout::from_size_align(1024 * 1024, 4096).unwrap();
        let buffer = NonNull::new(unsafe { alloc_zeroed(layout) })
            .ok_or_else(|| io::Error::other("SOC buffer allocation failed"))?;
        let result = unsafe { socInit(buffer.as_ptr().cast(), layout.size() as u32) };
        if result < 0 {
            unsafe { dealloc(buffer.as_ptr(), layout) };
            return Err(io::Error::other(format!("socInit failed: {result}")));
        }
        *guard = Some(SocService {
            buffer,
            layout,
            refs: 1,
        });
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
        if let Some(state) = shutdown {
            let result = unsafe { socExit() };
            unsafe { dealloc(state.buffer.as_ptr(), state.layout) };
            if result < 0 {
                return Err(io::Error::other(format!(
                    "socExit failed; buffer released: {result}"
                )));
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
