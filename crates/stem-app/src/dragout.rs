//! Dragging files out of the app into a DAW, Explorer or anything else that accepts dropped
//! files.
//!
//! On Windows this starts a native OLE drag (`SHDoDragDrop`) carrying a shell data object for
//! the files, which offers `CF_HDROP` plus the shell's own formats, exactly what Explorer
//! produces when you drag a file. FL Studio, Ableton Live, Reaper, Bitwig, Cubase and
//! Explorer all take it. Elsewhere [`NATIVE`] is false and the caller reveals the file in
//! the file manager instead.
//!
//! Approach (shell item array -> `BHID_DataObject` instead of a hand-written `IDataObject`)
//! follows the `drag` crate by CrabNebula (MIT / Apache-2.0); see THIRD_PARTY.md.

use std::path::PathBuf;

/// True where [`drag_files`] starts a real OS drag.
pub const NATIVE: bool = cfg!(windows);

/// Start dragging `paths` out of the window. Blocks until the user drops or cancels (the OS
/// runs its own message loop meanwhile). Must be called on the UI thread while the left mouse
/// button is still held, i.e. in the frame where egui reports the drag starting.
#[cfg(windows)]
pub fn drag_files(frame: &eframe::Frame, paths: &[PathBuf]) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let hwnd = match frame.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => return Err("no Win32 window handle".into()),
    };
    let paths: Vec<PathBuf> = paths
        .iter()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
        .filter(|p| p.exists())
        .collect();
    if paths.is_empty() {
        return Err("the file isn't there any more".into());
    }
    // SAFETY: plain Win32/COM calls on the UI thread; every pointer passed lives for the call.
    unsafe { win::drag(hwnd, &paths) }.map_err(|e| e.message())
}

#[cfg(not(windows))]
pub fn drag_files(_frame: &eframe::Frame, _paths: &[PathBuf]) -> Result<(), String> {
    Err("dragging files out is only supported on Windows".into())
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;

    use windows::Win32::Foundation::{E_INVALIDARG, HWND};
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, IDropSource, OleInitialize};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, ILCreateFromPathW, ILFree, IShellItemArray,
        SHCreateShellItemArrayFromIDLists, SHDoDragDrop,
    };
    use windows::core::{PCWSTR, Result};

    /// Absolute shell item ids (PIDLs) for some paths, freed on drop.
    struct Pidls(Vec<*mut ITEMIDLIST>);

    impl Drop for Pidls {
        fn drop(&mut self) {
            for &p in &self.0 {
                // SAFETY: each pointer came from ILCreateFromPathW and is freed exactly once.
                unsafe { ILFree(Some(p.cast_const())) };
            }
        }
    }

    pub(super) unsafe fn drag(hwnd: isize, paths: &[PathBuf]) -> Result<()> {
        unsafe {
            // winit already initialised OLE on this thread (drag-and-drop is enabled for the
            // window); a second call just returns S_FALSE. If it really fails, DoDragDrop
            // reports that below.
            let _ = OleInitialize(None);

            let mut pidls = Pidls(Vec::with_capacity(paths.len()));
            for p in paths {
                let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
                let pidl = ILCreateFromPathW(PCWSTR(wide.as_ptr()));
                if !pidl.is_null() {
                    pidls.0.push(pidl);
                }
            }
            if pidls.0.is_empty() {
                return Err(E_INVALIDARG.into());
            }
            let ids: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|p| p.cast_const()).collect();
            // The shell builds the data object (CF_HDROP, shell IDList array, file contents
            // formats...) for us, same as dragging the files in Explorer.
            let items: IShellItemArray = SHCreateShellItemArrayFromIDLists(&ids)?;
            let data: IDataObject = items.BindToHandler(None, &BHID_DataObject)?;
            // No drop source: SHDoDragDrop then uses the shell's default one (left button
            // release drops, Esc cancels) and shows the standard drag image. Copy only, so a
            // target can never move (delete) our stem files.
            SHDoDragDrop(
                Some(HWND(hwnd as *mut _)),
                &data,
                None::<&IDropSource>,
                DROPEFFECT_COPY,
            )?;
            Ok(())
        }
    }
}
