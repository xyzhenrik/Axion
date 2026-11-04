use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::checksum::sha256_bytes;
use crate::install::{InstallKind, detect};
use crate::release::Asset;
use crate::testing::{Route, TestServer};
use axion_config::Updates;
use semver::Version;

const NAME: &str = "Axion-2.0.0-linux-x86_64.tar.gz";

enum Item<'a> {
    File(&'a str, &'a str, u32),
    Dir(&'a str),
    Link(&'a str, &'a str),
    Device(&'a str),
    /// A path written into the header as is (`..` and absolute paths included).
    Raw(&'a str, &'a str),
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, bytes).expect("gzip");
    gz.finish().expect("gzip")
}

fn build(items: &[Item]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for item in items {
        let mut header = tar::Header::new_gnu();
        match item {
            Item::File(path, content, mode) => {
                header.set_size(content.len() as u64);
                header.set_mode(*mode);
                header.set_entry_type(tar::EntryType::Regular);
                builder.append_data(&mut header, path, content.as_bytes()).expect("append");
            }
            Item::Dir(path) => {
                header.set_mode(0o755);
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                builder.append_data(&mut header, path, std::io::empty()).expect("append");
            }
            Item::Link(path, target) => {
                header.set_mode(0o777);
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                builder.append_link(&mut header, path, target).expect("append");
            }
            Item::Device(path) => {
                header.set_entry_type(tar::EntryType::Char);
                header.set_size(0);
                builder.append_data(&mut header, path, std::io::empty()).expect("append");
            }
            Item::Raw(path, content) => {
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_entry_type(tar::EntryType::Regular);
                let name = &mut header.as_old_mut().name;
                name[..path.len()].copy_from_slice(path.as_bytes());
                header.set_cksum();
                builder.append(&header, content.as_bytes()).expect("append");
            }
        }
    }
    gzip(&builder.into_inner().expect("finish"))
}

fn good() -> Vec<u8> {
    build(&[
        Item::Dir("Axion-2.0.0-linux-x86_64/"),
        Item::File("Axion-2.0.0-linux-x86_64/AppRun", "#!/bin/sh\n", 0o755),
        Item::Dir("Axion-2.0.0-linux-x86_64/usr/bin/"),
        Item::File("Axion-2.0.0-linux-x86_64/usr/bin/axion", "new binary", 0o755),
        Item::File("Axion-2.0.0-linux-x86_64/usr/share/doc.txt", "docs", 0o644),
        Item::Link("Axion-2.0.0-linux-x86_64/axion", "AppRun"),
        Item::Link("Axion-2.0.0-linux-x86_64/usr/lib64", "../lib"),
    ])
}

struct Fixture {
    dir: tempfile::TempDir,
    install: PathBuf,
    server: TestServer,
    release: Release,
}

/// An installation `<tmp>/opt/axion` of version 1.0.0 and a server offering `archive`
/// (checksummed as `sum_of`, when that differs).
fn fixture(archive: Vec<u8>, sum_of: Option<&[u8]>) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let install = dir.path().join("opt/axion");
    std::fs::create_dir_all(install.join("usr/bin")).expect("mkdir");
    std::fs::write(install.join("AppRun"), "old launcher").expect("write");
    std::fs::write(install.join("usr/bin/axion"), "old binary").expect("write");
    write_marker(&install, "1.0.0").expect("marker");
    let sums = format!("{}  {NAME}\n", sha256_bytes(sum_of.unwrap_or(&archive)));
    let server = TestServer::start(vec![
        Route::bytes(&format!("/dl/{NAME}"), archive),
        Route::text("/dl/SHA256SUMS", &sums),
    ]);
    let asset = |name: &str| Asset {
        name: name.into(),
        url: server.url(&format!("/dl/{name}")),
        size: None,
        sha256: None,
    };
    let release = Release {
        version: Version::new(2, 0, 0),
        tag: "v2.0.0".into(),
        name: String::new(),
        notes: String::new(),
        page_url: String::new(),
        prerelease: false,
        assets: vec![asset(NAME), asset("SHA256SUMS")],
    };
    Fixture { dir, install, server, release }
}

