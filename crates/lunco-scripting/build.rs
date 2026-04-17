fn main() {
    // If we are on Linux/macOS and want to allow unresolved symbols for PyO3
    // so we can load libpython at runtime.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-undefined,dynamic_lookup");
    } else if target_os == "linux" {
        // On Linux, we can use this to allow unresolved symbols in the binary
        // which will be resolved via dlopen(..., RTLD_GLOBAL) at runtime.
        println!("cargo:rustc-link-arg=-Wl,--allow-shlib-undefined");
    }
}
