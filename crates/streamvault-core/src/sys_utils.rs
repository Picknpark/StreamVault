use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NetSpeedSnapshot {
    pub down_bps: u64,
    pub up_bps: u64,
}

/// Writes UTF-8 `text` directly to the native Windows OS clipboard (`CF_UNICODETEXT`).
/// Works reliably even when a WebView2 tab is on an insecure `http://` LAN origin
/// (where `navigator.clipboard` is `undefined`) and preserves percent-encoded URLs verbatim.
pub fn write_system_clipboard_text(text: &str) -> Result<()> {
    #[cfg(windows)]
    {
        use anyhow::anyhow;
        use std::ptr;
        use std::time::Duration;
        use windows_sys::Win32::Foundation::GlobalFree;
        use windows_sys::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows_sys::Win32::System::Memory::{
            GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
        };

        const CF_UNICODETEXT: u32 = 13;

        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let byte_len = utf16
            .len()
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or_else(|| anyhow!("Clipboard text size overflow"))?;

        unsafe {
            let mut opened = false;
            for attempt in 0..6 {
                if OpenClipboard(ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                if attempt < 5 {
                    std::thread::sleep(Duration::from_millis(15));
                }
            }
            if !opened {
                return Err(anyhow!("Windows clipboard is temporarily busy"));
            }

            let result = (|| -> Result<()> {
                if EmptyClipboard() == 0 {
                    return Err(anyhow!("Failed to clear Windows clipboard"));
                }

                let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
                if hmem.is_null() {
                    return Err(anyhow!("GlobalAlloc failed for clipboard buffer"));
                }

                let dst = GlobalLock(hmem) as *mut u16;
                if dst.is_null() {
                    let _ = GlobalFree(hmem);
                    return Err(anyhow!("GlobalLock failed for clipboard buffer"));
                }

                ptr::copy_nonoverlapping(utf16.as_ptr(), dst, utf16.len());
                let _ = GlobalUnlock(hmem);

                if SetClipboardData(CF_UNICODETEXT, hmem).is_null() {
                    let _ = GlobalFree(hmem);
                    return Err(anyhow!("SetClipboardData(CF_UNICODETEXT) failed"));
                }

                // Ownership of `hmem` has transferred to the Windows clipboard!
                Ok(())
            })();

            let _ = CloseClipboard();
            result
        }
    }

    #[cfg(not(windows))]
    {
        let _ = text;
        Ok(())
    }
}

/// Lightweight, allocation-minimal sampler of OS network adapter byte counters.
/// Uses `GetIfTable2` (`iphlpapi.dll`) on Windows 10/11 and `/proc/net/dev` on Linux.
pub struct NetSpeedSampler {
    prev_rx: u64,
    prev_tx: u64,
    prev_instant: Instant,
    initialized: bool,
}

impl Default for NetSpeedSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl NetSpeedSampler {
    pub fn new() -> Self {
        let mut s = Self {
            prev_rx: 0,
            prev_tx: 0,
            prev_instant: Instant::now(),
            initialized: false,
        };
        let _ = s.sample();
        s
    }

    pub fn sample(&mut self) -> NetSpeedSnapshot {
        let now = Instant::now();
        let Some((rx, tx)) = read_os_net_octets() else {
            return NetSpeedSnapshot {
                down_bps: 0,
                up_bps: 0,
            };
        };

        if !self.initialized {
            self.prev_rx = rx;
            self.prev_tx = tx;
            self.prev_instant = now;
            self.initialized = true;
            return NetSpeedSnapshot {
                down_bps: 0,
                up_bps: 0,
            };
        }

        let dt = now.duration_since(self.prev_instant).as_secs_f64();
        if dt < 0.05 {
            return NetSpeedSnapshot {
                down_bps: 0,
                up_bps: 0,
            };
        }

        let delta_rx = rx.saturating_sub(self.prev_rx);
        let delta_tx = tx.saturating_sub(self.prev_tx);

        self.prev_rx = rx;
        self.prev_tx = tx;
        self.prev_instant = now;

        NetSpeedSnapshot {
            down_bps: ((delta_rx as f64) / dt).round() as u64,
            up_bps: ((delta_tx as f64) / dt).round() as u64,
        }
    }
}

#[cfg(windows)]
fn read_os_net_octets() -> Option<(u64, u64)> {
    use std::ptr;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2, MIB_IF_TYPE_LOOPBACK,
    };

    const IF_OPER_STATUS_UP: i32 = 1;
    const IF_TYPE_TUNNEL: u32 = 131;
    // Bit 2 (0x04) in MIB_IF_ROW2.InterfaceAndOperStatusFlags._bitfield is `FilterInterface`
    // (NDIS lightweight filter driver duplicate layer); skip it so NIC bytes aren't double-counted.
    const FILTER_INTERFACE_FLAG: u8 = 0x04;

    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = ptr::null_mut();
        let status = GetIfTable2(&mut table);
        if status != 0 || table.is_null() {
            return None;
        }

        let count = (*table).NumEntries as usize;
        let rows_ptr = (*table).Table.as_ptr();
        let rows: &[MIB_IF_ROW2] = std::slice::from_raw_parts(rows_ptr, count);

        let mut total_rx: u64 = 0;
        let mut total_tx: u64 = 0;

        for row in rows {
            if row.Type == MIB_IF_TYPE_LOOPBACK || row.Type == IF_TYPE_TUNNEL {
                continue;
            }
            if row.OperStatus != IF_OPER_STATUS_UP {
                continue;
            }
            if (row.InterfaceAndOperStatusFlags._bitfield & FILTER_INTERFACE_FLAG) != 0 {
                continue;
            }
            total_rx = total_rx.saturating_add(row.InOctets);
            total_tx = total_tx.saturating_add(row.OutOctets);
        }

        FreeMibTable(table as *const core::ffi::c_void);
        Some((total_rx, total_tx))
    }
}

#[cfg(target_os = "linux")]
fn read_os_net_octets() -> Option<(u64, u64)> {
    let content = std::fs::read_to_string("/proc/net/dev").ok()?;
    let mut total_rx: u64 = 0;
    let mut total_tx: u64 = 0;
    for line in content.lines().skip(2) {
        let Some((iface, stats)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" {
            continue;
        }
        let cols: Vec<&str> = stats.split_whitespace().collect();
        if cols.len() >= 9 {
            let rx = cols[0].parse::<u64>().unwrap_or(0);
            let tx = cols[8].parse::<u64>().unwrap_or(0);
            total_rx = total_rx.saturating_add(rx);
            total_tx = total_tx.saturating_add(tx);
        }
    }
    Some((total_rx, total_tx))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn read_os_net_octets() -> Option<(u64, u64)> {
    Some((0, 0))
}
