fn main() {
    #[cfg(all(windows, feature = "package-icons"))]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let icon = std::env::var("DEP_LUNCO_LUNCOSIM_ICON_ICON_PATH")
                .expect("the UI package must generate the Windows application icon");
            let product_version = std::env::var("LUNCO_RELEASE_VERSION")
                .or_else(|_| std::env::var("CARGO_PKG_VERSION"))
                .expect("Cargo always provides the package version");
            println!("cargo:rerun-if-changed={icon}");
            println!("cargo:rerun-if-env-changed=LUNCO_RELEASE_VERSION");
            let mut resource = winresource::WindowsResource::new();
            resource
                .set_icon(&icon)
                .set("CompanyName", "LunCoSim")
                .set("FileDescription", "LunCoSim")
                .set("FileVersion", &product_version)
                .set("OriginalFilename", "luncosim.exe")
                .set("ProductName", "LunCoSim")
                .set("ProductVersion", &product_version);
            resource
                .compile()
                .expect("failed to embed the LunCoSim executable icon");
        }
    }
}
