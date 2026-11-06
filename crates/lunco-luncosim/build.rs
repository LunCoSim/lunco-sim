fn main() {
    #[cfg(all(windows, feature = "package-icons"))]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let icon = std::env::var("DEP_LUNCO_LUNCOSIM_ICON_ICON_PATH")
                .expect("the UI package must generate the Windows application icon");
            println!("cargo:rerun-if-changed={icon}");
            let mut resource = winresource::WindowsResource::new();
            resource.set_icon(&icon);
            resource
                .compile()
                .expect("failed to embed the LunCoSim executable icon");
        }
    }
}
