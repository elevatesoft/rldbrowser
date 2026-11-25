use rustbrowser_core::SecurityProbe;
#[cfg(target_os = "macos")]
use rustbrowser_core::{SecurityFinding, Severity};

/// Returns the default runtime security probes for the current platform.
pub fn default_probes() -> Vec<Box<dyn SecurityProbe>> {
    #[cfg(target_os = "macos")]
    {
        vec![
            Box::new(DebuggerProbe),
            Box::new(InjectionProbe),
            Box::new(VmProbe),
            Box::new(DisplayProbe),
            Box::new(ScreenCaptureProcessProbe),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Prevents debuggers from attaching to the current process (`PT_DENY_ATTACH` on macOS).
///
/// No-op on non-macOS platforms.
pub fn deny_debugger_attach() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        const PT_DENY_ATTACH: libc::c_int = 31;
        // SAFETY: `ptrace(PT_DENY_ATTACH, 0, null, 0)` is the documented macOS syscall
        // to reject future debugger attachments on the calling process.
        let rc = unsafe { libc::ptrace(PT_DENY_ATTACH, 0, std::ptr::null_mut(), 0) };
        if rc == -1 {
            Err(format!(
                "ptrace(PT_DENY_ATTACH) failed: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos_probes {
    use std::ffi::{CStr, c_void};
    use std::path::Path;

    use super::*;

    const P_TRACED: libc::c_int = 0x0000_0800;

    const CRITICAL_HOOK_MARKERS: &[&str] =
        &["frida", "substrate", "cycript", "libinject", "gadget"];

    const SUSPICIOUS_DYLD_VARS: &[(&str, Severity)] = &[
        ("DYLD_INSERT_LIBRARIES", Severity::Critical),
        ("DYLD_FORCE_FLAT_NAMESPACE", Severity::Warning),
        ("DYLD_LIBRARY_PATH", Severity::Warning),
        ("DYLD_FRAMEWORK_PATH", Severity::Warning),
        ("DYLD_IMAGE_SUFFIX", Severity::Warning),
    ];

    const VM_MARKERS: &[&str] = &["virtualmac", "vmware", "parallels", "qemu", "virtualbox"];

    const SCREEN_CAPTURE_PROCESSES: &[&str] = &[
        "ScreenSharingAgent",
        "screensharingd",
        "screencapture",
        "ARDAgent",
        "OBS",
        "TeamViewer",
        "AnyDesk",
        "RustDesk",
        "QuickTime Player",
        "CleanShot",
        "Shottr",
        "ScreenFlow",
        "Loom",
        "zoom.us",
        "Discord",
        "Parsec",
        "ChromeRemoteDesktopHost",
        "vncserver",
    ];

    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(image_index: u32) -> *const libc::c_char;

        fn proc_listallpids(buffer: *mut c_void, buffersize: libc::c_int) -> libc::c_int;
        fn proc_pidpath(pid: libc::c_int, buffer: *mut c_void, buffersize: u32) -> libc::c_int;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetOnlineDisplayList(
            max_displays: u32,
            online_displays: *mut u32,
            display_count: *mut u32,
        ) -> i32;
        fn CGDisplayIsInMirrorSet(display: u32) -> u32;
    }

    #[repr(C, align(8))]
    struct KinfoProcBuffer {
        _head: [u8; 32],
        p_flag: libc::c_int,
        _tail: [u8; 648 - 32 - std::mem::size_of::<libc::c_int>()],
    }

    /// Detects whether a debugger is currently attached via `sysctl` (`P_TRACED`).
    pub struct DebuggerProbe;

    impl SecurityProbe for DebuggerProbe {
        fn name(&self) -> &'static str {
            "debugger"
        }

        fn check(&self) -> Vec<SecurityFinding> {
            let mut mib = [
                libc::CTL_KERN,
                libc::KERN_PROC,
                libc::KERN_PROC_PID,
                // SAFETY: `getpid()` has no preconditions and always succeeds.
                unsafe { libc::getpid() },
            ];
            let mut info = KinfoProcBuffer {
                _head: [0; 32],
                p_flag: 0,
                _tail: [0; 648 - 32 - std::mem::size_of::<libc::c_int>()],
            };
            let mut size = std::mem::size_of::<KinfoProcBuffer>();

            // SAFETY: `mib` has 4 elements, `info` is 648 bytes (matching `struct kinfo_proc`
            // on 64-bit macOS) and 8-byte aligned, `size` is initialized to its size, and
            // `newp` is null.
            let rc = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    4,
                    (&raw mut info).cast::<c_void>(),
                    &raw mut size,
                    std::ptr::null_mut(),
                    0,
                )
            };

            if rc == 0 && is_traced_flag(info.p_flag) {
                vec![SecurityFinding {
                    probe: self.name(),
                    severity: Severity::Critical,
                    message: format!(
                        "Process is being traced by a debugger (p_flag=0x{:x})",
                        info.p_flag
                    ),
                }]
            } else {
                Vec::new()
            }
        }
    }

    /// Detects `DYLD_*` environment injection and suspicious loaded dynamic libraries.
    pub struct InjectionProbe;

    impl SecurityProbe for InjectionProbe {
        fn name(&self) -> &'static str {
            "dylib-injection"
        }

        fn check(&self) -> Vec<SecurityFinding> {
            let mut findings = Vec::new();

            for &(var, severity) in SUSPICIOUS_DYLD_VARS {
                if let Some(val) = std::env::var_os(var)
                    && !val.is_empty()
                {
                    findings.push(SecurityFinding {
                        probe: self.name(),
                        severity,
                        message: format!(
                            "Suspicious environment variable set: {var}={}",
                            val.to_string_lossy()
                        ),
                    });
                }
            }

            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()));

            // SAFETY: `_dyld_image_count()` is thread-safe on macOS and returns the
            // current count of loaded Mach-O images.
            let count = unsafe { _dyld_image_count() };
            for idx in 0..count {
                // SAFETY: `idx < count` and `_dyld_get_image_name` returns either null
                // or a static/dyld-owned NUL-terminated C string.
                let ptr = unsafe { _dyld_get_image_name(idx) };
                if ptr.is_null() {
                    continue;
                }
                // SAFETY: `ptr` is non-null and NUL-terminated from dyld.
                let path = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
                if let Some((severity, reason)) = classify_dylib_path(&path, exe_dir.as_deref()) {
                    findings.push(SecurityFinding {
                        probe: self.name(),
                        severity,
                        message: format!("{reason}: {path}"),
                    });
                }
            }

            findings
        }
    }

    /// Detects virtualized execution environments via `sysctlbyname`.
    pub struct VmProbe;

    impl SecurityProbe for VmProbe {
        fn name(&self) -> &'static str {
            "vm"
        }

        fn check(&self) -> Vec<SecurityFinding> {
            let mut findings = Vec::new();

            if sysctl_int(c"kern.hv_vmm_present") == Some(1) {
                findings.push(SecurityFinding {
                    probe: self.name(),
                    severity: Severity::Critical,
                    message: "Hypervisor VMM detected (kern.hv_vmm_present=1)".to_string(),
                });
            }

            for (key, label) in [
                (c"hw.model", "hw.model"),
                (c"machdep.cpu.brand_string", "machdep.cpu.brand_string"),
            ] {
                if let Some(value) = sysctl_string(key)
                    && let Some(marker) = has_vm_marker(&value)
                {
                    findings.push(SecurityFinding {
                        probe: self.name(),
                        severity: Severity::Critical,
                        message: format!(
                            "VM marker '{marker}' detected in {label}: {value}"
                        ),
                    });
                }
            }

            findings
        }
    }

    /// Detects multiple or mirrored displays via CoreGraphics
    /// (`CGGetOnlineDisplayList` and `CGDisplayIsInMirrorSet`).
    pub struct DisplayProbe;

    impl SecurityProbe for DisplayProbe {
        fn name(&self) -> &'static str {
            "displays"
        }

        fn check(&self) -> Vec<SecurityFinding> {
            let mut count: u32 = 0;
            // SAFETY: Passing `0` and null for `online_displays` queries the online
            // display count into `count`.
            let err = unsafe { CGGetOnlineDisplayList(0, std::ptr::null_mut(), &raw mut count) };
            if err != 0 {
                return Vec::new();
            }

            let mut any_mirrored = false;
            if count > 0 {
                let mut displays = vec![0u32; count as usize];
                let mut fetched: u32 = 0;
                // SAFETY: `displays` has capacity for `count` `CGDirectDisplayID` entries.
                let err =
                    unsafe { CGGetOnlineDisplayList(count, displays.as_mut_ptr(), &raw mut fetched) };
                if err == 0 {
                    let used = (fetched as usize).min(displays.len());
                    for &display_id in &displays[..used] {
                        // SAFETY: `display_id` was returned by `CGGetOnlineDisplayList`.
                        if unsafe { CGDisplayIsInMirrorSet(display_id) } != 0 {
                            any_mirrored = true;
                            break;
                        }
                    }
                    return evaluate_display_state(used, any_mirrored)
                        .into_iter()
                        .collect();
                }
            }

            evaluate_display_state(count as usize, any_mirrored)
                .into_iter()
                .collect()
        }
    }

    /// Detects running screen-sharing, recording, or remote-desktop processes.
    pub struct ScreenCaptureProcessProbe;

    impl SecurityProbe for ScreenCaptureProcessProbe {
        fn name(&self) -> &'static str {
            "screen-capture"
        }

        fn check(&self) -> Vec<SecurityFinding> {
            // SAFETY: Calling `proc_listallpids(null, 0)` returns the number of active PIDs.
            let estimated = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
            if estimated <= 0 {
                return Vec::new();
            }

            let capacity = (estimated as usize).saturating_add(64);
            let mut pids: Vec<libc::pid_t> = vec![0; capacity];
            let buf_bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;

            // SAFETY: `pids` is valid for `buf_bytes` bytes.
            let actual = unsafe { proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), buf_bytes) };
            if actual <= 0 {
                return Vec::new();
            }

            let count = (actual as usize).min(pids.len());
            let mut findings = Vec::new();
            let mut path_buf = [0u8; 4096];

            for &pid in &pids[..count] {
                if pid <= 0 {
                    continue;
                }
                // SAFETY: `path_buf` is valid for `path_buf.len()` bytes.
                let len = unsafe {
                    proc_pidpath(
                        pid,
                        path_buf.as_mut_ptr().cast::<c_void>(),
                        path_buf.len() as u32,
                    )
                };
                if len <= 0 {
                    continue;
                }
                let path = String::from_utf8_lossy(&path_buf[..(len as usize).min(path_buf.len())]);
                if let Some(matched) = match_screen_capture_process(&path) {
                    findings.push(SecurityFinding {
                        probe: self.name(),
                        severity: Severity::Critical,
                        message: format!(
                            "Screen capture / remote desktop process detected ({matched}, pid={pid}): {path}"
                        ),
                    });
                }
            }

            findings
        }
    }

    pub(crate) fn is_traced_flag(p_flag: libc::c_int) -> bool {
        (p_flag & P_TRACED) != 0
    }

    pub(crate) fn classify_dylib_path(
        path: &str,
        exe_dir: Option<&str>,
    ) -> Option<(Severity, &'static str)> {
        let lower = path.to_ascii_lowercase();
        if CRITICAL_HOOK_MARKERS.iter().any(|m| lower.contains(m)) {
            return Some((Severity::Critical, "Suspicious hook library loaded"));
        }

        let is_system = path.starts_with("/System/")
            || path.starts_with("/usr/lib/")
            || path.starts_with("/Library/Apple/");
        if is_system {
            return None;
        }

        let is_in_exe_dir = exe_dir.is_some_and(|dir| path.starts_with(dir));
        #[cfg(debug_assertions)]
        let is_in_target_dir = path.contains("/target/");
        #[cfg(not(debug_assertions))]
        let is_in_target_dir = false;
        if is_in_exe_dir || is_in_target_dir {
            return None;
        }

        Some((Severity::Warning, "Non-system library loaded"))
    }

    pub(crate) fn has_vm_marker(value: &str) -> Option<&'static str> {
        let lower = value.to_ascii_lowercase();
        VM_MARKERS
            .iter()
            .copied()
            .find(|&marker| lower.contains(marker))
    }

    pub(crate) fn evaluate_display_count(count: u32) -> Option<SecurityFinding> {
        if count > 1 {
            Some(SecurityFinding {
                probe: "displays",
                severity: Severity::Critical,
                message: format!("Multiple active displays detected ({count})"),
            })
        } else {
            None
        }
    }

    pub(crate) fn evaluate_display_state(
        online_count: usize,
        any_mirrored: bool,
    ) -> Option<SecurityFinding> {
        if any_mirrored {
            return Some(SecurityFinding {
                probe: "displays",
                severity: Severity::Critical,
                message: format!(
                    "Mirrored display detected (online_displays={online_count})"
                ),
            });
        }
        evaluate_display_count(u32::try_from(online_count).unwrap_or(u32::MAX))
    }

    pub(crate) fn match_screen_capture_process(path: &str) -> Option<&'static str> {
        let file_name = Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(path);
        let file_lower = file_name.to_ascii_lowercase();

        for &tool in SCREEN_CAPTURE_PROCESSES {
            let tool_lower = tool.to_ascii_lowercase();
            if matches_tool_name(&file_lower, &tool_lower) {
                return Some(tool);
            }
        }

        // Also check `.app` bundle directory names along the path.
        for component in Path::new(path).components() {
            if let Some(comp_str) = component.as_os_str().to_str()
                && let Some(app_stem) = comp_str.strip_suffix(".app")
            {
                let stem_lower = app_stem.to_ascii_lowercase();
                for &tool in SCREEN_CAPTURE_PROCESSES {
                    let tool_lower = tool.to_ascii_lowercase();
                    if matches_tool_name(&stem_lower, &tool_lower) {
                        return Some(tool);
                    }
                }
            }
        }

        None
    }

    fn matches_tool_name(candidate_lower: &str, tool_lower: &str) -> bool {
        if candidate_lower == tool_lower {
            return true;
        }
        if let Some(rest) = candidate_lower.strip_prefix(tool_lower) {
            return rest.starts_with(' ') || rest.starts_with('-') || rest.starts_with('_');
        }
        false
    }

    fn sysctl_int(name: &CStr) -> Option<libc::c_int> {
        let mut value: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>();
        // SAFETY: `name` is NUL-terminated, `value` is sized to `size`, and `newp` is null.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&raw mut value).cast::<c_void>(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 { Some(value) } else { None }
    }

    fn sysctl_string(name: &CStr) -> Option<String> {
        let mut size: usize = 0;
        // SAFETY: Querying buffer size with null `oldp` is valid for `sysctlbyname`.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                std::ptr::null_mut(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 || size == 0 {
            return None;
        }

        let mut buf = vec![0u8; size];
        // SAFETY: `buf` has `size` bytes allocated.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buf.as_mut_ptr().cast::<c_void>(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return None;
        }

        let used = size.min(buf.len());
        let trimmed = buf[..used]
            .split(|&b| b == 0)
            .next()
            .unwrap_or_default();
        Some(String::from_utf8_lossy(trimmed).into_owned())
    }
}

