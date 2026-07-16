# uBar measured results

## Five modern tabs — Windows x86-64

Measured 2026-07-16 on a machine with 16,435,761,152 bytes physical memory.

Profile: GitHub, Reddit, Wikipedia, Microsoft, Apple. Each site loaded while visible. Four background tabs were suspended. Child working sets were trimmed to the Windows pagefile. The benchmark then reactivated GitHub and waited for its second animation frame before the final memory reading.

| Metric | Result |
| --- | ---: |
| Tabs loaded | 5/5 |
| Resident browser-tree RAM after reactivation | 529 MiB |
| Private committed memory | 1,123 MiB |
| Browser-tree processes | 15 |
| Suspended tabs | 4 |
| Suspended-tab second-animation-frame latency | 206 ms |
| Total benchmark time | 24 s |

Command:

```powershell
cargo run -- --benchmark-five-tabs
```

The 529 MiB result is resident memory, not total committed virtual memory. Windows may keep roughly 1.1 GiB committed across RAM and pagefile. This is intentional and matches the disk-backed pressure policy, but it must be shown beside the selling-point number.

This result does not certify Linux/macOS, ARM64, 512 MiB Linux, YouTube 720p, battery use, or tab-restore tail latency. Those require their named hardware and platform runs.
