fn main() {
    #[cfg(target_os = "macos")]
    {
        let plist = format!("{}/Info.plist", env!("CARGO_MANIFEST_DIR"));
        println!("cargo:rerun-if-changed=Info.plist");
        println!("cargo:rerun-if-changed=build.rs");
        println!(
            "cargo:rustc-link-arg-bin=audiomirror-cli=-Wl,-sectcreate,__TEXT,__info_plist,{plist}"
        );
        add_swift_rpath();
    }
}

#[cfg(target_os = "macos")]
fn add_swift_rpath() {
    use std::process::Command;

    // /usr/lib/swift ships libswiftCore on macOS; the Xcode/CLT paths below are
    // needed for libswift_Concurrency.dylib which lives in the toolchain runtimes
    // and is NOT re-exported from /usr/lib/swift on older SDKs.
    println!("cargo:rustc-link-arg-bin=audiomirror-cli=-Wl,-rpath,/usr/lib/swift");

    if let Ok(output) = Command::new("xcode-select").arg("-p").output() {
        if output.status.success() {
            let xcode_path = String::from_utf8_lossy(&output.stdout).trim().to_string();

            let dirs = [
                format!(
                    "{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx"
                ),
                format!("{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx"),
                format!("{xcode_path}/usr/lib/swift-5.5/macosx"),
                format!("{xcode_path}/usr/lib/swift/macosx"),
            ];

            for dir in &dirs {
                println!("cargo:rustc-link-arg-bin=audiomirror-cli=-Wl,-rpath,{dir}");
            }
        }
    }
}