fn stage(f: &Fixture, max: u64) -> Result<Staged, UpdateError> {
    let http = Http::new(&Updates::default(), "Axion/test");
    stage_tarball(&f.release, &f.install, "x86_64", &http, max, &AtomicBool::new(false), &mut |_| {})
}

fn names(f: &Fixture) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(f.dir.path().join("opt"))
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

const BIG: u64 = 1 << 30;

#[test]
fn detection_by_marker_or_by_the_old_layout() {
    let f = fixture(good(), None);
    let exe = f.install.join("usr/bin/axion");
    assert_eq!(detect_dir(&exe), Some(f.install.clone()));
    assert_eq!(detect(None, &exe), InstallKind::Tarball { dir: f.install.clone() });
    // 0.2.0 / 0.3.0 tarballs have no marker: AppRun + usr/bin/axion is enough.
    std::fs::remove_file(f.install.join(MARKER)).expect("rm");
    assert_eq!(detect_dir(&exe), Some(f.install.clone()));
    // Neither marker nor launcher: not an installation.
    std::fs::remove_file(f.install.join("AppRun")).expect("rm");
    assert_eq!(detect_dir(&exe), None);
    write_marker(&f.install, "1.0.0").expect("marker");
    assert_eq!(detect_dir(&exe), Some(f.install.clone()));
    assert_eq!(detect_dir(Path::new("/usr/bin/axion")), None);
}

#[test]
fn a_good_archive_is_staged_then_swapped_in_keeping_the_old_copy() {
    let f = fixture(good(), None);
    // An older backup and a stale staging folder are cleaned up.
    std::fs::create_dir_all(f.dir.path().join("opt/axion.old-0.5.0")).expect("mkdir");
    std::fs::create_dir_all(f.dir.path().join("opt/axion.new-9.9.9")).expect("mkdir");
    let events = std::cell::RefCell::new(Vec::new());
    let http = Http::new(&Updates::default(), "Axion/test");
    let staged =
        stage_tarball(&f.release, &f.install, "x86_64", &http, BIG, &AtomicBool::new(false), &mut |p| {
            events.borrow_mut().push(p)
        })
        .expect("stages");
    let steps: Vec<Step> = events
        .borrow()
        .iter()
        .filter_map(|p| if let Progress::Step(s) = p { Some(*s) } else { None })
        .collect();
    assert_eq!(steps, [Step::Downloading, Step::Verifying, Step::Extracting]);
    assert_eq!(staged.version, "2.0.0");
    // Staging does not touch the running installation.
    assert_eq!(std::fs::read_to_string(f.install.join("usr/bin/axion")).expect("read"), "old binary");
    assert_eq!(names(&f), ["axion", "axion.new-2.0.0", "axion.old-0.5.0"]);
    let mode = std::fs::metadata(staged.staging.join("usr/bin/axion")).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "permissions are kept");
    assert_eq!(std::fs::read_link(staged.staging.join("axion")).expect("link"), Path::new("AppRun"));

    let backup = apply_staged(&staged).expect("swaps");
    assert_eq!(backup, f.dir.path().join("opt/axion.old-1.0.0"));
    assert_eq!(std::fs::read_to_string(f.install.join("usr/bin/axion")).expect("read"), "new binary");
    assert_eq!(std::fs::read_to_string(backup.join("usr/bin/axion")).expect("read"), "old binary");
    assert_eq!(installed_version(&f.install).as_deref(), Some("2.0.0"));
    assert_eq!(names(&f), ["axion", "axion.old-1.0.0"], "older backups and the download are gone");
}

#[test]
fn a_wrong_checksum_is_refused_and_leaves_nothing() {
    let f = fixture(good(), Some(b"something else"));
    let err = stage(&f, BIG).expect_err("must refuse");
    assert!(matches!(err, UpdateError::ChecksumMismatch { .. }), "{err}");
    assert_eq!(names(&f), ["axion"]);
    assert_eq!(std::fs::read_to_string(f.install.join("usr/bin/axion")).expect("read"), "old binary");
}

