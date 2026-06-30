fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(target_os = "macos")]
    add_swift_rpath();
}

#[cfg(target_os = "macos")]
fn add_swift_rpath() {
    use std::process::Command;

    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    if let Ok(output) = Command::new("xcode-select").arg("-p").output() {
        if output.status.success() {
            let xcode_path = String::from_utf8_lossy(&output.stdout).trim().to_string();

            let xctoolchain = format!(
                "{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx"
            );
            println!("cargo:rustc-link-arg=-Wl,-rpath,{xctoolchain}");

            let xctoolchain_new =
                format!("{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{xctoolchain_new}");

            let clt_swift55 = format!("{xcode_path}/usr/lib/swift-5.5/macosx");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{clt_swift55}");

            let clt_swift = format!("{xcode_path}/usr/lib/swift/macosx");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{clt_swift}");
        }
    }
}
