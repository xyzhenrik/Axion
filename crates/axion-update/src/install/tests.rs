use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::checksum::sha256_bytes;
use crate::parse_repository;
use crate::release::Asset;
use crate::testing::{Route, TestServer};
use axion_config::Updates;
use semver::Version;

// ---- detection ----

#[test]
fn appimage_is_recognised_by_its_environment_variable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let image = dir.path().join("Axion.AppImage");
    std::fs::write(&image, b"x").expect("write");
    let exe = Path::new("/tmp/.mount_AxionXYZ/usr/bin/axion");
    assert_eq!(detect(Some(image.as_os_str()), exe), InstallKind::AppImage { path: image.clone() });
    // A stale variable (file gone) or an empty one is ignored.
    assert_eq!(detect(Some(OsStr::new("/nonexistent/Axion.AppImage")), exe), InstallKind::Other);
    assert_eq!(detect(Some(OsStr::new("")), exe), InstallKind::Other);
    assert_eq!(detect(None, exe), InstallKind::Other);
}

#[test]
fn a_binary_in_a_checkouts_target_directory_is_a_source_install() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("AxionBrowser");
    std::fs::create_dir_all(root.join(".git")).expect("mkdir");
    std::fs::create_dir_all(root.join("target/release")).expect("mkdir");
    std::fs::write(root.join("Cargo.toml"), b"[workspace]").expect("write");
    let exe = root.join("target/release/axion");
    assert_eq!(detect(None, &exe), InstallKind::SourceCheckout { root: root.clone() });
    // Debug builds count too; a binary elsewhere in the tree does not.
    assert_eq!(
        detect(None, &root.join("target/debug/axion")),
        InstallKind::SourceCheckout { root: root.clone() }
    );
    assert_eq!(detect(None, &root.join("tools/axion")), InstallKind::Other);
    // No .git: just a source tarball.
    std::fs::remove_dir_all(root.join(".git")).expect("rm");
    assert_eq!(detect(None, &exe), InstallKind::Other);
    // A `.git` file (worktrees, submodules) counts.
    std::fs::write(root.join(".git"), b"gitdir: /elsewhere").expect("write");
    assert_eq!(detect(None, &exe), InstallKind::SourceCheckout { root });
    assert_eq!(detect(None, Path::new("/usr/bin/axion")), InstallKind::Other);
}

// ---- AppImage ----

fn elf(payload: &str) -> Vec<u8> {
    let mut bytes = b"\x7fELF".to_vec();
    bytes.extend_from_slice(payload.as_bytes());
    bytes
}

fn http(updates: &Updates) -> Http {
    Http::new(updates, "Axion/test")
}

