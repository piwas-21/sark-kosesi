//! The single-threaded child installs this filter before it reads untrusted input.
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule,
};
use std::{collections::BTreeMap, io};

fn condition(index: u8, op: SeccompCmpOp, value: u64) -> io::Result<SeccompCondition> {
    SeccompCondition::new(index, SeccompCmpArgLen::Dword, op, value).map_err(io::Error::other)
}

fn one(conditions: Vec<SeccompCondition>) -> io::Result<SeccompRule> {
    SeccompRule::new(conditions).map_err(io::Error::other)
}

pub(crate) fn install() -> io::Result<()> {
    let mut rules = BTreeMap::new();
    // Only anonymous, non-executable mappings, never mappings of inherited FDs.
    rules.insert(
        libc::SYS_mmap,
        vec![one(vec![
            condition(2, SeccompCmpOp::MaskedEq(libc::PROT_EXEC as u64), 0)?,
            condition(
                3,
                SeccompCmpOp::MaskedEq(libc::MAP_ANONYMOUS as u64),
                libc::MAP_ANONYMOUS as u64,
            )?,
        ])?],
    );
    rules.insert(
        libc::SYS_mprotect,
        vec![one(vec![condition(
            2,
            SeccompCmpOp::MaskedEq(libc::PROT_EXEC as u64),
            0,
        )?])?],
    );
    rules.insert(
        libc::SYS_read,
        vec![one(vec![condition(0, SeccompCmpOp::Eq, 0)?])?],
    );
    rules.insert(
        libc::SYS_write,
        vec![
            one(vec![condition(0, SeccompCmpOp::Eq, 1)?])?,
            one(vec![condition(0, SeccompCmpOp::Eq, 2)?])?,
        ],
    );
    for syscall in [
        libc::SYS_brk,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_futex,
        libc::SYS_sched_yield,
        libc::SYS_clock_gettime,
        libc::SYS_getrandom,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ] {
        rules.insert(syscall, vec![]);
    }
    let arch = std::env::consts::ARCH
        .try_into()
        .map_err(io::Error::other)?;
    let filter: BpfProgram = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        arch,
    )
    .map_err(io::Error::other)?
    .try_into()
    .map_err(io::Error::other)?;
    rustix::thread::set_no_new_privs(true).map_err(io::Error::other)?;
    seccompiler::apply_filter(&filter).map_err(io::Error::other)
}
