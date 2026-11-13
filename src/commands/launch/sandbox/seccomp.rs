//! Workload-only syscall restriction; filesystem and network namespaces are separate.
use std::io;

const fn stmt(code: u16, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}
const fn jump(k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: 0x15,
        jt,
        jf,
        k,
    }
}

/// Uses only stack memory and async-signal-safe syscalls, suitable for pre_exec.
pub(super) fn install() -> io::Result<()> {
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xc000003e;
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xc00000b7;
    const DENY: u32 = 0x00050000 | libc::EPERM as u32;
    const ALLOW: u32 = 0x7fff0000;
    let denied = [
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_pidfd_getfd,
        libc::SYS_kcmp,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_io_uring_setup,
        libc::SYS_open_by_handle_at,
    ];
    let mut filter = [stmt(0x06, DENY); 96];
    let mut n = 0;
    macro_rules! push {
        ($value:expr) => {{
            filter[n] = $value;
            n += 1;
        }};
    }
    push!(stmt(0x20, 4)); // seccomp_data.arch
    push!(jump(ARCH, 1, 0));
    push!(stmt(0x06, 0x80000000)); // KILL_PROCESS
    push!(stmt(0x20, 0));
    #[cfg(target_arch = "x86_64")]
    {
        push!(libc::sock_filter {
            code: 0x35,
            jt: 0,
            jf: 1,
            k: 0x40000000
        });
        push!(stmt(0x06, 0x00050000 | libc::ENOSYS as u32));
    }
    for syscall in denied {
        push!(jump(syscall as u32, 0, 1));
        push!(stmt(0x06, DENY));
    }
    push!(jump(libc::SYS_clone3 as u32, 0, 1));
    push!(stmt(0x06, 0x00050000 | libc::ENOSYS as u32));
    push!(jump(libc::SYS_clone as u32, 0, 4));
    push!(stmt(0x20, 16)); // arg0 flags
    push!(libc::sock_filter {
        code: 0x45,
        jt: 0,
        jf: 1,
        k: 0x7e020080
    });
    push!(stmt(0x06, DENY));
    push!(stmt(0x06, ALLOW));
    push!(jump(libc::SYS_ioctl as u32, 0, 6));
    push!(stmt(0x20, 24)); // arg1 request
    push!(jump(0x5412, 0, 1));
    push!(stmt(0x06, DENY));
    push!(jump(0x541c, 0, 1));
    push!(stmt(0x06, DENY));
    push!(stmt(0x06, ALLOW));
    push!(jump(libc::SYS_socket as u32, 1, 0));
    push!(jump(libc::SYS_socketpair as u32, 0, 5));
    push!(stmt(0x20, 16));
    push!(jump(libc::AF_INET as u32, 2, 0));
    push!(jump(libc::AF_INET6 as u32, 1, 0));
    push!(stmt(0x06, DENY));
    push!(stmt(0x06, ALLOW));
    push!(stmt(0x06, ALLOW));
    let program = libc::sock_fprog {
        len: n as u16,
        filter: filter.as_mut_ptr(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2, &program) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
