//! NVIDIA GPU readings: NVML (initialized once, cached), falling back to `nvidia-smi`.
//! On platforms without NVIDIA support (macOS) every reader returns None / empty.
//!
//! All functions block (NVML calls take microseconds, `nvidia-smi` ~100 ms), so async callers
//! should wrap them in `spawn_blocking`.

use super::{GpuProcess, GpuStats};

/// Current stats of the first GPU, or None without an NVIDIA GPU.
pub fn read() -> Option<GpuStats> {
    imp::read()
}

/// Processes holding GPU memory, largest first.
pub fn processes() -> Vec<GpuProcess> {
    let mut v = imp::processes();
    v.sort_by_key(|p| std::cmp::Reverse(p.vram_bytes.unwrap_or(0)));
    v
}

/// VRAM used by one process, if the driver reports it (WDDM on Windows often does not).
pub fn process_vram(pid: u32) -> Option<u64> {
    imp::processes().into_iter().find(|p| p.pid == pid).and_then(|p| p.vram_bytes)
}

/// Parse `nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,
/// temperature.gpu,power.draw,clocks.sm --format=csv,noheader,nounits` (first GPU).
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn parse_smi_gpu(text: &str) -> Option<GpuStats> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let f: Vec<&str> = line.split(',').map(str::trim).collect();
    if f.len() < 4 {
        return None;
    }
    let num = |i: usize| f.get(i).and_then(|s| s.parse::<f64>().ok());
    const MIB: f64 = 1024.0 * 1024.0;
    Some(GpuStats {
        name: f[0].to_string(),
        util_pct: num(1).unwrap_or(0.0) as f32,
        vram_used: (num(2)? * MIB) as u64,
        vram_total: (num(3)? * MIB) as u64,
        temp_c: num(4).map(|v| v as f32),
        power_w: num(5).map(|v| v as f32),
        clock_mhz: num(6).map(|v| v as u32),
        source: "nvidia-smi".into(),
    })
}

/// Parse `nvidia-smi --query-compute-apps=pid,process_name,used_memory
/// --format=csv,noheader,nounits`.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn parse_smi_apps(text: &str) -> Vec<GpuProcess> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, ',').map(str::trim);
            let pid = it.next()?.parse::<u32>().ok()?;
            let path = it.next().unwrap_or("");
            let mem = it.next().and_then(|m| m.parse::<f64>().ok());
            let name = path.rsplit(['\\', '/']).next().unwrap_or(path).to_string();
            Some(GpuProcess { pid, name, vram_bytes: mem.map(|m| (m * 1024.0 * 1024.0) as u64) })
        })
        .collect()
}

#[cfg(any(windows, target_os = "linux"))]
mod imp {
    use super::{parse_smi_apps, parse_smi_gpu, GpuProcess, GpuStats};
    use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
    use nvml_wrapper::enums::device::UsedGpuMemory;
    use nvml_wrapper::Nvml;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::process::Command;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    static NVML: OnceLock<Option<Nvml>> = OnceLock::new();
    /// When nvidia-smi last failed; we don't retry it more than once a minute.
    static SMI_FAILED: Mutex<Option<Instant>> = Mutex::new(None);
    const SMI_RETRY: Duration = Duration::from_secs(60);

