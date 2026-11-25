use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};

pub const CHILD_ENV: &str = "CORRA_RUNNER_REAPED";

static CHILD: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward(sig: libc::c_int) {
    let pid = CHILD.load(Ordering::Relaxed);
    if pid > 0 {
        unsafe { libc::kill(pid, sig) };
    }
}

pub fn supervise(mut cmd: Command) -> std::io::Result<i32> {
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let pid = cmd.spawn()?.id() as i32;
    CHILD.store(pid, Ordering::Relaxed);
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT] {
        unsafe { libc::signal(sig, forward as *const () as libc::sighandler_t) };
    }
    loop {
        let mut status = 0;
        let got = unsafe { libc::waitpid(-1, &mut status, 0) };
        if got == pid {
            return Ok(if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else {
                128 + libc::WTERMSIG(status)
            });
        }
        if got < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EINTR) {
                return Err(e);
            }
        }
    }
}
