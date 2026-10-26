//! Hardware acceleration: maps the `[hardware]` settings, the environment overrides and the
//! detected graphics card onto the engine's options. The functions here are pure (the tests
//! cover them); `gpu.rs` and `browser.rs` feed them and apply the result.
//!
//! Axion's own interface always draws with wgpu, whatever the master switch says: it needs a
//! GPU. The switch decides how *Chromium* renders pages, video and canvas, and how its frames
//! reach us.

use axion_config::{Config, GpuChoice, Hardware, PageFrames};

pub const NVIDIA_VENDOR_ID: u32 = 0x10DE;
pub const AMD_VENDOR_ID: u32 = 0x1002;

/// The environment variables that win over the settings (`AXION_GPU`, `AXION_OSR`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    pub gpu: Option<String>,
    pub osr: Option<String>,
}

impl Overrides {
    pub fn from_env() -> Self {
        let get = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        Self { gpu: get("AXION_GPU"), osr: get("AXION_OSR") }
    }
}

/// Which graphics card to look for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterChoice {
    Auto,
    /// The integrated card.
    Low,
    /// The most powerful (discrete) card.
    High,
    /// Any card whose name contains this (lower-case) text: `AXION_GPU=radeon`.
    Name(String),
}

/// The card to use: `AXION_GPU` (`low`, `high`, or part of a card's name) wins over the setting.
pub fn adapter_choice(hardware: &Hardware, env: &Overrides) -> AdapterChoice {
    if let Some(value) = &env.gpu {
        return match value.trim().to_lowercase().as_str() {
            "low" | "integrated" => AdapterChoice::Low,
            "high" | "discrete" => AdapterChoice::High,
            "auto" => from_setting(hardware.gpu),
            name => AdapterChoice::Name(name.to_owned()),
        };
    }
    from_setting(hardware.gpu)
}

fn from_setting(choice: GpuChoice) -> AdapterChoice {
    match choice {
        GpuChoice::Auto => AdapterChoice::Auto,
        GpuChoice::Low => AdapterChoice::Low,
        GpuChoice::High => AdapterChoice::High,
    }
}

/// wgpu's power preference for the card: only "integrated" asks for low power (what
/// `AXION_GPU=low` always did).
pub fn power_preference(choice: &AdapterChoice) -> wgpu::PowerPreference {
    match choice {
        AdapterChoice::Low => wgpu::PowerPreference::LowPower,
        _ => wgpu::PowerPreference::HighPerformance,
    }
}

/// The first card (by index into `adapters`: name and type) that fits the choice, or `None`
/// to let wgpu decide with the power preference.
pub fn pick_adapter(adapters: &[(String, wgpu::DeviceType)], choice: &AdapterChoice) -> Option<usize> {
    match choice {
        AdapterChoice::Auto => None,
        AdapterChoice::Low => adapters.iter().position(|(_, t)| *t == wgpu::DeviceType::IntegratedGpu),
        AdapterChoice::High => adapters.iter().position(|(_, t)| *t == wgpu::DeviceType::DiscreteGpu),
        AdapterChoice::Name(name) => {
            adapters.iter().position(|(n, _)| n.to_lowercase().contains(name.as_str()))
        }
    }
}

/// How Chromium's page frames reach us.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameMode {
    /// Shared GPU memory (dmabuf textures).
    Gpu,
    /// Copied through system memory.
    Software,
}

/// Where the choice came from (shown in the diagnostics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Environment,
    Setting,
    Automatic,
    /// The device cannot import shared frames at all.
    Unsupported,
}

/// What the GPU session looks like, needed to pick the frame mode.
#[derive(Clone, Copy, Debug)]
pub struct Session {
    /// The adapter's PCI vendor.
    pub vendor: u32,
    /// Chromium runs on Wayland (X11 has no GBM device for shared buffers).
    pub wayland: bool,
    /// The wgpu device has the features for importing frames.
    pub can_import: bool,
}

