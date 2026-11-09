//! Keep Windows from going to sleep while a split runs.
//!
//! A long GPU split (minutes for the RoFormer models) froze mid-run when the laptop idled
//! into standby. While a [`KeepAwake`] lives, the thread that made it asks Windows to stay
//! out of idle sleep (the display may still turn off). Closing the lid follows the power
//! plan's lid action, which no app can override.

/// Holds the system awake until dropped. Must be dropped on the thread that made it.
pub struct KeepAwake {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl KeepAwake {
    pub fn new() -> Self {
        #[cfg(windows)]
        // SAFETY: plain Win32 call with flag constants; it only affects this thread's state.
        unsafe {
            use windows::Win32::System::Power::{
                ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
            };
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        }
        Self {
            _not_send: std::marker::PhantomData,
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: as above; ES_CONTINUOUS alone clears this thread's request.
        unsafe {
            use windows::Win32::System::Power::{ES_CONTINUOUS, SetThreadExecutionState};
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}
