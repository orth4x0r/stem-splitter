//! Compute devices: CPU everywhere, DirectML adapters (enumerated through DXGI) on Windows.

use crate::Device;

/// A hardware GPU adapter as DXGI reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuAdapter {
    /// DXGI `EnumAdapters1` index; the value to put in [`Device::DirectMl`].
    pub index: u32,
    pub name: String,
    /// Dedicated video memory in bytes (an iGPU reports only its small carve-out).
    pub dedicated_vram: u64,
}

/// Hardware GPU adapters usable by DirectML, in DXGI order. Software adapters ("Microsoft
/// Basic Render Driver") are skipped. Always empty off Windows.
pub fn gpu_adapters() -> Vec<GpuAdapter> {
    #[cfg(windows)]
    {
        dxgi::adapters()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Devices the user can pick from, with display names (e.g. "AMD Radeon RX 6700S (DirectML)").
/// Always includes [`Device::Cpu`]; DirectML adapters are listed on Windows only, the one
/// [`Device::Auto`] would choose first.
pub fn list_devices() -> Vec<(Device, String)> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut out = vec![(Device::Cpu, format!("CPU ({threads} threads)"))];
    let mut gpus = gpu_adapters();
    gpus.sort_by_key(|a| std::cmp::Reverse(a.dedicated_vram));
    for a in gpus {
        out.push((
            Device::DirectMl(a.index),
            format!("{} ({}, DirectML)", a.name, format_vram(a.dedicated_vram)),
        ));
    }
    out
}

fn format_vram(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let g = bytes as f64 / GIB;
    if g >= 1.0 {
        format!("{g:.0} GB")
    } else {
        format!("{} MB", bytes / (1024 * 1024))
    }
}

/// The adapter [`Device::Auto`] uses: the one with the most dedicated VRAM, which on a laptop
/// is the discrete GPU rather than the iGPU.
pub(crate) fn best_adapter(adapters: &[GpuAdapter]) -> Option<&GpuAdapter> {
    // `max_by_key` returns the last maximum; prefer the first (lower index) on ties.
    adapters.iter().rev().max_by_key(|a| a.dedicated_vram)
}

/// What a [`Device`] means on this machine right now: `Auto` becomes a concrete choice.
pub(crate) fn resolve(device: Device) -> Device {
    match device {
        Device::Cpu => Device::Cpu,
        Device::DirectMl(i) => {
            if cfg!(windows) {
                Device::DirectMl(i)
            } else {
                tracing::warn!("DirectML is only available on Windows; using the CPU");
                Device::Cpu
            }
        }
        Device::Auto => match best_adapter(&gpu_adapters()) {
            Some(a) => {
                tracing::info!(
                    "auto device: {} (adapter {}, {} dedicated)",
                    a.name,
                    a.index,
                    format_vram(a.dedicated_vram)
                );
                Device::DirectMl(a.index)
            }
            None => Device::Cpu,
        },
    }
}

#[cfg(windows)]
mod dxgi {
    use super::GpuAdapter;
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };

    pub(super) fn adapters() -> Vec<GpuAdapter> {
        let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("CreateDXGIFactory1 failed: {e}");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for index in 0u32.. {
            // Fails with DXGI_ERROR_NOT_FOUND past the last adapter.
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
                break;
            };
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
                continue;
            };
            let len = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..len])
                .trim()
                .to_string();
            let software = desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0
                || name.contains("Microsoft Basic Render Driver");
            if software {
                continue;
            }
            out.push(GpuAdapter {
                index,
                name,
                dedicated_vram: desc.DedicatedVideoMemory as u64,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(index: u32, name: &str, gib: u64) -> GpuAdapter {
        GpuAdapter {
            index,
            name: name.into(),
            dedicated_vram: gib << 30,
        }
    }

    #[test]
    fn auto_prefers_most_vram() {
        // Windows often lists the iGPU first on hybrid laptops.
        let list = [
            a(0, "AMD Radeon(TM) Graphics", 0),
            a(1, "AMD Radeon RX 6700S", 8),
        ];
        assert_eq!(best_adapter(&list).unwrap().index, 1);
        let tie = [a(0, "x", 4), a(1, "y", 4)];
        assert_eq!(best_adapter(&tie).unwrap().index, 0);
        assert!(best_adapter(&[]).is_none());
    }

    #[test]
    fn cpu_always_listed() {
        let d = list_devices();
        assert_eq!(d[0].0, Device::Cpu);
        assert_eq!(resolve(Device::Cpu), Device::Cpu);
        if !cfg!(windows) {
            assert_eq!(resolve(Device::Auto), Device::Cpu);
            assert_eq!(resolve(Device::DirectMl(0)), Device::Cpu);
        }
    }
}
