//! CPU topology as the operating system reports it: which logical processors
//! share a physical core (simultaneous multithreading), and which cores belong
//! to the faster class on a hybrid processor.
//!
//! Processors differ in both respects and in every combination: uniform cores
//! with or without SMT, hybrid performance/efficiency designs with SMT on only
//! the performance cores, or with none at all. A thread count derived from a
//! single total ("all physical cores", "logical / 2") is right for one shape
//! and wrong for the others, so the topology is read, never assumed. Nothing
//! here names a processor model.
//!
//! The numbering of logical processors is the operating system's. On one
//! hybrid laptop measured during the 2026-09-16 audit the performance cores
//! were logical 0, 1, 10-13, 22 and 23, interleaved with efficiency cores, so
//! "the first N processors" is not a class boundary either.

use serde::{Deserialize, Serialize};

/// One logical processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalCpu {
    /// Index the OS uses for affinity masks.
    pub index: u32,
    /// Physical core it belongs to; SMT siblings share this.
    pub core: u32,
    /// Higher is faster. Uniform processors report a single class.
    pub efficiency_class: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuTopology {
    pub logical: Vec<LogicalCpu>,
    /// Where the facts came from, for the runtime notes.
    pub source: String,
}

impl CpuTopology {
    pub fn logical_count(&self) -> usize {
        self.logical.len()
    }

    pub fn physical_count(&self) -> usize {
        let mut cores: Vec<u32> = self.logical.iter().map(|cpu| cpu.core).collect();
        cores.sort_unstable();
        cores.dedup();
        cores.len()
    }

    pub fn is_hybrid(&self) -> bool {
        let first = self.logical.first().map(|cpu| cpu.efficiency_class);
        self.logical.iter().any(|cpu| Some(cpu.efficiency_class) != first)
    }

    pub fn has_smt(&self) -> bool {
        self.logical_count() > self.physical_count()
    }

    fn top_class(&self) -> u8 {
        self.logical.iter().map(|cpu| cpu.efficiency_class).max().unwrap_or(0)
    }

    /// Physical cores of the fastest class (all cores on a uniform processor).
    pub fn performance_cores(&self) -> usize {
        let top = self.top_class();
        let mut cores: Vec<u32> = self
            .logical
            .iter()
            .filter(|cpu| cpu.efficiency_class == top)
            .map(|cpu| cpu.core)
            .collect();
        cores.sort_unstable();
        cores.dedup();
        cores.len()
    }

    /// One logical processor per fastest-class physical core: the set a
    /// compute thread per core should be pinned to, SMT siblings excluded.
    pub fn performance_primary_cpus(&self) -> Vec<u32> {
        let top = self.top_class();
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<u32> = self
            .logical
            .iter()
            .filter(|cpu| cpu.efficiency_class == top && seen.insert(cpu.core))
            .map(|cpu| cpu.index)
            .collect();
        out.sort_unstable();
        out
    }

    /// llama.cpp's --cpu-mask format for a set of logical processors: a hex
    /// bitmask, lowest processor in the lowest bit.
    pub fn mask_hex(cpus: &[u32]) -> String {
        let highest = cpus.iter().copied().max().unwrap_or(0) as usize;
        let mut nibbles = vec![0u8; highest / 4 + 1];
        for &cpu in cpus {
            nibbles[cpu as usize / 4] |= 1 << (cpu % 4);
        }
        let digits: String = nibbles
            .iter()
            .rev()
            .map(|nibble| char::from_digit(u32::from(*nibble), 16).unwrap_or('0'))
            .collect();
        let trimmed = digits.trim_start_matches('0');
        format!("0x{}", if trimmed.is_empty() { "0" } else { trimmed })
    }

    /// Plain summary for runtime notes.
    pub fn describe(&self) -> String {
        let shape = if self.is_hybrid() {
            format!(
                "hybrid: {} fastest-class of {} physical cores",
                self.performance_cores(),
                self.physical_count()
            )
        } else {
            format!("{} physical cores", self.physical_count())
        };
        format!(
            "{} logical processors, {shape}, {}",
            self.logical_count(),
            if self.has_smt() { "with SMT" } else { "no SMT" }
        )
    }
}