/// Page frame sharing. `Auto` keeps the long-standing rule: shared GPU memory only on NVIDIA's
/// driver under Wayland (RADV currently faults on import, see plan.md).
pub fn frame_mode(hardware: &Hardware, env: &Overrides, session: Session) -> (FrameMode, Source) {
    if !session.can_import {
        return (FrameMode::Software, Source::Unsupported);
    }
    match env.osr.as_deref().map(str::trim) {
        Some("software") => return (FrameMode::Software, Source::Environment),
        Some("gpu") => return (FrameMode::Gpu, Source::Environment),
        _ => {}
    }
    if !hardware.enabled {
        return (FrameMode::Software, Source::Setting);
    }
    match hardware.page_frames {
        PageFrames::Gpu => (FrameMode::Gpu, Source::Setting),
        PageFrames::Software => (FrameMode::Software, Source::Setting),
        PageFrames::Auto => {
            let gpu = session.vendor == NVIDIA_VENDOR_ID && session.wayland;
            (if gpu { FrameMode::Gpu } else { FrameMode::Software }, Source::Automatic)
        }
    }
}

/// Whether Chromium's GPU process is pinned to our adapter's render node.
pub fn pin_render_node(hardware: &Hardware) -> bool {
    hardware.enabled
}

/// VA-API video decoding: needs the master switch too.
pub fn video_decoding(config: &Config) -> bool {
    config.hardware.enabled && config.media.hardware_decoding
}

/// A Chromium switch: name and optional value.
pub type Switch = (String, Option<String>);

/// The switches the settings add to Chromium's command line (the video decoding features are
/// added by the engine from [`video_decoding`]).
///
/// With GPU rasterization on, Chromium decides itself (it uses the GPU wherever the driver
/// allows); forcing it on would override its driver blocklist, so only "off" adds switches.
pub fn chromium_switches(hardware: &Hardware) -> Vec<Switch> {
    let flag = |name: &str| (name.to_owned(), None);
    if !hardware.enabled {
        return vec![flag("disable-gpu"), flag("disable-gpu-compositing")];
    }
    if !hardware.gpu_rasterization {
        return vec![
            flag("disable-gpu-rasterization"),
            flag("disable-accelerated-2d-canvas"),
            flag("disable-zero-copy"),
        ];
    }
    Vec::new()
}

/// AMD's open driver (RADV), where shared GPU memory can crash the driver.
pub fn is_amd(vendor: u32) -> bool {
    vendor == AMD_VENDOR_ID
}

/// Picking "Shared GPU memory" on AMD needs a confirmation first.
pub fn needs_gpu_frames_confirmation(vendor: u32, choice: PageFrames) -> bool {
    is_amd(vendor) && choice == PageFrames::Gpu
}

/// What the running process was started with: the settings only apply after a restart.
#[derive(Clone, Debug, PartialEq)]
pub struct Startup {
    pub hardware: Hardware,
    pub video_decoding: bool,
}

impl Startup {
    pub fn of(config: &Config) -> Self {
        Self { hardware: config.hardware.clone(), video_decoding: config.media.hardware_decoding }
    }

    /// The settings differ from what this process started with.
    pub fn needs_restart(&self, config: &Config) -> bool {
        *self != Self::of(config)
    }
}

/// One graphics card as wgpu lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterLine {
    pub name: String,
    pub backend: String,
    pub kind: &'static str,
    pub driver: String,
    pub vendor: u32,
    pub device: u32,
}

impl AdapterLine {
    pub fn of(info: &wgpu::AdapterInfo) -> Self {
        Self {
            name: info.name.clone(),
            backend: format!("{:?}", info.backend),
            kind: match info.device_type {
                wgpu::DeviceType::IntegratedGpu => "integrated",
                wgpu::DeviceType::DiscreteGpu => "discrete",
                wgpu::DeviceType::VirtualGpu => "virtual",
                wgpu::DeviceType::Cpu => "software",
                wgpu::DeviceType::Other => "other",
            },
            driver: if info.driver_info.is_empty() { info.driver.clone() } else { info.driver_info.clone() },
            vendor: info.vendor,
            device: info.device,
        }
    }

