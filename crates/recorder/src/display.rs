/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! DXGI adapter / output enumeration, used to map a region to a `ddagrab` output and to
//! fingerprint the GPU. Indices match what FFmpeg's D3D11 device and `ddagrab` use.

use framelet_core::Region;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1};
use windows::core::Interface;

pub const NVIDIA_VENDOR_ID: u32 = 0x10DE;

pub struct Adapter {
    pub index: u32,
    pub vendor_id: u32,
    pub description: String,
    /// User-mode driver version, e.g. "32.0.15.6094".
    pub driver: Option<String>,
    /// Desktop rectangle of every output attached to the desktop, with its output index.
    pub outputs: Vec<(u32, Region)>,
}

/// A monitor that fully contains a capture region.
pub struct OutputMatch {
    pub adapter: u32,
    pub output: u32,
    pub nvidia: bool,
    /// Region origin relative to the output.
    pub x: i32,
    pub y: i32,
}

/// Enumerates all adapters. Returns an empty list if DXGI is unavailable.
pub fn adapters() -> Vec<Adapter> {
    // SAFETY: plain COM calls on interfaces we own.
    unsafe {
        let factory: IDXGIFactory1 = match CreateDXGIFactory1() {
            Ok(f) => f,
            Err(e) => {
                log::warn!("CreateDXGIFactory1 failed: {e}");
                return Vec::new();
            }
        };
        let mut result = Vec::new();
        let mut index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            let Ok(desc) = adapter.GetDesc1() else {
                index += 1;
                continue;
            };
            let name_len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let driver = adapter.CheckInterfaceSupport(&IDXGIDevice::IID).ok().map(|v| {
                format!("{}.{}.{}.{}", (v >> 48) & 0xFFFF, (v >> 32) & 0xFFFF, (v >> 16) & 0xFFFF, v & 0xFFFF)
            });

            let mut outputs = Vec::new();
            let mut out_index = 0;
            while let Ok(output) = adapter.EnumOutputs(out_index) {
                if let Ok(od) = output.GetDesc() {
                    let r = od.DesktopCoordinates;
                    if od.AttachedToDesktop.as_bool() {
                        let rect = Region {
                            x: r.left,
                            y: r.top,
                            width: (r.right - r.left).max(0) as u32,
                            height: (r.bottom - r.top).max(0) as u32,
                        };
                        outputs.push((out_index, rect));
                    }
                }
                out_index += 1;
            }

            result.push(Adapter {
                index,
                vendor_id: desc.VendorId,
                description: String::from_utf16_lossy(&desc.Description[..name_len]),
                driver,
                outputs,
            });
            index += 1;
        }
        result
    }
}

/// First NVIDIA adapter, if any.
pub fn nvidia_adapter() -> Option<Adapter> {
    adapters().into_iter().find(|a| a.vendor_id == NVIDIA_VENDOR_ID)
}

/// Finds the single output that fully contains `region`. `None` if the region spans outputs.
pub fn find_output(region: &Region) -> Option<OutputMatch> {
    adapters().into_iter().find_map(|adapter| {
        let nvidia = adapter.vendor_id == NVIDIA_VENDOR_ID;
        adapter.outputs.iter().find(|(_, rect)| region.is_inside(rect)).map(|(output, rect)| OutputMatch {
            adapter: adapter.index,
            output: *output,
            nvidia,
            x: region.x - rect.x,
            y: region.y - rect.y,
        })
    })
}