    fn nvml() -> Option<&'static Nvml> {
        NVML.get_or_init(|| match Nvml::init() {
            Ok(n) => Some(n),
            Err(e) => {
                log::info!("NVML unavailable ({e}); falling back to nvidia-smi");
                None
            }
        })
        .as_ref()
    }

    fn smi(args: &[&str]) -> Option<String> {
        if let Some(t) = *SMI_FAILED.lock() {
            if t.elapsed() < SMI_RETRY {
                return None;
            }
        }
        let mut cmd = Command::new("nvidia-smi");
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match cmd.output() {
            Ok(o) if o.status.success() => {
                *SMI_FAILED.lock() = None;
                Some(String::from_utf8_lossy(&o.stdout).into_owned())
            }
            _ => {
                *SMI_FAILED.lock() = Some(Instant::now());
                None
            }
        }
    }

    fn read_nvml(n: &Nvml) -> Option<GpuStats> {
        let d = n.device_by_index(0).ok()?;
        let mem = d.memory_info().ok()?;
        Some(GpuStats {
            name: d.name().unwrap_or_else(|_| "NVIDIA GPU".into()),
            util_pct: d.utilization_rates().map(|u| u.gpu as f32).unwrap_or(0.0),
            vram_used: mem.used,
            vram_total: mem.total,
            temp_c: d.temperature(TemperatureSensor::Gpu).ok().map(|t| t as f32),
            power_w: d.power_usage().ok().map(|mw| mw as f32 / 1000.0),
            clock_mhz: d.clock_info(Clock::SM).ok(),
            source: "nvml".into(),
        })
    }

    pub fn read() -> Option<GpuStats> {
        if let Some(s) = nvml().and_then(read_nvml) {
            return Some(s);
        }
        let out = smi(&[
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,clocks.sm",
            "--format=csv,noheader,nounits",
        ])?;
        parse_smi_gpu(&out)
    }

    fn processes_nvml(n: &Nvml) -> Option<Vec<GpuProcess>> {
        let count = n.device_count().ok()?;
        let mut by_pid: HashMap<u32, Option<u64>> = HashMap::new();
        let mut any_ok = false;
        for i in 0..count {
            let Ok(d) = n.device_by_index(i) else { continue };
            for list in [d.running_compute_processes(), d.running_graphics_processes()] {
                let Ok(list) = list else { continue };
                any_ok = true;
                for p in list {
                    let used = match p.used_gpu_memory {
                        UsedGpuMemory::Used(b) => Some(b),
                        UsedGpuMemory::Unavailable => None,
                    };
                    let e = by_pid.entry(p.pid).or_insert(None);
                    *e = match (*e, used) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                }
            }
        }
        if !any_ok {
            return None;
        }
        let pids: Vec<sysinfo::Pid> = by_pid.keys().map(|p| sysinfo::Pid::from_u32(*p)).collect();
        let mut sys = sysinfo::System::new();
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&pids),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        Some(
            by_pid
                .into_iter()
                .map(|(pid, vram_bytes)| GpuProcess {
                    pid,
                    name: sys
                        .process(sysinfo::Pid::from_u32(pid))
                        .map(|p| p.name().to_string_lossy().into_owned())
                        .unwrap_or_else(|| format!("pid {pid}")),
                    vram_bytes,
                })
                .collect(),
        )
    }

    pub fn processes() -> Vec<GpuProcess> {
        if let Some(v) = nvml().and_then(processes_nvml) {
            return v;
        }
        smi(&["--query-compute-apps=pid,process_name,used_memory", "--format=csv,noheader,nounits"])
            .map(|o| parse_smi_apps(&o))
            .unwrap_or_default()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::{GpuProcess, GpuStats};
    pub fn read() -> Option<GpuStats> {
        None
    }
    pub fn processes() -> Vec<GpuProcess> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_smi_gpu_line() {
        let s = parse_smi_gpu("NVIDIA GeForce RTX 5090 Laptop GPU, 37, 20480, 24463, 61, 95.12, 2100\n").unwrap();
        assert_eq!(s.name, "NVIDIA GeForce RTX 5090 Laptop GPU");
        assert_eq!(s.util_pct, 37.0);
        assert_eq!(s.vram_used, 20480 * 1024 * 1024);
        assert_eq!(s.vram_total, 24463 * 1024 * 1024);
        assert_eq!(s.temp_c, Some(61.0));
        assert_eq!(s.power_w, Some(95.12));
        assert_eq!(s.clock_mhz, Some(2100));
        assert_eq!(s.source, "nvidia-smi");
        // Laptop GPUs often report [N/A] for power.
        let s = parse_smi_gpu("GPU, 0, 100, 8192, 40, [N/A], 300").unwrap();
        assert_eq!(s.power_w, None);
        assert!(parse_smi_gpu("").is_none());
    }

    #[test]
    fn parses_smi_apps() {
        let v = parse_smi_apps(
            "1234, C:\\AI\\llama.cpp\\b6000\\llama-server.exe, 20000\n5678, C:\\Program Files\\x.exe, [N/A]\n",
        );
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "llama-server.exe");
        assert_eq!(v[0].vram_bytes, Some(20000 * 1024 * 1024));
        assert_eq!(v[1].vram_bytes, None);
    }
}
