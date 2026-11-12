# Third-Party Notices

Ferrite (`https://github.com/Gaalix/ferrite`) is licensed under the MIT License (`LICENSE`).
Ferrite incorporates or links dynamically with the following open-source components:

---

## 1. Non-Affiliation & Trademark Notice

Ferrite is an independent open-source desktop media player and is **not** affiliated with,
endorsed by, sponsored by, or officially connected with SmartCode OOD or Stremio.
Compatibility with the open `stremio://` addon manifest protocol is provided solely for user
interoperability.

---

## 2. `stremio-core` (`vendor/stremio-core`)

- **Upstream:** `https://github.com/Stremio/stremio-core`
- **Copyright:** Copyright (c) SmartCode OOD
- **License:** MIT License (full text in `vendor/stremio-core/LICENSE.md`)

Permission is hereby granted, free of charge, to any person obtaining a copy of `stremio-core`
and associated documentation files, subject to the standard MIT copyright and permission notice
preserved in `vendor/stremio-core/LICENSE.md`.

---

## 3. `gpui` & Platform Crates (`vendor/zed-gpui`)

- **Upstream:** `https://github.com/zed-industries/zed`
- **Copyright:** Copyright (c) Zed Industries, Inc.
- **License:** Apache License, Version 2.0 (full text in `vendor/zed-gpui/LICENSE-APACHE`)

Licensed under the Apache License, Version 2.0. Modified files in `vendor/zed-gpui` add Wayland
transparent video surface handles (`wayland_video_handles`) and Windows 11 window/icon integration
(`windows_video_handles`).

---

## 4. `librqbit`

- **Upstream:** `https://github.com/ikatson/rqbit`
- **Copyright:** Copyright (c) Igor Katson and contributors
- **License:** Apache License, Version 2.0 (`http://www.apache.org/licenses/LICENSE-2.0`)

---

## 5. `libmpv` / `mpv` (Dynamic Library)

- **Upstream:** `https://github.com/mpv-player/mpv`
- **License:** GNU Lesser General Public License v2.1 or later (`LGPL-2.1-or-later`), or GNU
  General Public License v2.0 or later (`GPL-2.0-or-later`) depending on optional codecs compiled
  into the shared library by the operating system vendor or binary distributor.

Ferrite does **not** statically link `libmpv`:
- On **Linux / Ubuntu**, Ferrite links dynamically against the system-installed `libmpv.so.2` /
  `libmpv.so.1` package managed by `apt`.
- On **Windows 11**, Ferrite loads `libmpv-2.dll` dynamically at runtime from the application
  directory (or a user-specified `FERRITE_MPV_DLL` path), allowing end users to inspect, upgrade,
  or replace `libmpv-2.dll` freely with any compatible build.
