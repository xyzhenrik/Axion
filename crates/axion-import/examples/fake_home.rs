//! Writes a fake home directory with browser profiles (the fixtures of the tests) for
//! captures of Axion's import UI:
//!
//! ```text
//! cargo run -p axion-import --example fake_home -- /tmp/fake-home
//! AXION_IMPORT_HOME=/tmp/fake-home axion --capture=out.png --open=settings:data
//! ```

fn main() {
    let Some(dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: fake_home <directory>");
        std::process::exit(2);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1_800_000_000, |d| d.as_secs() as i64);
    let dir = std::path::Path::new(&dir);
    axion_import::fixtures::fake_home(dir, now);
    println!("fake home written to {}", dir.display());
}