#[cfg(target_os = "macos")]
pub use macos_probes::{
    DebuggerProbe, DisplayProbe, InjectionProbe, ScreenCaptureProcessProbe, VmProbe,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_probes_returns_expected_probe_names() {
        let probes = default_probes();
        let names: Vec<&str> = probes.iter().map(|p| p.name()).collect();

        #[cfg(target_os = "macos")]
        assert_eq!(
            names,
            vec![
                "debugger",
                "dylib-injection",
                "vm",
                "displays",
                "screen-capture"
            ]
        );

        #[cfg(not(target_os = "macos"))]
        assert!(names.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_probes_smoke_test_does_not_panic() {
        for probe in default_probes() {
            let _findings = probe.check();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn is_traced_flag_detects_p_traced_bit() {
        assert!(!macos_probes::is_traced_flag(0));
        assert!(!macos_probes::is_traced_flag(0x0000_0004));
        assert!(macos_probes::is_traced_flag(0x0000_0800));
        assert!(macos_probes::is_traced_flag(0x0000_0804));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn classify_dylib_path_flags_hooks_and_non_system_libs() {
        assert_eq!(
            macos_probes::classify_dylib_path("/tmp/frida-gadget.dylib", Some("/app/bin")),
            Some((Severity::Critical, "Suspicious hook library loaded"))
        );
        assert_eq!(
            macos_probes::classify_dylib_path("/usr/local/lib/libinject.dylib", None),
            Some((Severity::Critical, "Suspicious hook library loaded"))
        );
        assert_eq!(
            macos_probes::classify_dylib_path("/opt/homebrew/lib/libfoo.dylib", Some("/app/bin")),
            Some((Severity::Warning, "Non-system library loaded"))
        );
        assert_eq!(
            macos_probes::classify_dylib_path(
                "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation",
                None
            ),
            None
        );
        assert_eq!(
            macos_probes::classify_dylib_path("/usr/lib/libSystem.B.dylib", None),
            None
        );
        assert_eq!(
            macos_probes::classify_dylib_path(
                "/Library/Apple/System/Library/PrivateFrameworks/Foo.framework/Foo",
                None
            ),
            None
        );
        assert_eq!(
            macos_probes::classify_dylib_path("/app/bin/libhelper.dylib", Some("/app/bin")),
            None
        );
        #[cfg(debug_assertions)]
        assert_eq!(
            macos_probes::classify_dylib_path(
                "/Users/dev/repo/target/debug/deps/libtest.dylib",
                None
            ),
            None
        );
        #[cfg(not(debug_assertions))]
        assert_eq!(
            macos_probes::classify_dylib_path(
                "/Users/dev/repo/target/release/deps/libtest.dylib",
                None
            ),
            Some((Severity::Warning, "Non-system library loaded"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn has_vm_marker_matches_case_insensitively() {
        assert_eq!(macos_probes::has_vm_marker("VirtualMac2,1"), Some("virtualmac"));
        assert_eq!(macos_probes::has_vm_marker("VMware20,1"), Some("vmware"));
        assert_eq!(
            macos_probes::has_vm_marker("Parallels Virtual Platform"),
            Some("parallels")
        );
        assert_eq!(macos_probes::has_vm_marker("QEMU Virtual CPU"), Some("qemu"));
        assert_eq!(macos_probes::has_vm_marker("VirtualBox"), Some("virtualbox"));
        assert_eq!(macos_probes::has_vm_marker("MacBookPro18,1"), None);
        assert_eq!(macos_probes::has_vm_marker("Apple M2 Max"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn evaluate_display_count_flags_only_multiple_displays() {
        assert_eq!(macos_probes::evaluate_display_count(0), None);
        assert_eq!(macos_probes::evaluate_display_count(1), None);
        let finding = macos_probes::evaluate_display_count(2).expect("2 displays should alert");
        assert_eq!(finding.probe, "displays");
        assert_eq!(finding.severity, Severity::Critical);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn evaluate_display_state_flags_multiple_and_mirrored_displays() {
        assert_eq!(macos_probes::evaluate_display_state(1, false), None);

        let multi = macos_probes::evaluate_display_state(2, false)
            .expect("2 online displays should alert");
        assert_eq!(multi.probe, "displays");
        assert_eq!(multi.severity, Severity::Critical);

        let mirrored = macos_probes::evaluate_display_state(1, true)
            .expect("1 active mirrored display should alert");
        assert_eq!(mirrored.probe, "displays");
        assert_eq!(mirrored.severity, Severity::Critical);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn match_screen_capture_process_detects_known_tools_without_false_positives() {
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/Applications/OBS.app/Contents/MacOS/OBS"
            ),
            Some("OBS")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/Applications/CleanShot X.app/Contents/MacOS/CleanShot X"
            ),
            Some("CleanShot")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/MacOS/ARDAgent"
            ),
            Some("ARDAgent")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process("/usr/libexec/screensharingd"),
            Some("screensharingd")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/System/Library/CoreServices/RemoteManagement/ScreenSharingAgent.app/Contents/MacOS/ScreenSharingAgent"
            ),
            Some("ScreenSharingAgent")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process("/usr/sbin/screencapture"),
            Some("screencapture")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/Applications/zoom.us.app/Contents/MacOS/zoom.us"
            ),
            Some("zoom.us")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/Applications/Discord.app/Contents/MacOS/Discord"
            ),
            Some("Discord")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/Applications/Parsec.app/Contents/MacOS/parsecd"
            ),
            Some("Parsec")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/Library/PrivilegedHelperTools/ChromeRemoteDesktopHost.app/Contents/MacOS/remoting_me2me_host"
            ),
            Some("ChromeRemoteDesktopHost")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process("/usr/local/bin/vncserver"),
            Some("vncserver")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process(
                "/Applications/QuickTime Player.app/Contents/MacOS/QuickTime Player"
            ),
            Some("QuickTime Player")
        );
        assert_eq!(
            macos_probes::match_screen_capture_process("/usr/bin/observer"),
            None
        );
        assert_eq!(
            macos_probes::match_screen_capture_process("/Applications/Safari.app/Contents/MacOS/Safari"),
            None
        );
    }
}
