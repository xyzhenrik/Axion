use std::process::Command;

fn main() {
    // cef-dll-sys copies libcef.so and its resources next to the binary; let the loader find it there.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
    embed_git_describe();
}

/// `AXION_GIT_DESCRIBE`: what `git describe --tags --always --dirty` says about the checkout
/// this binary was built from (shown in Settings › About). Empty when built outside a Git
/// checkout (a source tarball, a package build): the About page then shows the version alone.
fn embed_git_describe() {
    let describe = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .current_dir(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()))
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_default();
    println!("cargo::rustc-env=AXION_GIT_DESCRIBE={describe}");
    // Rebuild when the checked-out commit changes (works for worktrees, where `.git` is a file).
    for path in ["HEAD", "refs"] {
        if let Ok(out) = Command::new("git").args(["rev-parse", "--git-path", path]).output()
            && out.status.success()
        {
            println!("cargo::rerun-if-changed={}", String::from_utf8_lossy(&out.stdout).trim());
        }
    }
    println!("cargo::rerun-if-changed=build.rs");
}
