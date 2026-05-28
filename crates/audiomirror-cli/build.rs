fn main() {
    #[cfg(target_os = "macos")]
    {
        let plist = format!("{}/Info.plist", env!("CARGO_MANIFEST_DIR"));
        println!("cargo:rerun-if-changed=Info.plist");
        println!("cargo:rerun-if-changed=build.rs");
        // Embed Info.plist into the Mach-O __TEXT,__info_plist section so macOS
        // reads NSScreenCaptureUsageDescription for the permission prompt.
        println!(
            "cargo:rustc-link-arg-bin=audiomirror-cli=-Wl,-sectcreate,__TEXT,__info_plist,{plist}"
        );
    }
}
