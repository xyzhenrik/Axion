//! What the stall watchdog (`crate::stall`) can learn about the stuck main thread from the
//! system: where the kernel has it waiting (`/proc`), and, when allowed, its stack through
//! a debugger (`eu-stack` or `gdb`).

use std::process::{Command, Stdio};
use std::time::Duration;

/// How long a debugger may take for the stack before it is given up.
const STACK_TIMEOUT: Duration = Duration::from_secs(10);

/// The main thread's id (on Linux the process id).
pub fn main_thread_id() -> u32 {
    std::process::id()
}

/// Where the kernel has thread `tid` of this process waiting, from `/proc` ("?" where it
/// can't be read).
pub fn kernel_wait(tid: u32) -> String {
    let base = format!("/proc/self/task/{tid}");
    let read = |name: &str| std::fs::read_to_string(format!("{base}/{name}")).ok();
    let state = read("stat").as_deref().and_then(thread_state).unwrap_or("?").to_owned();
    let wchan = read("wchan").filter(|w| !w.is_empty() && w != "0").unwrap_or_else(|| "-".to_owned());
    let syscall = read("syscall")
        .and_then(|s| s.split_whitespace().next().map(syscall_name))
        .unwrap_or_else(|| "?".to_owned());
    format!("thread state {state}, kernel wait {wchan}, syscall {syscall}")
}

/// The state letter of a `/proc/…/stat` line (after the command name, which may hold spaces).
fn thread_state(stat: &str) -> Option<&str> {
    stat.rsplit_once(')')?.1.split_whitespace().next()
}

/// A readable name for the common blocking syscalls (x86-64 numbers; others as numbers).
fn syscall_name(number: &str) -> String {
    if number == "running" {
        return "none (running)".to_owned();
    }
    if !cfg!(target_arch = "x86_64") {
        return number.to_owned();
    }
    let name = match number {
        "0" => "read",
        "7" => "poll",
        "23" => "select",
        "35" => "nanosleep",
        "45" => "recvfrom",
        "47" => "recvmsg",
        "61" => "wait4",
        "202" => "futex",
        "230" => "clock_nanosleep",
        "232" => "epoll_wait",
        "270" => "pselect6",
        "271" => "ppoll",
        "281" => "epoll_pwait",
        _ => return number.to_owned(),
    };
    name.to_owned()
}

/// Lets a debugger started by this process attach to it (Yama's `ptrace_scope` 1 allows only
/// a process's ancestors). Only with `AXION_STALL_STACK`: any process of the user may then
/// trace Axion, so it is a debugging setting.
pub fn allow_stack_capture() {
    #[cfg(target_os = "linux")]
    if let Err(e) = rustix::process::set_ptracer(rustix::process::PTracer::Any) {
        // No Yama (tracing is allowed anyway), or not permitted: the debugger may still work.
        tracing::debug!("stall: set_ptracer: {e}");
    }
}

/// The main thread's stack, read by `eu-stack` (elfutils) or else `gdb`, if either is
/// installed and may attach. Blocks the calling (watchdog) thread for up to a few seconds.
pub fn main_thread_stack() -> Option<String> {
    let pid = std::process::id().to_string();
    let tools: [(&str, Vec<&str>); 2] = [
        ("eu-stack", vec!["-1", "-p", &pid]),
        ("gdb", vec!["-p", &pid, "-batch", "-nx", "-ex", "thread 1", "-ex", "bt 40"]),
    ];
    for (tool, args) in tools {
        let child = Command::new(tool)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(child) = child else { continue };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        match rx.recv_timeout(STACK_TIMEOUT) {
            Ok(Ok(output)) if !output.stdout.is_empty() => {
                return Some(String::from_utf8_lossy(&output.stdout).into_owned());
            }
            Ok(_) => continue,
            Err(_) => {
                tracing::warn!("stall: {tool} took too long");
                return None;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_state_after_a_command_with_spaces() {
        assert_eq!(thread_state("1234 (axion (main)) S 1 2 3"), Some("S"));
        assert_eq!(thread_state("garbage"), None);
    }

    #[test]
    fn syscall_names() {
        assert_eq!(syscall_name("running"), "none (running)");
        assert_eq!(syscall_name("99999"), "99999");
        if cfg!(target_arch = "x86_64") {
            assert_eq!(syscall_name("202"), "futex");
        }
    }

    #[test]
    fn kernel_wait_reads_this_process() {
        assert!(kernel_wait(main_thread_id()).starts_with("thread state "));
    }
}
