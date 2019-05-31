use core::mem;
use core::ops::Deref;

use x86::bits64::paging::{PAddr, VAddr};
use x86::bits64::rflags::{self, RFlags};
use x86::msr::{rdmsr, wrmsr, IA32_EFER, IA32_FMASK, IA32_LSTAR, IA32_STAR};
use x86::segmentation::SegmentSelector;
use x86::tlb;
use x86::Ring;

use kpi::*;

use crate::error::KError;

use super::process::{Process, CURRENT_PROCESS};
use super::vspace;
use crate::prelude::NoDrop;

extern "C" {
    #[no_mangle]
    fn syscall_enter();
}

struct UserValue<T> {
    value: T,
}

impl<T> UserValue<T> {
    fn new(pointer: T) -> UserValue<T> {
        UserValue { value: pointer }
    }
}

impl<T> Deref for UserValue<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe {
            rflags::stac();
            &self.value
        }
    }
}

impl<T> Drop for UserValue<T> {
    fn drop(&mut self) {
        unsafe { rflags::clac() };
    }
}

/// System call handler for printing
fn process_print(buf: UserValue<&str>) -> Result<(), KError> {
    let buffer: &str = *buf;
    sprint!("{}", buffer);
    Ok(())
}

/// System call handler for process exit
fn process_exit(code: u64) -> Result<(), KError> {
    info!("Process got exit, we are done for now...");
    super::debug::shutdown(crate::ExitReason::Ok);
    Ok(())
}

fn handle_process(arg1: u64, arg2: u64, arg3: u64) -> Result<(), KError> {
    let op = ProcessOperation::from(arg1);
    debug!("{:?} {:#x} {:#x}", op, arg2, arg3);

    match op {
        ProcessOperation::Log => {
            let buffer: *const u8 = arg2 as *const u8;
            let len: usize = arg3 as usize;

            let user_str = unsafe {
                let slice = core::slice::from_raw_parts(buffer, len);
                core::str::from_utf8_unchecked(slice)
            };

            process_print(UserValue::new(user_str))
        }
        ProcessOperation::Exit => {
            let exit_code = arg2;
            process_exit(arg1)
        }
        _ => Err(KError::InvalidProcessOperation { a: arg1 }),
    }
}

/// System call handler for vspace operations
fn handle_vspace(arg1: u64, arg2: u64, arg3: u64) -> Result<(), KError> {
    let op = VSpaceOperation::from(arg1);
    let base = VAddr::from(arg2);
    let bound = arg3;
    debug!("{:?} {:#x} {:#x}", op, base, bound);

    match op {
        VSpaceOperation::Map => unsafe {
            let mut plock = CURRENT_PROCESS.lock();
            (*plock)
                .as_mut()
                .map_or(Err(KError::ProcessNotSet), |ref mut p| {
                    let (paddr, size) = (*p).vspace.map(
                        base,
                        bound as usize,
                        vspace::MapAction::ReadWriteUser,
                        0x1000,
                    )?;
                    tlb::flush_all();
                    (*p).save_area.set_syscall_ret1(paddr.as_u64());
                    (*p).save_area.set_syscall_ret2(size as u64);
                    Ok(())
                })
        },
        VSpaceOperation::MapDevice => unsafe {
            let mut plock = CURRENT_PROCESS.lock();

            (*plock)
                .as_mut()
                .map_or(Err(KError::ProcessNotSet), |ref mut p| {
                    let paddr = PAddr::from(base.as_u64());

                    (*p).vspace.map_generic(
                        base,
                        (paddr, bound as usize),
                        vspace::MapAction::ReadWriteUser,
                    )?;

                    tlb::flush_all();
                    (*p).save_area.set_syscall_ret1(paddr.as_u64());
                    (*p).save_area.set_syscall_ret2(bound);

                    Ok(())
                })
        },
        VSpaceOperation::Unmap => {
            error!("Can't do VSpaceOperation unmap yet.");
            Err(KError::NotSupported)
        }
        VSpaceOperation::Identify => unsafe {
            trace!("Identify base {:#x}.", base);
            let mut plock = CURRENT_PROCESS.lock();
            (*plock)
                .as_mut()
                .map_or(Err(KError::ProcessNotSet), |ref mut p| {
                    let paddr = (*p).vspace.resolve_addr(base);

                    (*p).save_area
                        .set_syscall_ret1(paddr.map(|p| p.as_u64()).unwrap_or(0x0));
                    (*p).save_area.set_syscall_ret2(0x0);

                    Ok(())
                })
        },
        VSpaceOperation::Unknown => {
            error!("Got an invalid VSpaceOperation code.");
            Err(KError::InvalidVSpaceOperation { a: arg1 })
        }
    }
}

#[inline(never)]
#[no_mangle]
pub extern "C" fn syscall_handle(
    function: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> u64 {
    let status: Result<(), KError> = match SystemCall::new(function) {
        SystemCall::Process => handle_process(arg1, arg2, arg3),
        SystemCall::VSpace => handle_vspace(arg1, arg2, arg3),
        _ => Err(KError::InvalidSyscallArgument1 { a: function }),
    };

    let retcode = match status {
        Ok(()) => SystemCallError::Ok,
        Err(status) => {
            error!("System call returned with error: {:?}", status);
            status.into()
        }
    };

    retcode as u64
}

/// Enables syscall/sysret functionality.
pub fn enable_fast_syscalls(cs_selector: SegmentSelector, ss_selector: SegmentSelector) {
    unsafe {
        let mut star = rdmsr(IA32_STAR);
        star |= (cs_selector.bits() as u64) << 32;
        star |= (ss_selector.bits() as u64) << 48;
        wrmsr(IA32_STAR, star);

        // System call RIP
        let rip = syscall_enter as u64;
        wrmsr(IA32_LSTAR, rip);
        info!("syscalls jump to {:#x}", rip);

        wrmsr(
            IA32_FMASK,
            !(rflags::RFlags::FLAGS_IOPL3 | rflags::RFlags::FLAGS_A1 | rflags::RFlags::FLAGS_IF)
                .bits(),
        );

        // Enable fast syscalls
        let efer = rdmsr(IA32_EFER) | 0b1;
        wrmsr(IA32_EFER, efer);
    }

    debug!("Fast syscalls enabled!");
}
