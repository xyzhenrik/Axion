fn main() {
    // cef-dll-sys copies libcef.so and its resources next to the binary; let the loader find it there.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