#[test]
fn entries_leaving_the_tree_are_refused() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("dotdot", build(&[Item::Raw("Axion/../../evil", "x")])),
        ("absolute", build(&[Item::Raw("/tmp/axion-evil", "x")])),
        ("link up", build(&[Item::Link("Axion/l", "../../..")])),
        ("link absolute", build(&[Item::Link("Axion/l", "/etc")])),
        ("link after a name", build(&[Item::Link("Axion/a/l", "x/../..")])),
        (
            "write through a link",
            build(&[Item::Link("Axion/l", ".."), Item::File("Axion/l/evil", "x", 0o644)]),
        ),
        ("device", build(&[Item::Device("Axion/null")])),
    ];
    for (label, archive) in cases {
        let f = fixture(archive, None);
        let err = stage(&f, BIG).expect_err(label);
        assert!(matches!(err, UpdateError::BadArchive(_)), "{label}: {err}");
        assert_eq!(names(&f), ["axion"], "{label}: nothing left behind");
        assert!(!f.dir.path().join("evil").exists() && !f.dir.path().join("opt/evil").exists(), "{label}");
        assert!(!Path::new("/tmp/axion-evil").exists(), "{label}");
    }
}

#[test]
fn a_truncated_archive_is_refused() {
    let full = good();
    let cut = full[..full.len() * 2 / 3].to_vec();
    // Its checksum matches (the download was complete, the upload was not).
    let f = fixture(cut, None);
    let err = stage(&f, BIG).expect_err("must refuse");
    assert!(matches!(err, UpdateError::BadArchive(_)), "{err}");
    assert_eq!(names(&f), ["axion"]);
}

#[test]
fn a_bomb_and_a_wrong_tree_are_refused() {
    let f = fixture(good(), None);
    let err = stage(&f, 4).expect_err("too big once unpacked");
    assert!(matches!(err, UpdateError::BadArchive(_)), "{err}");
    assert_eq!(names(&f), ["axion"]);

    let f = fixture(build(&[Item::File("Axion/AppRun", "x", 0o755)]), None);
    let err = stage(&f, BIG).expect_err("no usr/bin/axion");
    assert!(matches!(err, UpdateError::BadArchive(_)), "{err}");

    let f = fixture(
        build(&[Item::File("Axion/AppRun", "x", 0o755), Item::File("Axion/usr/bin/axion", "x", 0o644)]),
        None,
    );
    assert!(matches!(stage(&f, BIG), Err(UpdateError::BadArchive(_))), "not executable");

    // A marker for another version is a wrong archive.
    let wrong = r#"{"kind":"tarball","version":"1.5.0","installed_at":1}"#;
    let f = fixture(
        build(&[
            Item::File("Axion/AppRun", "x", 0o755),
            Item::File("Axion/usr/bin/axion", "x", 0o755),
            Item::File("Axion/axion-install.json", wrong, 0o644),
        ]),
        None,
    );
    assert!(matches!(stage(&f, BIG), Err(UpdateError::BadArchive(_))), "wrong version");
}

#[test]
fn cancelling_stops_and_cleans_up() {
    let f = fixture(good(), None);
    let http = Http::new(&Updates::default(), "Axion/test");
    let err =
        stage_tarball(&f.release, &f.install, "x86_64", &http, BIG, &AtomicBool::new(true), &mut |_| {})
            .expect_err("cancelled");
    assert!(matches!(err, UpdateError::Cancelled), "{err}");
    assert_eq!(names(&f), ["axion"]);
}

#[test]
fn a_read_only_install_is_reported_not_attempted() {
    let f = fixture(good(), None);
    let parent = f.dir.path().join("opt");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    let result = stage(&f, BIG);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    // Running as root, a read-only folder can still be written.
    if let Ok(staged) = &result {
        discard_staged(staged);
        return;
    }
    assert!(matches!(result, Err(UpdateError::NotWritable(_))), "{result:?}");
    assert_eq!(f.server.requests(), 0, "nothing was downloaded");
}

#[test]
fn a_failing_swap_is_rolled_back() {
    let f = fixture(good(), None);
    let staged = stage(&f, BIG).expect("stages");
    std::fs::remove_dir_all(&staged.staging).expect("rm");
    let err = apply_staged(&staged).expect_err("staged tree is gone");
    assert!(matches!(err, UpdateError::Io { .. }), "{err}");
    assert_eq!(std::fs::read_to_string(f.install.join("usr/bin/axion")).expect("read"), "old binary");
    assert_eq!(installed_version(&f.install).as_deref(), Some("1.0.0"));
    assert_eq!(names(&f), ["axion"], "no backup left over");
}
