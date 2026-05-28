fn main() {
    #[cfg(target_os = "macos")]
    add_swift_rpath();
}

#[cfg(target_os = "macos")]
fn add_swift_rpath() {
    use std::process::Command;

    // Add system Swift runtime path (macOS ships libswiftCore here)
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    // Add CLT / Xcode Swift 5.5 runtime path (provides libswift_Concurrency.dylib)
    if let Ok(output) = Command::new("xcode-select").arg("-p").output() {
        if output.status.success() {
            let xcode_path = String::from_utf8_lossy(&output.stdout).trim().to_string();

            // Xcode app toolchain layout
            let xctoolchain = format!(
                "{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx"
            );
            println!("cargo:rustc-link-arg=-Wl,-rpath,{xctoolchain}");

            let xctoolchain_new =
                format!("{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{xctoolchain_new}");

            // Command Line Tools layout (no Toolchains subdirectory)
            let clt_swift55 = format!("{xcode_path}/usr/lib/swift-5.5/macosx");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{clt_swift55}");

            let clt_swift = format!("{xcode_path}/usr/lib/swift/macosx");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{clt_swift}");
        }
    }
}
