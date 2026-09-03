//! Embed the checked-in application icon without fetching a resource-build crate.
//! MSVC uses Windows SDK rc.exe; GNU uses the existing MinGW windres compiler.
//! Custom SDK/toolchain locations can set LEIGOD_RC or LEIGOD_WINDRES to an executable.
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn watched_env(name: &str) -> Option<OsString> {
    println!("cargo:rerun-if-env-changed={name}");
    env::var_os(name).filter(|value| !value.is_empty())
}

fn find_program(program: &OsStr) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() > 1 {
        return path.is_file().then(|| path.canonicalize().ok()).flatten();
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return candidate.canonicalize().ok();
        }
        if candidate.extension().is_none() {
            let candidate = candidate.with_extension("exe");
            if candidate.is_file() {
                return candidate.canonicalize().ok();
            }
        }
    }
    None
}

fn explicit_tool(variable: &str) -> Option<PathBuf> {
    watched_env(variable).map(|value| {
        find_program(&value).unwrap_or_else(|| {
            panic!("{variable} must name an existing resource compiler executable, not command-line arguments")
        })
    })
}

fn sdk_candidates(bin_root: &Path, candidates: &mut Vec<PathBuf>) {
    let mut versions: Vec<(Vec<u32>, PathBuf)> = fs::read_dir(bin_root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let numbers: Option<Vec<u32>> = name
                .to_str()?
                .split('.')
                .map(|number| number.parse().ok())
                .collect();
            let numbers = numbers?;
            (numbers.len() >= 3).then(|| (numbers, entry.path()))
        })
        .collect();
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, version) in versions {
        candidates.push(version.join("x64/rc.exe"));
        candidates.push(version.join("x86/rc.exe"));
    }
    candidates.push(bin_root.join("x64/rc.exe"));
    candidates.push(bin_root.join("x86/rc.exe"));
}

fn msvc_resource_compiler() -> PathBuf {
    if let Some(compiler) = explicit_tool("LEIGOD_RC") {
        return compiler;
    }
    if let Some(compiler) = find_program(OsStr::new("rc.exe")) {
        return compiler;
    }
    let mut candidates = Vec::new();
    if let Some(bin) = watched_env("WindowsSdkVerBinPath") {
        candidates.push(PathBuf::from(&bin).join("x64/rc.exe"));
        candidates.push(PathBuf::from(bin).join("x86/rc.exe"));
    }
    if let Some(sdk) = watched_env("WindowsSdkDir") {
        sdk_candidates(&PathBuf::from(sdk).join("bin"), &mut candidates);
    }
    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(programs) = watched_env(variable) {
            sdk_candidates(
                &PathBuf::from(programs).join("Windows Kits/10/bin"),
                &mut candidates,
            );
        }
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .expect("Windows SDK rc.exe was not found. Install the Windows SDK with C++ Build Tools, use its Developer PowerShell, or set LEIGOD_RC to rc.exe")
}

fn gnu_resource_compiler(target: &str) -> PathBuf {
    if let Some(compiler) = explicit_tool("LEIGOD_WINDRES") {
        return compiler;
    }
    let linker_variable = format!(
        "CARGO_TARGET_{}_LINKER",
        target.replace('-', "_").to_ascii_uppercase()
    );
    for variable in [linker_variable.as_str(), "RUSTC_LINKER", "CC"] {
        if let Some(linker) = watched_env(variable).and_then(|value| find_program(&value)) {
            if let Some(directory) = linker.parent() {
                for name in ["windres.exe", "x86_64-w64-mingw32-windres.exe"] {
                    let candidate = directory.join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }
    }
    for name in ["x86_64-w64-mingw32-windres.exe", "windres.exe"] {
        if let Some(compiler) = find_program(OsStr::new(name)) {
            return compiler;
        }
    }
    panic!("MinGW windres.exe was not found beside the configured linker or on PATH. Set LEIGOD_WINDRES to the existing resource compiler")
}

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-env-changed=PATH");
    let target = env::var("TARGET").expect("Cargo must provide TARGET");
    if !target.contains("windows") {
        return;
    }
    assert!(
        matches!(
            target.as_str(),
            "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu"
        ),
        "Native icon resources currently support the project's Windows x64 targets only"
    );
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let icon = fs::read(root.join("assets/app-icon.ico"))
        .expect("Missing checked-in assets/app-icon.ico application icon");
    assert!(
        icon.len() >= 22 && icon[..4] == [0, 0, 1, 0],
        "assets/app-icon.ico must be a Windows ICO file"
    );
    // Keep the RC text ASCII and its filenames relative to OUT_DIR. This avoids
    // quoting/code-page problems in repositories with spaces or non-ASCII paths.
    fs::write(out.join("app-icon.ico"), icon).expect("Could not stage the icon");
    fs::write(out.join("app-icon.rc"), "1 ICON \"app-icon.ico\"\n")
        .expect("Could not write the icon resource script");

    let msvc = target.ends_with("-msvc");
    let compiler = if msvc {
        msvc_resource_compiler()
    } else {
        gnu_resource_compiler(&target)
    };
    let resource = if msvc { "app-icon.res" } else { "app-icon.o" };
    let mut command = Command::new(&compiler);
    command.current_dir(&out);
    if msvc {
        // rc.exe emits a .res file that the MSVC linker consumes directly.
        command.args(["/nologo", "/fo", resource, "app-icon.rc"]);
    } else {
        // GNU ld consumes COFF, not the Microsoft .res container.
        // Its preprocessor must come from the same MinGW tools as windres.
        let mut path = vec![compiler.parent().unwrap().to_path_buf()];
        path.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        command.env(
            "PATH",
            env::join_paths(path).expect("Invalid toolchain PATH"),
        );
        command.args([
            "--input-format=rc",
            "--output-format=coff",
            "--target=pe-x86-64",
            "--input=app-icon.rc",
            "--output=app-icon.o",
        ]);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let result = command.output().unwrap_or_else(|error| {
        panic!(
            "Could not run icon resource compiler {}: {error}",
            compiler.display()
        )
    });
    assert!(
        result.status.success(),
        "Icon resource compiler {} failed: {}\n{}",
        compiler.display(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        out.join(resource).is_file(),
        "Resource compiler produced no output"
    );
    println!(
        "cargo:rustc-link-arg-bin=leigod-guard={}",
        out.join(resource).display()
    );
}
