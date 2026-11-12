# Ferrite

**Ferrite** is a fast, native desktop cinema player for **Ubuntu Linux** and **Windows 11**, built in Rust on top of **GPUI**, **librqbit**, and **libmpv**.

Ferrite pairs a 120 fps GPU-rendered editorial interface with hardware-accelerated HDR/Dolby Vision video playback, an embedded loopback BitTorrent & HTTP range streaming engine, and seamless compatibility with existing `stremio://` addons and library sync.

---

## Highlights

- **Zero-Copy Wayland & Hardware-Decoded Windows 11 Playback** — On Linux (Wayland), Ferrite punches a transparent `wgpu` viewport directly down to a `wl_subsurface` driven by `libmpv` (`vaapi` / `nvdec` / `vulkan`) with `0` UI redraws per second when the OSD auto-hides. On Windows 11, Ferrite dynamically loads `libmpv-2.dll` (`d3d11va-copy`) with hardware decoding and embedded DPI-aware window icons.
- **Embedded `librqbit` Streaming Engine** — Loopback-bound (`127.0.0.1`) HTTP byte-range server supporting Warm-Resume Disk LRU caching (`10` / `20` / `50` GB) or zero-SSD-write RAM ring buffering (`2` / `4` GB), UPnP/NAT-PMP port mapping, and automatic dead-swarm stream failover.
- **Structured Stream Picker & Scrub Previews** — Composable multi-select filtering across addon sources, resolutions (`4K`, `1080p`, `720p`), `HDR` / `DV`, and Debrid cached availability, plus off-thread quantized timeline scrub thumbnails and interactive volume/track popovers.
- **Verified In-App Updater** — Checks `Gaalix/ferrite` GitHub Releases over HTTPS against an explicit GitHub host allowlist and verifies `SHA-256` digests against `SHA256SUMS.txt` before applying updates.
- **Self-Contained Repository** — All required `stremio-core` and `zed-gpui` crates are vendored directly under `vendor/`, so a fresh `git clone` builds cleanly without external sibling checkouts.

---

## Installation

### Ubuntu 24.04+ (`.deb` Package — Recommended)

Download the latest `ferrite_<version>_amd64.deb` and `SHA256SUMS.txt` from [GitHub Releases](https://github.com/Gaalix/ferrite/releases), verify the checksum, and install via `apt`:

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo apt install ./ferrite_0.1.0_amd64.deb
```

This installs `/usr/bin/ferrite`, hicolor application icons, and the desktop launcher with `stremio://` deep-link registration.

### Ubuntu User-Local Tarball (`~/.local/bin`)

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
tar -xzf ferrite-linux-x86_64.tar.gz
cd ferrite-linux-x86_64
./scripts/install-ubuntu.sh
```

### Windows 11 (`x64`)

1. **Installer (Recommended):** Download `Ferrite-Setup-<version>-x64.exe` from [GitHub Releases](https://github.com/Gaalix/ferrite/releases) and run it. The installer bundles `ferrite.exe` and `libmpv-2.dll`, creates Start Menu shortcuts, and registers the `stremio://` protocol handler under your user profile without requiring administrator elevation.
2. **Portable `.zip`:** Extract `ferrite-windows-x86_64.zip` to any folder and launch `ferrite.exe` (keep `libmpv-2.dll` in the same directory as `ferrite.exe`).

---

## Building from Source

### Prerequisites — Ubuntu 24.04+

Install Rust (`stable` via [rustup.rs](https://rustup.rs)) and the native Wayland/Vulkan/`libmpv` development headers:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential cmake pkg-config dpkg-dev \
  libmpv-dev libwayland-dev libvulkan-dev libegl-dev \
  libfreetype-dev libfontconfig-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev
```

Build and install locally, or package a `.deb`:

```bash
git clone https://github.com/Gaalix/ferrite.git
cd ferrite

cargo build --release -p ferrite
./scripts/install-ubuntu.sh

# Optional: build dist/ferrite_<version>_amd64.deb
./scripts/build-deb.sh
```

### Prerequisites — Windows 11 (`x86_64-pc-windows-msvc`)

1. Install **Rust (`stable-x86_64-pc-windows-msvc`)** and **Visual Studio 2022 Build Tools** (Desktop development with C++ workload).
2. Download a Windows x64 `libmpv-2.dll` build (for example from `shinchiro/mpv-winbuild-cmake`). No C import `.lib` is needed at compile time — Ferrite resolves `libmpv-2.dll` dynamically at runtime.
3. Optional: install **Inno Setup 6** (`ISCC.exe`) to build the setup `.exe`.

```powershell
git clone https://github.com/Gaalix/ferrite.git
cd ferrite

# Build release binary, portable .zip, and Inno Setup installer:
.\scripts\build-windows.ps1 -MpvDllPath C:\path\to\libmpv-2.dll
```

---

## Player Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `Space` | Play / Pause (hold to play at `2x` speed) |
| `Left` / `Right` | Seek `-5s` / `+5s` |
| `Shift+Left` / `Shift+Right` | Seek `-30s` / `+30s` |
| `Up` / `Down` | Volume `+5%` / `-5%` |
| `M` | Toggle mute |
| `F` / `F11` | Toggle fullscreen |
| `S` / `A` | Cycle subtitle / audio tracks |
| `Z` / `X` | Adjust subtitle delay `-100ms` / `+100ms` |
| `I` | Toggle live playback & swarm diagnostics |
| `Ctrl+K` / `/` | Global search & command palette |
| `Escape` | Exit fullscreen or return from player |

---

## Publishing Releases & Auto-Updates

Ferrite ships with a GitHub Actions release pipeline (`.github/workflows/release.yml`). Pushing a semantic version tag builds Ubuntu and Windows 11 artifacts, computes `SHA256SUMS.txt`, and publishes a GitHub Release:

```bash
# 1. Bump version in crates/app/Cargo.toml (e.g. 0.2.0) and commit:
git add crates/app/Cargo.toml Cargo.lock
git commit -m "Release v0.2.0"

# 2. Tag and push to trigger .github/workflows/release.yml:
git tag v0.2.0
git push origin main --tags
```

Installed clients checking `Settings -> Software updates` validate the release metadata over HTTPS, verify the downloaded package against `SHA256SUMS.txt`, and either atomically replace the user-local Linux binary, stage the verified Ubuntu `.deb`, or launch the silent Windows installer.

---

## Security Architecture

- **Protocol Injection Defense:** Stream and thumbnail URLs passed to `libmpv` are restricted to `http://` and `https://` (`file://`, `edl://`, `lavfi://`, `archive://`, and option-prefixed inputs are rejected).
- **Credential Redaction & Private Permissions:** Addon URLs containing Debrid API tokens are redacted (`scheme://host/...`) across logs and error messages; local SQLite databases, session files, and preferences are written with `0600` file / `0700` directory permissions on Unix.
- **Loopback Isolation & Safe DLL Loading:** The embedded streaming engine binds strictly to `127.0.0.1` without permissive cross-origin headers, and Windows `libmpv-2.dll` loading uses `LOAD_LIBRARY_SEARCH_APPLICATION_DIR` to prevent working-directory DLL hijacking.

---

## License

Released under the [MIT License](LICENSE). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for third-party attributions and dynamic linking notices.