/// Read the topology, or None when the platform does not report it.
pub fn detect() -> Option<CpuTopology> {
    platform::detect().filter(|topology| !topology.logical.is_empty())
}

#[cfg(windows)]
mod platform {
    use super::{CpuTopology, LogicalCpu};

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemCpuSetInformation(
            information: *mut u8,
            buffer_length: u32,
            returned_length: *mut u32,
            process: isize,
            flags: u32,
        ) -> i32;
    }

    /// SYSTEM_CPU_SET_INFORMATION (winnt.h): Size u32, Type u32, then for
    /// CpuSetInformation: Id u32, Group u16, LogicalProcessorIndex u8,
    /// CoreIndex u8, LastLevelCacheIndex u8, NumaNodeIndex u8,
    /// EfficiencyClass u8, flags.
    pub fn detect() -> Option<CpuTopology> {
        let mut needed = 0u32;
        // SAFETY: a null buffer of length 0 only reports the required size.
        unsafe { GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut needed, 0, 0) };
        if needed == 0 {
            return None;
        }
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: the buffer is exactly the length the call asked for.
        let ok = unsafe {
            GetSystemCpuSetInformation(buffer.as_mut_ptr(), needed, &mut needed, 0, 0)
        };
        if ok == 0 {
            return None;
        }
        parse(&buffer[..needed as usize])
    }

    pub(super) fn parse(buffer: &[u8]) -> Option<CpuTopology> {
        let u32_at = |at: usize| -> Option<u32> {
            buffer.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let u16_at = |at: usize| -> Option<u16> {
            buffer.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
        };
        let mut logical = Vec::new();
        let mut offset = 0usize;
        while offset + 20 <= buffer.len() {
            let size = u32_at(offset)? as usize;
            if size < 20 {
                return None;
            }
            if u32_at(offset + 4)? == 0 {
                let group = u32::from(u16_at(offset + 12)?);
                let index = u32::from(*buffer.get(offset + 14)?);
                let core = u32::from(*buffer.get(offset + 15)?);
                let efficiency_class = *buffer.get(offset + 18)?;
                // Processor groups beyond the first extend the index space.
                logical.push(LogicalCpu {
                    index: group * 64 + index,
                    core: group << 16 | core,
                    efficiency_class,
                });
            }
            offset += size;
        }
        logical.sort_by_key(|cpu| cpu.index);
        Some(CpuTopology {
            logical,
            source: "Windows CPU sets".into(),
        })
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{CpuTopology, LogicalCpu};

    pub fn detect() -> Option<CpuTopology> {
        let root = std::path::Path::new("/sys/devices/system/cpu");
        let read = |path: std::path::PathBuf| std::fs::read_to_string(path).ok();
        // Hybrid Intel lists the performance cores under cpu_core.
        let performance = read(root.join("types/cpu_core/cpulist"))
            .or_else(|| read(std::path::PathBuf::from("/sys/devices/cpu_core/cpus")))
            .map(|list| parse_list(&list));
        let mut logical = Vec::new();
        for entry in std::fs::read_dir(root).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(index) = name.strip_prefix("cpu").and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            let topology = entry.path().join("topology");
            let core: u32 = read(topology.join("core_id"))?.trim().parse().ok()?;
            let package: u32 = read(topology.join("physical_package_id"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            // ARM big.LITTLE reports relative capacity instead.
            let capacity: Option<u32> =
                read(entry.path().join("cpu_capacity")).and_then(|v| v.trim().parse().ok());
            let efficiency_class = match (&performance, capacity) {
                (Some(list), _) => u8::from(list.contains(&index)),
                (None, Some(capacity)) => (capacity / 256).min(255) as u8,
                (None, None) => 0,
            };
            logical.push(LogicalCpu {
                index,
                core: package << 16 | core,
                efficiency_class,
            });
        }
        logical.sort_by_key(|cpu| cpu.index);
        Some(CpuTopology {
            logical,
            source: "Linux sysfs".into(),
        })
    }

    fn parse_list(list: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for part in list.trim().split(',').filter(|p| !p.is_empty()) {
            match part.split_once('-') {
                Some((a, b)) => {
                    if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                        out.extend(a..=b);
                    }
                }
                None => {
                    if let Ok(v) = part.parse() {
                        out.push(v);
                    }
                }
            }
        }
        out
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    pub fn detect() -> Option<super::CpuTopology> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(index: u32, core: u32, efficiency_class: u8) -> LogicalCpu {
        LogicalCpu { index, core, efficiency_class }
    }

    #[test]
    fn hybrid_without_smt_and_interleaved_numbering() {
        // The layout read from a hybrid laptop: performance cores at 0, 1,
        // 10-13, 22, 23; no SMT anywhere.
        let fast = [0, 1, 10, 11, 12, 13, 22, 23];
        let topology = CpuTopology {
            logical: (0..24).map(|i| cpu(i, i, u8::from(fast.contains(&i)))).collect(),
            source: "test".into(),
        };
        assert!(topology.is_hybrid());
        assert!(!topology.has_smt());
        assert_eq!(topology.physical_count(), 24);
        assert_eq!(topology.performance_cores(), 8);
        assert_eq!(topology.performance_primary_cpus(), fast.to_vec());
        assert_eq!(CpuTopology::mask_hex(&fast), "0xc03c03");
    }

    #[test]
    fn uniform_with_smt_pins_one_thread_per_core() {
        // 8 cores x 2 threads, siblings numbered next to each other.
        let topology = CpuTopology {
            logical: (0..16).map(|i| cpu(i, i / 2, 0)).collect(),
            source: "test".into(),
        };
        assert!(!topology.is_hybrid());
        assert!(topology.has_smt());
        assert_eq!(topology.physical_count(), 8);
        assert_eq!(topology.performance_cores(), 8);
        assert_eq!(topology.performance_primary_cpus(), vec![0, 2, 4, 6, 8, 10, 12, 14]);
    }

    #[test]
    fn hybrid_with_smt_only_on_performance_cores() {
        // 6 performance cores with SMT (12 logical) plus 8 efficiency cores.
        let mut logical: Vec<LogicalCpu> = (0..12).map(|i| cpu(i, i / 2, 1)).collect();
        logical.extend((12..20).map(|i| cpu(i, i - 6, 0)));
        let topology = CpuTopology { logical, source: "test".into() };
        assert!(topology.is_hybrid() && topology.has_smt());
        assert_eq!(topology.physical_count(), 14);
        assert_eq!(topology.performance_cores(), 6);
        assert_eq!(topology.performance_primary_cpus().len(), 6);
    }

    #[cfg(windows)]
    #[test]
    fn windows_cpu_set_records_parse() {
        // Two records laid out as SYSTEM_CPU_SET_INFORMATION (32 bytes each).
        let mut buffer = Vec::new();
        for (index, core, class) in [(0u8, 0u8, 1u8), (1, 1, 0)] {
            let mut record = vec![0u8; 32];
            record[0..4].copy_from_slice(&32u32.to_le_bytes());
            record[14] = index;
            record[15] = core;
            record[18] = class;
            buffer.extend(record);
        }
        let topology = platform::parse(&buffer).unwrap();
        assert_eq!(topology.logical, vec![cpu(0, 0, 1), cpu(1, 1, 0)]);
    }

    #[test]
    fn this_machine_reports_something_consistent() {
        if let Some(topology) = detect() {
            assert!(topology.physical_count() <= topology.logical_count());
            assert!(topology.performance_cores() <= topology.physical_count());
            assert!(!topology.performance_primary_cpus().is_empty());
        }
    }
}
