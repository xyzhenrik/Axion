# Install

Axion comes in three forms. Pick the one that fits you.

| Form | Good for | Updates itself |
|---|---|---|
| **AppImage** (`Axion-X.Y.Z-x86_64.AppImage`) | One file you can put anywhere and start. | Yes, in place. |
| **Tarball** (`Axion-X.Y.Z-linux-x86_64.tar.gz`) | A folder you unpack and install for your user with one command. | No: install the next tarball over it. |
| **From source** | Developers; the newest code. | Yes (`git pull` and rebuild). |

All downloads are on the project's releases page, together with a `SHA256SUMS` file. Check what you
downloaded:

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

None of this needs root.

## AppImage

```bash
chmod +x Axion-*.AppImage
./Axion-*.AppImage
```

Put the file where you want to keep it (for example `~/Applications/`) before the first start: Axion
adds a menu entry that points at that file, and updates replace that file. To get a menu entry, icons
and an `axion` command in one step, use the installer from the tarball:

```bash
./install.sh Axion-X.Y.Z-x86_64.AppImage
```

If the AppImage does not start because your system has no FUSE, see
[Troubleshooting](troubleshooting.md#the-appimage-does-not-start). Short version:
`./Axion-*.AppImage --appimage-extract-and-run`.

## Tarball

```bash
tar -xzf Axion-X.Y.Z-linux-x86_64.tar.gz
cd Axion-X.Y.Z-linux-x86_64
./install.sh
```

The installer, for your user only:

- copies Axion to `~/.local/opt/axion/`,
- links `~/.local/bin/axion` to it (add `~/.local/bin` to your `PATH` if the installer says it is
  missing),
- adds the menu entry `~/.local/share/applications/axion.desktop` and the icons in
  `~/.local/share/icons/hicolor`.

Run it again with a newer tarball to update: the old copy is replaced in one step. Your settings and
data (`~/.config/axion`, `~/.local/share/axion`) are never touched.

Options: `--prefix DIR` (instead of `~/.local/opt/axion`), `--bin-dir DIR` (instead of
`~/.local/bin`), `--uninstall` (removes the program, the command, the entry and the icons; your data
stays).

You can also run Axion straight from the unpacked folder with `./axion`, without installing.

## From source

See [Getting started](getting-started.md#get-axion) and the [README](../README.md#build-and-run).
To make the packages yourself, run `tools/package.sh` (see [Updates](features/updates.md#for-maintainers)).
`tools/install.sh` also installs a folder from `dist/` (`tools/install.sh dist/Axion.AppDir`).

## What your system needs

Axion carries its own copy of Chromium and almost everything it links, but a browser cannot ship the
whole desktop. Your system provides:

- **x86_64 Linux** with **glibc 2.39 or newer** for the official releases (Ubuntu 24.04, Debian 13,
  Fedora 40, Arch and newer). A package you build yourself needs at least the glibc of the machine
  that built it; `tools/package.sh` prints that number.
- A **Vulkan-capable graphics card with its driver** (Mesa for AMD/Intel, the NVIDIA driver). The
  window opens on **Wayland** or **X11**.
- The libraries Chromium loads. On a normal desktop they are all there already. On a minimal system
  install them (Debian/Ubuntu names; Fedora and Arch have the same libraries under their own
  names):

  ```
  libnss3 libnspr4 libatk1.0-0 libatk-bridge2.0-0 libatspi2.0-0 libcups2 libdbus-1-3 libdrm2
  libgbm1 libglib2.0-0 libasound2 libxkbcommon0 libx11-6 libxcb1 libxcomposite1 libxdamage1
  libxext6 libxfixes3 libxrandr2 libcairo2 libpango-1.0-0 libexpat1 libudev1 libssl3 (OpenSSL 3)
  ```
- Loaded only when needed: `libwayland-client` (Wayland), `libva` (hardware video decoding),
  `libpipewire` and `libpulse` (screen sharing, sound), and the TPM libraries below.
- Optional: a **TPM** (`/dev/tpmrm0`, you in the `tss` group) to seal the password vault's key. The
  TPM libraries (`libtss2-*`) are inside the package so Axion starts everywhere; without the system's
  `libtss2-tcti-device` Axion simply uses its key file, which works as well for most people (see
  [Troubleshooting](troubleshooting.md#the-tpm-does-not-work)).

To check a download on your machine, from the unpacked folder:

```bash
ldd usr/bin/axion usr/bin/libcef.so | grep "not found"     # prints nothing when all is there
```

## The sandbox

Chromium runs every web page in a **sandbox**: a locked-down process that cannot read your files or
start programs even if a page attacks the browser. Axion keeps it on, always, with the
**user-namespace sandbox**, which needs no special files and no root.

(Chromium's older sandbox helper, `chrome-sandbox`, must be owned by root with the setuid bit. That is
impossible inside an AppImage or a home folder, so releases do not contain it.)

Some systems forbid the user namespaces that sandbox needs, most notably **Ubuntu 23.10 and newer**
(AppArmor), and some hardened kernels or containers. There Axion **refuses to start** and says why,
instead of silently browsing without protection. Pick one fix:

1. **Allow Axion in AppArmor** (Ubuntu; the narrowest fix). Save as `/etc/apparmor.d/axion`, with the
   path of the `axion` program you use (for an install made by `install.sh` it is the first form; for
   an AppImage the second):

   ```
   abi <abi/4.0>,
   include <tunables/global>

   profile axion /home/*/.local/opt/axion/usr/bin/axion flags=(unconfined) {
     userns,
     include if exists <local/axion>
   }
   ```

   ```
   profile axion /tmp/.mount_Axion*/usr/bin/axion flags=(unconfined) {
   ```

   then `sudo apparmor_parser -r /etc/apparmor.d/axion`. This follows the pattern Ubuntu documents for
   such programs; if it does not take, the next fix always works.
2. **Allow user namespaces for everybody** (a system security setting, your call):
   `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0` on Ubuntu, or
   `sudo sysctl kernel.unprivileged_userns_clone=1` on Debian-style kernels. Put it in a file under
   `/etc/sysctl.d/` to keep it after a reboot.
3. **Run without the sandbox**: `AXION_NO_SANDBOX=1 axion`. **Only for containers and test runs.** A
   web page that exploits a Chromium bug then has all the rights of your user account. Do not browse
   the open web like this.

`AXION_SANDBOX_CHECK=0` turns off only the launcher's up-front test (use it if the test is wrong for
your system); Chromium still needs a working sandbox.

## Updating

- **AppImage**: Axion updates the file itself: **Settings > About & Updates**, see
  [Updates](features/updates.md).
- **Tarball**: download the new tarball and run its `install.sh`.
- **From source**: **Update from Git** in the same place, or `git pull` and rebuild.

## Uninstall

```bash
~/.local/opt/axion/install.sh --uninstall     # a tarball install keeps its installer here
```

(After `./install.sh Axion-X.Y.Z-x86_64.AppImage`, run `--uninstall` from the unpacked tarball or from
`tools/install.sh`. An AppImage you only downloaded: delete the file and
`~/.local/share/applications/axion.desktop`.)
To remove your data as well, delete `~/.config/axion` and `~/.local/share/axion`. That also deletes
the password vault's device key: exported passwords are the only way back.

## What is in a release

| File | What it is |
|---|---|
| `Axion-X.Y.Z-x86_64.AppImage` | The program and its engine in one file (about 160 to 350 MB, depending on the engine build). |
| `Axion-X.Y.Z-linux-x86_64.tar.gz` | The same tree as a folder, plus `install.sh` and an `axion` launcher. |
| `SHA256SUMS` | Checksums of the two files above; Axion's updater checks them. |

Inside, `usr/bin/` holds the `axion` program, the Chromium engine (`libcef.so`, `*.pak` resources,
`icudtl.dat`, `v8_context_snapshot.bin`, `locales/`, the software Vulkan fallback `libvk_swiftshader.so`
and `libvulkan.so.1`) and the TPM libraries. The program finds its engine next to itself (its
`RUNPATH` is `$ORIGIN`), so the folder can be moved anywhere. `AppRun` is the launcher; it never
sets `LD_LIBRARY_PATH`, so programs Axion starts keep a clean environment.

**Video codecs.** Official release files are built on GitHub with the stock Chromium engine, which
cannot play H.264/AAC (YouTube in some resolutions, many news sites). VP8/VP9/AV1 and Opus/Vorbis work.
A package built on your own machine with a custom engine (see
[Troubleshooting](troubleshooting.md#some-videos-do-not-play-h264--aac) and `tools/build-cef.sh`)
includes them; the maintainer may attach such a build to a release instead.