    /// "AMD Radeon 610M (Vulkan, integrated)".
    pub fn summary(&self) -> String {
        format!("{} ({}, {})", self.name, self.backend, self.kind)
    }
}

/// What is actually active in this process (the Hardware page and the diagnostics).
#[derive(Clone, Debug)]
pub struct Status {
    pub adapters: Vec<AdapterLine>,
    pub in_use: AdapterLine,
    pub frames: FrameMode,
    pub frames_source: Source,
    pub video_decoding: bool,
    pub cef_version: String,
    pub wayland: bool,
    pub env: Overrides,
    pub startup: Startup,
}

impl Status {
    /// The plain-text summary "Copy diagnostics" puts on the clipboard.
    pub fn diagnostics(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let line = |out: &mut String, text: String| {
            let _ = writeln!(out, "{text}");
        };
        line(&mut out, format!("Axion {}", env!("CARGO_PKG_VERSION")));
        line(&mut out, format!("Graphics card in use: {}", self.in_use.summary()));
        line(
            &mut out,
            format!(
                "Driver: {} (vendor 0x{:04X}, device 0x{:04X})",
                self.in_use.driver, self.in_use.vendor, self.in_use.device
            ),
        );
        for (i, adapter) in self.adapters.iter().enumerate() {
            line(&mut out, format!("Adapter {}: {}", i + 1, adapter.summary()));
        }
        let mode = match self.frames {
            FrameMode::Gpu => "shared GPU memory",
            FrameMode::Software => "software (system memory)",
        };
        let source = match self.frames_source {
            Source::Environment => "AXION_OSR",
            Source::Setting => "settings",
            Source::Automatic => "automatic",
            Source::Unsupported => "device cannot import frames",
        };
        line(&mut out, format!("Page frames: {mode} ({source})"));
        line(
            &mut out,
            format!(
                "Video decoding (VA-API features requested): {}",
                if self.video_decoding { "yes" } else { "no" }
            ),
        );
        let h = &self.startup.hardware;
        line(
            &mut out,
            format!(
                "Settings at start: hardware acceleration {}, page rendering {:?}, graphics card {:?}, GPU rasterization {}",
                if h.enabled { "on" } else { "off" },
                h.page_frames,
                h.gpu,
                if h.gpu_rasterization { "on" } else { "off" },
            ),
        );
        if let Some(gpu) = &self.env.gpu {
            line(&mut out, format!("AXION_GPU={gpu}"));
        }
        if let Some(osr) = &self.env.osr {
            line(&mut out, format!("AXION_OSR={osr}"));
        }
        line(&mut out, format!("Session: {}", if self.wayland { "Wayland" } else { "X11" }));
        line(&mut out, format!("Chromium Embedded Framework: {}", self.cef_version));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTEL_VENDOR_ID: u32 = 0x8086;

    fn env(gpu: Option<&str>, osr: Option<&str>) -> Overrides {
        Overrides { gpu: gpu.map(str::to_owned), osr: osr.map(str::to_owned) }
    }

    fn session(vendor: u32, wayland: bool) -> Session {
        Session { vendor, wayland, can_import: true }
    }

    #[test]
    fn automatic_frames_follow_the_nvidia_rule() {
        let hw = Hardware::default();
        let none = Overrides::default();
        assert_eq!(
            frame_mode(&hw, &none, session(NVIDIA_VENDOR_ID, true)),
            (FrameMode::Gpu, Source::Automatic)
        );
        assert_eq!(frame_mode(&hw, &none, session(NVIDIA_VENDOR_ID, false)).0, FrameMode::Software);
        assert_eq!(frame_mode(&hw, &none, session(AMD_VENDOR_ID, true)).0, FrameMode::Software);
        assert_eq!(frame_mode(&hw, &none, session(INTEL_VENDOR_ID, true)).0, FrameMode::Software);
    }

    #[test]
    fn the_setting_forces_a_mode_and_the_environment_wins() {
        let gpu = Hardware { page_frames: PageFrames::Gpu, ..Hardware::default() };
        let software = Hardware { page_frames: PageFrames::Software, ..Hardware::default() };
        assert_eq!(
            frame_mode(&gpu, &Overrides::default(), session(AMD_VENDOR_ID, true)),
            (FrameMode::Gpu, Source::Setting)
        );
        assert_eq!(
            frame_mode(&software, &Overrides::default(), session(NVIDIA_VENDOR_ID, true)).0,
            FrameMode::Software
        );
        assert_eq!(
            frame_mode(&gpu, &env(None, Some("software")), session(AMD_VENDOR_ID, true)),
            (FrameMode::Software, Source::Environment)
        );
        assert_eq!(
            frame_mode(&software, &env(None, Some("gpu")), session(NVIDIA_VENDOR_ID, true)),
            (FrameMode::Gpu, Source::Environment)
        );
        // An unknown value is ignored.
        assert_eq!(
            frame_mode(&software, &env(None, Some("x")), session(AMD_VENDOR_ID, true)).1,
            Source::Setting
        );
    }

    #[test]
    fn the_master_switch_turns_frame_sharing_off() {
        let off = Hardware { enabled: false, page_frames: PageFrames::Gpu, ..Hardware::default() };
        assert_eq!(
            frame_mode(&off, &Overrides::default(), session(NVIDIA_VENDOR_ID, true)),
            (FrameMode::Software, Source::Setting)
        );
        assert_eq!(
            frame_mode(&off, &env(None, Some("gpu")), session(NVIDIA_VENDOR_ID, true)).0,
            FrameMode::Gpu
        );
    }

    #[test]
    fn a_device_without_import_support_always_uses_software() {
        let s = Session { vendor: NVIDIA_VENDOR_ID, wayland: true, can_import: false };
        let hw = Hardware { page_frames: PageFrames::Gpu, ..Hardware::default() };
        assert_eq!(frame_mode(&hw, &env(None, Some("gpu")), s), (FrameMode::Software, Source::Unsupported));
    }

    #[test]
    fn graphics_card_choice_and_power_preference() {
        let mut hw = Hardware::default();
        let none = Overrides::default();
        assert_eq!(adapter_choice(&hw, &none), AdapterChoice::Auto);
        assert_eq!(power_preference(&AdapterChoice::Auto), wgpu::PowerPreference::HighPerformance);
        hw.gpu = GpuChoice::Low;
        assert_eq!(adapter_choice(&hw, &none), AdapterChoice::Low);
        assert_eq!(power_preference(&adapter_choice(&hw, &none)), wgpu::PowerPreference::LowPower);
        hw.gpu = GpuChoice::High;
        assert_eq!(power_preference(&adapter_choice(&hw, &none)), wgpu::PowerPreference::HighPerformance);
        // The environment wins, also by name.
        assert_eq!(adapter_choice(&hw, &env(Some("low"), None)), AdapterChoice::Low);
        assert_eq!(adapter_choice(&hw, &env(Some("Radeon"), None)), AdapterChoice::Name("radeon".into()));
        assert_eq!(
            adapter_choice(&Hardware { gpu: GpuChoice::Low, ..hw }, &env(Some("auto"), None)),
            AdapterChoice::Low
        );
    }

    #[test]
    fn adapters_are_picked_by_type_or_name() {
        use wgpu::DeviceType::{DiscreteGpu, IntegratedGpu};
        let list = vec![
            ("NVIDIA GeForce RTX 3060".to_owned(), DiscreteGpu),
            ("AMD Radeon 610M (RADV)".to_owned(), IntegratedGpu),
        ];
        assert_eq!(pick_adapter(&list, &AdapterChoice::Auto), None);
        assert_eq!(pick_adapter(&list, &AdapterChoice::Low), Some(1));
        assert_eq!(pick_adapter(&list, &AdapterChoice::High), Some(0));
        assert_eq!(pick_adapter(&list, &AdapterChoice::Name("radeon".into())), Some(1));
        assert_eq!(pick_adapter(&list, &AdapterChoice::Name("intel".into())), None);
        assert_eq!(pick_adapter(&list[..1], &AdapterChoice::Low), None);
    }

    #[test]
    fn chromium_switches_follow_the_settings() {
        assert!(chromium_switches(&Hardware::default()).is_empty());
        let off = chromium_switches(&Hardware { enabled: false, ..Hardware::default() });
        assert_eq!(off, [("disable-gpu".to_owned(), None), ("disable-gpu-compositing".to_owned(), None)]);
        let names: Vec<String> =
            chromium_switches(&Hardware { gpu_rasterization: false, ..Hardware::default() })
                .into_iter()
                .map(|(n, _)| n)
                .collect();
        assert_eq!(
            names,
            ["disable-gpu-rasterization", "disable-accelerated-2d-canvas", "disable-zero-copy"]
        );
    }

    #[test]
    fn video_decoding_needs_the_master_switch() {
        let mut config = Config::default();
        assert!(video_decoding(&config));
        config.media.hardware_decoding = false;
        assert!(!video_decoding(&config));
        config.media.hardware_decoding = true;
        config.hardware.enabled = false;
        assert!(!video_decoding(&config));
        assert!(!pin_render_node(&config.hardware));
    }

    #[test]
    fn amd_asks_before_shared_gpu_memory() {
        assert!(needs_gpu_frames_confirmation(AMD_VENDOR_ID, PageFrames::Gpu));
        assert!(!needs_gpu_frames_confirmation(AMD_VENDOR_ID, PageFrames::Auto));
        assert!(!needs_gpu_frames_confirmation(NVIDIA_VENDOR_ID, PageFrames::Gpu));
        assert!(!needs_gpu_frames_confirmation(INTEL_VENDOR_ID, PageFrames::Gpu));
    }

    #[test]
    fn restart_is_needed_only_for_changes_since_start() {
        let mut config = Config::default();
        let startup = Startup::of(&config);
        assert!(!startup.needs_restart(&config));
        config.hardware.gpu = GpuChoice::Low;
        assert!(startup.needs_restart(&config));
        config.hardware.gpu = GpuChoice::Auto;
        assert!(!startup.needs_restart(&config));
        config.media.hardware_decoding = false;
        assert!(startup.needs_restart(&config));
    }

    #[test]
    fn diagnostics_list_the_essentials() {
        let adapter = AdapterLine {
            name: "AMD Radeon 610M".into(),
            backend: "Vulkan".into(),
            kind: "integrated",
            driver: "Mesa 25".into(),
            vendor: AMD_VENDOR_ID,
            device: 0x164E,
        };
        let config = Config::default();
        let status = Status {
            adapters: vec![adapter.clone()],
            in_use: adapter,
            frames: FrameMode::Software,
            frames_source: Source::Automatic,
            video_decoding: true,
            cef_version: "156.0.14".into(),
            wayland: true,
            env: env(Some("low"), None),
            startup: Startup::of(&config),
        };
        let text = status.diagnostics();
        for needle in [
            "AMD Radeon 610M (Vulkan, integrated)",
            "Mesa 25",
            "software (system memory) (automatic)",
            "AXION_GPU=low",
            "156.0.14",
            "Wayland",
        ] {
            assert!(text.contains(needle), "{needle:?} missing in {text}");
        }
    }
}
