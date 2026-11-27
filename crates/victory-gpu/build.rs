//! The `dlss` feature links the DLSS SDK's loader (`nvsdk_ngx_d.lib`, the
//! dynamic-CRT one) from `VICTORY_DLSS_SDK` (claude/dlss-1440, from
//! pepezin-ai/VIctory-NVIDIA). Nothing happens without the feature.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=VICTORY_DLSS_SDK");
    if std::env::var_os("CARGO_FEATURE_DLSS").is_some() {
        let sdk = std::env::var_os("VICTORY_DLSS_SDK")
            .map(PathBuf::from)
            .expect("the dlss feature needs VICTORY_DLSS_SDK (the NVIDIA/DLSS checkout)");
        println!("cargo:rustc-link-search=native={}", sdk.join("lib/Windows_x86_64/x64").display());
        // The SDK's loader reads the registry (RegOpenKeyExW and friends).
        println!("cargo:rustc-link-lib=advapi32");
    }
}
