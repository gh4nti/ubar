#[cfg_attr(target_os = "windows", allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PressureAction {
    None,
    TrimCaches,
    FreezeBackground,
    SwapBackground,
    SwapAllHidden,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryPolicy {
    pub physical_bytes: u64,
    pub preferred_resident_bytes: u64,
    pub elastic_ceiling_bytes: u64,
    pub constrained: bool,
    pub renderer_limit: u8,
}

impl MemoryPolicy {
    pub fn detect() -> Self {
        Self::for_physical_memory(total_physical_bytes())
    }

    pub fn for_physical_memory(physical_bytes: u64) -> Self {
        const MIB: u64 = 1024 * 1024;
        const GIB: u64 = 1024 * MIB;
        let constrained = physical_bytes <= GIB;
        if constrained {
            return Self {
                physical_bytes,
                preferred_resident_bytes: 280 * MIB,
                elastic_ceiling_bytes: 320 * MIB,
                constrained: true,
                renderer_limit: 1,
            };
        }
        Self {
            physical_bytes,
            preferred_resident_bytes: GIB,
            elastic_ceiling_bytes: physical_bytes / 4,
            constrained: false,
            renderer_limit: 4,
        }
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub fn action_for_available_percent(self, available_percent: u8) -> PressureAction {
        match available_percent {
            0..=9 => PressureAction::SwapAllHidden,
            10..=14 => PressureAction::SwapBackground,
            15..=19 => PressureAction::FreezeBackground,
            20..=24 => PressureAction::TrimCaches,
            _ => PressureAction::None,
        }
    }
}

fn total_physical_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("MemTotal:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
            })
            .map(|kib| kib * 1024)
            .unwrap_or(4 * 1024 * 1024 * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(4 * 1024 * 1024 * 1024)
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() {
            status.ullTotalPhys
        } else {
            4 * 1024 * 1024 * 1024
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_policy_uses_quarter_of_ram() {
        let policy = MemoryPolicy::for_physical_memory(16 * 1024 * 1024 * 1024);
        assert_eq!(policy.elastic_ceiling_bytes, 4 * 1024 * 1024 * 1024);
        assert!(!policy.constrained);
        assert_eq!(policy.renderer_limit, 4);
    }

    #[test]
    fn tiny_machine_gets_single_renderer_exception() {
        let policy = MemoryPolicy::for_physical_memory(512 * 1024 * 1024);
        assert!(policy.constrained);
        assert_eq!(policy.renderer_limit, 1);
        assert_eq!(policy.preferred_resident_bytes, 280 * 1024 * 1024);
        assert_eq!(
            policy.action_for_available_percent(8),
            PressureAction::SwapAllHidden
        );
    }
}
