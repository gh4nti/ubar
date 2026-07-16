use std::collections::HashSet;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::{
    K32EmptyWorkingSet, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SET_QUOTA, PROCESS_VM_READ,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessTreeMemory {
    pub resident_bytes: u64,
    pub committed_bytes: u64,
    pub process_count: usize,
}

fn browser_tree_ids() -> HashSet<u32> {
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return HashSet::from([std::process::id()]);
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = Vec::new();
    if unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok() {
        loop {
            processes.push((entry.th32ProcessID, entry.th32ParentProcessID));
            if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
                break;
            }
        }
    }
    let _ = unsafe { CloseHandle(snapshot) };

    let mut tree = HashSet::from([std::process::id()]);
    loop {
        let before = tree.len();
        for &(pid, parent) in &processes {
            if tree.contains(&parent) {
                tree.insert(pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }

    tree
}

pub fn browser_tree_memory() -> ProcessTreeMemory {
    let mut result = ProcessTreeMemory::default();
    for pid in browser_tree_ids() {
        let Ok(process) = (unsafe {
            OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
        }) else {
            continue;
        };
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        if unsafe {
            K32GetProcessMemoryInfo(
                process,
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            )
        }
        .as_bool()
        {
            result.resident_bytes = result
                .resident_bytes
                .saturating_add(counters.WorkingSetSize as u64);
            result.committed_bytes = result
                .committed_bytes
                .saturating_add(counters.PrivateUsage as u64);
            result.process_count += 1;
        }
        let _ = unsafe { CloseHandle(process) };
    }
    result
}

pub fn trim_browser_children_working_sets() -> usize {
    let root = std::process::id();
    browser_tree_ids()
        .into_iter()
        .filter(|pid| *pid != root)
        .filter(|pid| {
            let Ok(process) = (unsafe {
                OpenProcess(
                    PROCESS_QUERY_INFORMATION | PROCESS_SET_QUOTA,
                    false,
                    *pid,
                )
            }) else {
                return false;
            };
            let trimmed = unsafe { K32EmptyWorkingSet(process) }.as_bool();
            let _ = unsafe { CloseHandle(process) };
            trimmed
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_current_process() {
        let memory = browser_tree_memory();
        assert!(memory.process_count >= 1);
        assert!(memory.resident_bytes > 0);
        assert!(memory.committed_bytes > 0);
    }
}