fn release_with(server: &TestServer, assets: &[(&str, Option<String>)]) -> Release {
    Release {
        version: Version::new(2, 0, 0),
        tag: "v2.0.0".into(),
        name: String::new(),
        notes: String::new(),
        page_url: String::new(),
        prerelease: false,
        assets: assets
            .iter()
            .map(|(name, sha)| Asset {
                name: (*name).into(),
                url: server.url(&format!("/dl/{name}")),
                size: None,
                sha256: sha.clone(),
            })
            .collect(),
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    target: PathBuf,
    server: TestServer,
    new_image: Vec<u8>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("Axion-x86_64.AppImage");
    std::fs::write(&target, b"old image").expect("write");
    let new_image = elf("new image");
    let server = TestServer::start(vec![Route::bytes("/dl/Axion-2.0-x86_64.AppImage", new_image.clone())]);
    Fixture { dir, target, server, new_image }
}

fn install(f: &Fixture, release: &Release, updates: &Updates) -> Result<AppImageInstalled, UpdateError> {
    install_appimage(release, &f.target, "x86_64", &http(updates), &AtomicBool::new(false), &mut |_| {})
}

fn leftovers(f: &Fixture) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(f.dir.path())
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn appimage_is_replaced_atomically_and_the_old_one_kept() {
    let f = fixture();
    let sums = format!("{}  Axion-2.0-x86_64.AppImage\n", sha256_bytes(&f.new_image));
    f.server.add(Route::text("/dl/SHA256SUMS", &sums));
    let release = release_with(&f.server, &[("Axion-2.0-x86_64.AppImage", None), ("SHA256SUMS", None)]);
    let events = RefCell::new(Vec::new());
    let done = install_appimage(
        &release,
        &f.target,
        "x86_64",
        &http(&Updates::default()),
        &AtomicBool::new(false),
        &mut |p| events.borrow_mut().push(p),
    )
    .expect("installs");
    assert!(done.verified);
    assert_eq!(std::fs::read(&f.target).expect("read"), f.new_image);
    assert_eq!(std::fs::read(&done.backup).expect("read"), b"old image");
    assert!(done.backup.to_string_lossy().ends_with("Axion-x86_64.AppImage.old"));
    assert_eq!(std::fs::metadata(&f.target).expect("meta").permissions().mode() & 0o111, 0o111);
    assert_eq!(leftovers(&f), ["Axion-x86_64.AppImage", "Axion-x86_64.AppImage.old"]);
    let steps: Vec<Step> = events
        .borrow()
        .iter()
        .filter_map(|p| if let Progress::Step(s) = p { Some(*s) } else { None })
        .collect();
    assert_eq!(steps, [Step::Downloading, Step::Verifying, Step::Installing]);
    assert!(events.borrow().iter().any(|p| matches!(p, Progress::Download { done, .. } if *done > 0)));
}

#[test]
fn a_wrong_checksum_leaves_everything_untouched() {
    let f = fixture();
    f.server.add(Route::text("/dl/SHA256SUMS", &format!("{}  Axion-2.0-x86_64.AppImage\n", "0".repeat(64))));
    let release = release_with(&f.server, &[("Axion-2.0-x86_64.AppImage", None), ("SHA256SUMS", None)]);
    let err = install(&f, &release, &Updates::default()).expect_err("must fail");
    assert!(matches!(err, UpdateError::ChecksumMismatch { .. }), "{err}");
    assert_eq!(std::fs::read(&f.target).expect("read"), b"old image");
    assert_eq!(leftovers(&f), ["Axion-x86_64.AppImage"]);
}

#[test]
fn sums_without_an_entry_for_the_file_are_refused() {
    let f = fixture();
    f.server.add(Route::text("/dl/SHA256SUMS", &format!("{}  something-else.bin\n", "1".repeat(64))));
    let release = release_with(&f.server, &[("Axion-2.0-x86_64.AppImage", None), ("SHA256SUMS", None)]);
    assert!(matches!(install(&f, &release, &Updates::default()), Err(UpdateError::Parse(_))));
    assert_eq!(std::fs::read(&f.target).expect("read"), b"old image");
}

#[test]
fn a_manifest_checksum_is_checked_too_and_none_means_unverified() {
    let f = fixture();
    let good = Some(sha256_bytes(&f.new_image));
    let release = release_with(&f.server, &[("Axion-2.0-x86_64.AppImage", good)]);
    assert!(install(&f, &release, &Updates::default()).expect("installs").verified);

    let f = fixture();
    let release = release_with(&f.server, &[("Axion-2.0-x86_64.AppImage", Some("f".repeat(64)))]);
    assert!(matches!(install(&f, &release, &Updates::default()), Err(UpdateError::ChecksumMismatch { .. })));

    let f = fixture();
    let release = release_with(&f.server, &[("Axion-2.0-x86_64.AppImage", None)]);
    assert!(!install(&f, &release, &Updates::default()).expect("installs").verified);
}

#[test]
fn only_an_executable_image_may_replace_the_old_one() {
    let f = fixture();
    f.server.add(Route::bytes("/dl/Axion-2.0-x86_64.AppImage", b"<html>404</html>".to_vec()));
    let release = release_with(&f.server, &[("Axion-2.0-x86_64.AppImage", None)]);
    assert!(install(&f, &release, &Updates::default()).is_err());
    assert_eq!(std::fs::read(&f.target).expect("read"), b"old image");
    assert_eq!(leftovers(&f), ["Axion-x86_64.AppImage"]);
}

#[test]
fn size_limit_cancel_and_missing_asset() {
    let f = fixture();
    let release = release_with(&f.server, &[("Axion-2.0-x86_64.AppImage", None)]);
    let tiny = Updates { max_download_mb: 0, ..Updates::default() };
    assert!(matches!(install(&f, &release, &tiny), Err(UpdateError::TooLarge { .. })));
    assert_eq!(leftovers(&f), ["Axion-x86_64.AppImage"]);

    let cancelled = AtomicBool::new(true);
    let result =
        install_appimage(&release, &f.target, "x86_64", &http(&Updates::default()), &cancelled, &mut |_| {});
    assert!(matches!(result, Err(UpdateError::Cancelled)));
    assert_eq!(leftovers(&f), ["Axion-x86_64.AppImage"]);

    let none = release_with(&f.server, &[("notes.txt", None)]);
    assert!(matches!(install(&f, &none, &Updates::default()), Err(UpdateError::NoAsset)));
}

// ---- source checkout, in throwaway repositories ----

struct Repos {
    dir: tempfile::TempDir,
    bare: PathBuf,
    checkout: PathBuf,
}

fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.org")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.org")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("runs");
    assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A bare "remote" at `<tmp>/henrik/axion.git`, a clone of it with one commit pushed, and a
