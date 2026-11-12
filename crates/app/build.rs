use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let ico_path = manifest_dir.join("../../assets/icons/ferrite.ico");
    println!("cargo:rerun-if-changed={}", ico_path.display());

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let ico_bytes = fs::read(&ico_path)?;
    let res_bytes = compile_ico_to_win32_res(&ico_bytes)?;
    let res_path = out_dir.join("ferrite_icon.res");
    fs::write(&res_path, res_bytes)?;

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if target_env == "gnu" {
        let obj_path = out_dir.join("ferrite_icon.o");
        if try_gnu_windres(&res_path, &obj_path) {
            println!("cargo:rustc-link-arg-bins={}", obj_path.display());
        } else {
            println!("cargo:rustc-link-arg-bins={}", res_path.display());
        }
    } else {
        let obj_path = out_dir.join("ferrite_icon.obj");
        if try_msvc_cvtres(&res_path, &obj_path, &target_arch) {
            println!("cargo:rustc-link-arg-bins={}", obj_path.display());
        } else {
            println!("cargo:rustc-link-arg-bins={}", res_path.display());
        }
    }

    Ok(())
}

fn try_msvc_cvtres(res_path: &Path, obj_path: &Path, target_arch: &str) -> bool {
    let machine = match target_arch {
        "aarch64" => "/MACHINE:ARM64",
        "x86" => "/MACHINE:X86",
        _ => "/MACHINE:X64",
    };
    let out_arg = format!("/OUT:{}", obj_path.display());

    for candidate in discover_cvtres_candidates(target_arch) {
        let ok = Command::new(&candidate)
            .args(["/NOLOGO", "/READONLY", machine, &out_arg])
            .arg(res_path)
            .status()
            .is_ok_and(|s| s.success());
        if ok && obj_path.is_file() {
            return true;
        }
    }
    false
}

fn discover_cvtres_candidates(target_arch: &str) -> Vec<PathBuf> {
    let mut list = vec![PathBuf::from("cvtres.exe"), PathBuf::from("llvm-cvtres.exe")];
    let arch_folder = match target_arch {
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => "x64",
    };

    if let Ok(vc_tools) = env::var("VCToolsInstallDir") {
        let p = PathBuf::from(vc_tools)
            .join("bin")
            .join("Hostx64")
            .join(arch_folder)
            .join("cvtres.exe");
        if p.is_file() {
            list.insert(0, p);
        }
    }

    for env_key in ["ProgramFiles", "ProgramFiles(x86)"] {
        let Ok(base) = env::var(env_key) else {
            continue;
        };
        let vs_root = PathBuf::from(base).join("Microsoft Visual Studio");
        let Ok(years) = fs::read_dir(vs_root) else {
            continue;
        };
        for yr in years.flatten() {
            let Ok(editions) = fs::read_dir(yr.path()) else {
                continue;
            };
            for ed in editions.flatten() {
                let msvc_dir = ed.path().join("VC").join("Tools").join("MSVC");
                let Ok(vers) = fs::read_dir(msvc_dir) else {
                    continue;
                };
                for ver in vers.flatten() {
                    let exe = ver
                        .path()
                        .join("bin")
                        .join("Hostx64")
                        .join(arch_folder)
                        .join("cvtres.exe");
                    if exe.is_file() {
                        list.push(exe);
                    }
                }
            }
        }
    }
    list
}

fn try_gnu_windres(res_path: &Path, obj_path: &Path) -> bool {
    for tool in ["x86_64-w64-mingw32-windres", "windres"] {
        let ok = Command::new(tool)
            .arg("-i")
            .arg(res_path)
            .arg("-o")
            .arg(obj_path)
            .status()
            .is_ok_and(|s| s.success());
        if ok && obj_path.is_file() {
            return true;
        }
    }
    false
}

fn compile_ico_to_win32_res(ico: &[u8]) -> Result<Vec<u8>, String> {
    if ico.len() < 6 {
        return Err("ICO file shorter than 6-byte header".to_owned());
    }
    let reserved = u16::from_le_bytes([ico[0], ico[1]]);
    let ico_type = u16::from_le_bytes([ico[2], ico[3]]);
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if reserved != 0 || ico_type != 1 || count == 0 {
        return Err("Invalid ICO header fields".to_owned());
    }
    let dir_end = 6usize
        .checked_add(count.checked_mul(16).ok_or("ICO directory overflow")?)
        .ok_or("ICO directory overflow")?;
    if ico.len() < dir_end {
        return Err("ICO directory truncated".to_owned());
    }

    let mut out = Vec::with_capacity(ico.len() + 256);
    append_res_record(&mut out, 0, 0, 0, &[]);

    let mut grp_payload = Vec::with_capacity(6 + count * 14);
    grp_payload.extend_from_slice(&ico[0..6]);

    for idx in 0..count {
        let entry_off = 6 + idx * 16;
        let entry = &ico[entry_off..entry_off + 16];
        let bytes_in_res =
            u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let img_offset =
            u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        let img_end = img_offset
            .checked_add(bytes_in_res)
            .ok_or("ICO image slice overflow")?;
        if img_end > ico.len() {
            return Err("ICO image slice out of bounds".to_owned());
        }

        let icon_id = (idx + 1) as u16;
        append_res_record(&mut out, 3, icon_id, 0x1010, &ico[img_offset..img_end]);

        grp_payload.extend_from_slice(&entry[0..12]);
        grp_payload.extend_from_slice(&icon_id.to_le_bytes());
    }

    append_res_record(&mut out, 14, 1, 0x1030, &grp_payload);
    Ok(out)
}

fn append_res_record(out: &mut Vec<u8>, type_id: u16, name_id: u16, mem_flags: u16, data: &[u8]) {
    let data_size = data.len() as u32;
    let header_size = 32u32;
    let lang_id: u16 = if type_id == 0 { 0 } else { 0x0409 };

    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&type_id.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&name_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&mem_flags.to_le_bytes());
    out.extend_from_slice(&lang_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(data);

    let pad = (4 - (data.len() % 4)) % 4;
    for _ in 0..pad {
        out.push(0);
    }
}
