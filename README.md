# sqm‑autorate‑rust‑hybrid

**sqm‑autorate‑rust‑hybrid** is an advanced Rust re‑implementation of the original sqm‑autorate tool. It dynamically adjusts CAKE SQM bandwidth settings by measuring real-time traffic load and latency. 

While the original tool was designed for fluctuating LTE/DOCSIS links, this **Hybrid** version is enhanced with high-precision measurement logic and safety constraints, making it equally effective for high-speed symmetric Fiber (FTTH) connections where maintaining the "bottleneck" on the router is critical for zero-bufferbloat performance.

## Features

* **Adaptive Rate Control**: Continuously monitors one‑way delay baselines and traffic load to tune both download and upload shaping rates in real-time.
* **Hybrid Per-Reflector Measurement**: 
    * **Smart Fallback**: Prefers high-precision **ICMP Timestamps (Type 13)** for one-way delay calculation.
    * **Dynamic Downgrade**: Automatically falls back to **ICMP Echo (Type 8)** on a per-reflector basis if Timestamps are filtered by the target (e.g., public DNS like 1.1.1.1).
    * **Auto-Promotion**: Periodically retries Timestamps to restore maximum precision if the network path allows.
* **Hardware Ceiling (Max Rate)**: Introduces `download_max_kbits` and `upload_max_kbits` to prevent capacity probing from exceeding physical link limits (e.g., 1Gbps Ethernet overhead or Cat 5e constraints), ensuring CAKE always remains in control.
* **Robust Reflector Logic**: Dynamic thresholding for reflector health. The system remains operational and quiet as long as at least one reflector is reachable, avoiding log spam in small-pool configurations.
* **Concurrent Architecture**: Dedicated threads for packet sending, receiving, baselining, and rate-control for sub-millisecond responsiveness.
* **Deep OpenWrt Integration**: 
    * UCI support via the init script and `/etc/config/sqm-autorate`.
    * Robust boolean parsing (supports `true/false`, `1/0`, `on/off`).
    * Environment-based logging configuration.
* **Minimal Dependencies**: Relies on a small set of high-performance Rust crates (`anyhow`, `netlink-bindings`, `rustix`), requiring no external shell scripts or heavy libraries.

## Configuration Options (UCI)

| Option | Description | Default |
| :--- | :--- | :--- |
| `enabled` | Enable/Disable the service | `0` |
| `interface` | Interface to manage (e.g., `eth1`) | (required) |
| `download_base_kbits` | Starting download speed | `750000` |
| `download_max_kbits` | **Hard ceiling** for download probing (0 = Unlimited) | `0` |
| `download_min_kbits` | Minimum download speed during congestion | `375000` |
| `upload_base_kbits` / `upload_max_kbits` / `upload_min_kbits` | Same as above for upload | see `sqm-autorate.config` |
| `measurement_type` | `icmp-timestamps` (with per-reflector echo fallback) or `icmp` | `icmp-timestamps` |
| `tick_interval`, `min_change_interval`, `num_reflectors`, `peer_reselection_time` | Measurement/control timing and reflector pool tuning | `0.5`, `0.5`, `5`, `15` |
| `suppress_statistics`, `stats_file` | CSV statistics in `/tmp` (capped at 1 MiB) | `0`, `/tmp/sqm-autorate.csv` |

## Installation

### 1. Native Rust build

1. Ensure you have the Rust toolchain (1.85+, edition 2024), Cargo, and `libnl` headers installed.
2. Clone and build:

   ```bash
   git clone https://github.com/glolsh/sqm-autorate-rust-hybrid.git
   cd sqm-autorate-rust-hybrid
   cargo build --release
   ```
3. The optimized binary is at `target/release/sqm-autorate-rust`.

### 2. OpenWrt SDK build

1. **Prepare the OpenWrt SDK** for your target platform.
2. **Add the package** to your build system:
   ```bash
   # Create a custom feed or link the directory
   mkdir -p package/utils
   cp -r /path/to/sqm-autorate-rust-hybrid/openwrt-pkg package/utils/sqm-autorate-rust
   ```
3. **Configure and build**:
   ```bash
   make menuconfig  # Network -> sqm-autorate-rust-hybrid -> Enable
   make package/utils/sqm-autorate-rust/compile V=s
   ```
4. **Install on your device**:
   ```bash
   opkg install sqm-autorate-rust_*.ipk
   ```
---
AI Collaboration: Major architectural improvements, including the hybrid ICMP measurement logic and hardware ceiling implementation, were developed in collaboration with AI. We advocate for the transparent and ethical use of AI tools to build more reliable software.