/// second clone that pushes a newer commit — so the first one can fast-forward.
fn repos() -> Repos {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let bare = root.join("henrik/axion.git");
    std::fs::create_dir_all(&bare).expect("mkdir");
    run(&bare, "git", &["init", "--bare", "-b", "main"]);
    let checkout = root.join("checkout");
    run(root, "git", &["clone", bare.to_str().expect("utf8"), "checkout"]);
    std::fs::write(checkout.join("Cargo.toml"), b"[workspace]\n").expect("write");
    std::fs::write(checkout.join(".gitignore"), b"target/\n").expect("write");
    run(&checkout, "git", &["add", "."]);
    run(&checkout, "git", &["commit", "-m", "first"]);
    run(&checkout, "git", &["push", "-u", "origin", "main"]);

    run(root, "git", &["clone", bare.to_str().expect("utf8"), "developer"]);
    let developer = root.join("developer");
    std::fs::write(developer.join("feature.txt"), b"new\n").expect("write");
    run(&developer, "git", &["add", "."]);
    run(&developer, "git", &["commit", "-m", "second"]);
    run(&developer, "git", &["push"]);
    Repos { dir, bare, checkout }
}

/// A `cargo` that logs its arguments and working directory, and exits with `code`.
fn stub_cargo(dir: &Path, code: i32) -> PathBuf {
    let path = dir.join(format!("cargo-stub-{code}"));
    let log = dir.join("cargo.log");
    let script = format!(
        "#!/bin/sh\necho \"$@ in $(pwd)\" >> '{}'\necho '   Compiling axion-app v0.0.1'\necho 'warning: stub' >&2\nexit {code}\n",
        log.display()
    );
    std::fs::write(&path, script).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

fn cargo_log(r: &Repos) -> String {
    std::fs::read_to_string(r.dir.path().join("cargo.log")).unwrap_or_default()
}

fn tools(r: &Repos, cargo_code: i32) -> Tools {
    Tools { git: "git".into(), cargo: stub_cargo(r.dir.path(), cargo_code) }
}

fn repository() -> Repository {
    parse_repository("henrik/axion").expect("valid")
}

fn head(r: &Repos) -> String {
    run(&r.checkout, "git", &["rev-parse", "HEAD"])
}

#[test]
fn a_clean_matching_checkout_pulls_and_builds() {
    let r = repos();
    let before = head(&r);
    let events = RefCell::new(Vec::new());
    update_checkout(&r.checkout, &repository(), &tools(&r, 0), &mut |p| events.borrow_mut().push(p))
        .expect("updates");
    assert_ne!(head(&r), before, "fast-forwarded");
    assert!(r.checkout.join("feature.txt").exists());
    let log = cargo_log(&r);
    assert!(log.starts_with("build -p axion-app --release in "), "{log}");
    assert!(log.contains("checkout"), "built in the checkout: {log}");
    let events = events.borrow();
    let steps: Vec<Step> =
        events.iter().filter_map(|p| if let Progress::Step(s) = p { Some(*s) } else { None }).collect();
    assert_eq!(steps, [Step::CheckingTree, Step::Pulling, Step::Building]);
    let lines: Vec<&str> = events
        .iter()
        .filter_map(|p| if let Progress::Line(l) = p { Some(l.as_str()) } else { None })
        .collect();
    assert!(lines.iter().any(|l| l.contains("Compiling axion-app")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("warning: stub")), "stderr is streamed too: {lines:?}");
    assert!(lines.iter().any(|l| l.contains("feature.txt") || l.contains("Fast-forward")), "{lines:?}");
}

#[test]
fn a_dirty_tree_is_never_touched() {
    let r = repos();
    std::fs::write(r.checkout.join("Cargo.toml"), b"[workspace]\n# local edit\n").expect("write");
    let before = head(&r);
    let err =
        update_checkout(&r.checkout, &repository(), &tools(&r, 0), &mut |_| {}).expect_err("must refuse");
    assert!(matches!(err, UpdateError::DirtyTree), "{err}");
    assert_eq!(head(&r), before);
    assert_eq!(cargo_log(&r), "");
    assert!(!is_clean(&r.checkout, &tools(&r, 0)).expect("status"));

    // An untracked file counts as a change too; an ignored one (target/) does not.
    let r = repos();
    std::fs::create_dir_all(r.checkout.join("target/release")).expect("mkdir");
    std::fs::write(r.checkout.join("target/release/axion"), b"binary").expect("write");
    assert!(is_clean(&r.checkout, &tools(&r, 0)).expect("status"));
    std::fs::write(r.checkout.join("scratch.txt"), b"x").expect("write");
    assert!(!is_clean(&r.checkout, &tools(&r, 0)).expect("status"));
}

#[test]
fn a_foreign_remote_is_refused() {
    let r = repos();
    let other = parse_repository("someone/else").expect("valid");
    let err = update_checkout(&r.checkout, &other, &tools(&r, 0), &mut |_| {}).expect_err("must refuse");
    assert!(matches!(err, UpdateError::RemoteMismatch { .. }), "{err}");
    assert!(!r.checkout.join("feature.txt").exists());
    assert_eq!(cargo_log(&r), "");
    let _ = &r.bare;
}

#[test]
fn a_diverged_branch_does_not_merge_or_force() {
    let r = repos();
    std::fs::write(r.checkout.join("local.txt"), b"mine\n").expect("write");
    run(&r.checkout, "git", &["add", "."]);
    run(&r.checkout, "git", &["commit", "-m", "local work"]);
    let before = head(&r);
    let err = update_checkout(&r.checkout, &repository(), &tools(&r, 0), &mut |_| {}).expect_err("must fail");
    assert!(matches!(err, UpdateError::CommandFailed { .. }), "{err}");
    assert_eq!(head(&r), before, "history untouched");
    assert!(!r.checkout.join("feature.txt").exists());
    assert_eq!(cargo_log(&r), "", "no build after a failed pull");
}

#[test]
fn a_failing_build_aborts_and_reports() {
    let r = repos();
    let err =
        update_checkout(&r.checkout, &repository(), &tools(&r, 101), &mut |_| {}).expect_err("must fail");
    match err {
        UpdateError::CommandFailed { command, status } => {
            assert!(command.starts_with("cargo build"), "{command}");
            assert!(status.contains("101"), "{status}");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn a_detached_head_or_missing_upstream_is_refused() {
    let r = repos();
    run(&r.checkout, "git", &["checkout", "--detach"]);
    let err = inspect_checkout(&r.checkout, &repository(), &tools(&r, 0)).expect_err("detached");
    assert!(matches!(err, UpdateError::NoUpstream), "{err}");

    let r = repos();
    run(&r.checkout, "git", &["checkout", "-b", "topic"]);
    let err = inspect_checkout(&r.checkout, &repository(), &tools(&r, 0)).expect_err("no upstream");
    assert!(matches!(err, UpdateError::NoUpstream), "{err}");

    let r = repos();
    let checkout = inspect_checkout(&r.checkout, &repository(), &tools(&r, 0)).expect("fine");
    assert_eq!((checkout.branch.as_str(), checkout.remote.as_str()), ("main", "origin"));
}
