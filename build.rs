use std::{env, fs, path::PathBuf, process::Command};

fn resource_compiler() -> PathBuf {
    if let Some(path) = env::var_os("RC") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            let candidate = dir.join("rc.exe");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    let sdk = env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("ProgramFiles(x86)").expect("Windows SDK is required"))
                .join("Windows Kits/10")
        });
    let host = env::var("HOST").unwrap_or_default();
    let arch = if host.starts_with("aarch64") {
        "arm64"
    } else if host.starts_with("i686") {
        "x86"
    } else {
        "x64"
    };
    let mut versions: Vec<_> = fs::read_dir(sdk.join("bin"))
        .expect("Install the Windows SDK, or set RC to rc.exe")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("10."))
        })
        .collect();
    versions.sort();
    versions
        .into_iter()
        .rev()
        .map(|version| version.join(arch).join("rc.exe"))
        .find(|path| path.is_file())
        .expect("Install the Windows SDK resource compiler, or set RC to rc.exe")
}

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=WindowsSdkDir");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "Windows builds require the MSVC toolchain"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("assets/icon.ico")
        .to_string_lossy()
        .replace('\\', "/");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numeric = ["MAJOR", "MINOR", "PATCH"].map(|part| {
        env::var(format!("CARGO_PKG_VERSION_{part}"))
            .unwrap()
            .parse::<u16>()
            .expect("Windows version components must fit in 16 bits")
    });
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0
FILEOS 0x40004L
FILETYPE 1
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "FileDescription", "Speaker Studio\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "ProductName", "Speaker Studio\0"
      VALUE "ProductVersion", "{version}\0"
      VALUE "OriginalFilename", "Speaker Studio.exe\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        major = numeric[0],
        minor = numeric[1],
        patch = numeric[2]
    );
    let script = out.join("speaker-studio.rc");
    let resource = out.join("speaker-studio.res");
    fs::write(&script, rc).expect("Could not write Windows resources");
    let status = Command::new(resource_compiler())
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&script)
        .status()
        .expect("Could not start Windows resource compiler");
    assert!(status.success(), "Windows resource compilation failed");
    println!("cargo:rustc-link-arg={}", resource.display());
}
