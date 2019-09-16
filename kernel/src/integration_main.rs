#[cfg(all(feature = "integration-test", feature = "test-time"))]
pub fn xmain() {
    unsafe {
        let tsc = x86::time::rdtsc();
        let tsc2 = x86::time::rdtsc();

        let start = rawtime::Instant::now();
        let done = start.elapsed().as_nanos();
        // We do this twice because I think it traps the first time?
        let start = rawtime::Instant::now();
        let done = start.elapsed().as_nanos();
        sprintln!("rdtsc overhead: {:?} cycles", tsc2 - tsc);
        sprintln!("Instant overhead: {:?} ns", done);

        if cfg!(debug_assertions) {
            assert!(tsc2 - tsc <= 100, "rdtsc overhead big?");
            // TODO: should be less:
            assert!(done <= 100, "Instant overhead big?");
        } else {
            assert!(tsc2 - tsc <= 50);
            // TODO: should be less:
            assert!(done <= 100);
        }
    }
    arch::debug::shutdown(ExitReason::Ok);
}

#[cfg(all(feature = "integration-test", feature = "test-buddy"))]
pub fn xmain() {
    use buddy::FreeBlock;
    use buddy::Heap;
    let mut heap = Heap::new(
        heap_base: *mut u8,
        heap_size: usize,
        free_lists: &mut [*mut FreeBlock],
    );

    let b = heap.allocate(4096, 4096);

    arch::debug::shutdown(ExitReason::Ok);
}

#[cfg(all(feature = "integration-test", feature = "test-exit"))]
pub fn xmain() {
    arch::debug::shutdown(ExitReason::Ok);
}

#[cfg(all(feature = "integration-test", feature = "test-pfault"))]
#[inline(never)]
pub fn xmain() {
    use arch::memory::{paddr_to_kernel_vaddr, PAddr};

    unsafe {
        let paddr = PAddr::from(0xdeadbeefu64);
        let kernel_vaddr = paddr_to_kernel_vaddr(paddr);
        let ptr: *mut u64 = kernel_vaddr.as_mut_ptr();
        debug!("before causing the pfault");
        let val = *ptr;
        assert!(val != 0);
    }
}

#[cfg(all(feature = "integration-test", feature = "test-gpfault"))]
pub fn xmain() {
    // Note that int!(13) doesn't work in qemu. It doesn't push an error code properly for it.
    // So we cause a GP by loading garbage in the ss segment register.
    use x86::segmentation::{load_ss, SegmentSelector};
    unsafe {
        load_ss(SegmentSelector::new(99, x86::Ring::Ring3));
    }
}

#[cfg(all(feature = "integration-test", feature = "test-alloc"))]
pub fn xmain() {
    use alloc::vec::Vec;
    {
        let mut buf: Vec<u8> = Vec::with_capacity(0);
        for i in 0..1024 {
            buf.push(i as u8);
        }
    } // Make sure we drop here.
    info!("small allocations work.");

    {
        let size: usize = x86::bits64::paging::BASE_PAGE_SIZE;
        let mut buf: Vec<u8> = Vec::with_capacity(size);
        for i in 0..size {
            buf.push(i as u8);
        }

        let size: usize = x86::bits64::paging::BASE_PAGE_SIZE * 256;
        let mut buf: Vec<usize> = Vec::with_capacity(size);
        for i in 0..size {
            buf.push(i as usize);
        }
    } // Make sure we drop here.
    info!("large allocations work.");
    arch::debug::shutdown(ExitReason::Ok);
}

/// Checks that we can initialize ACPI and query the ACPI tables.
///
/// # Note
/// This test is supposed to spawn on a small machine with 1 socket, 2 cores
/// and no numa nodes.
#[cfg(all(feature = "integration-test", feature = "test-acpi-smoke"))]
pub fn xmain() {
    use topology::MACHINE_TOPOLOGY;

    // We have 2 cores ...
    assert_eq!(MACHINE_TOPOLOGY.num_threads(), 2);
    // ... no SMT ...
    assert_eq!(MACHINE_TOPOLOGY.num_cores(), 2);
    // ... 1 sockets ...
    assert_eq!(MACHINE_TOPOLOGY.num_packages(), 1);
    // ... no numa ...
    assert_eq!(MACHINE_TOPOLOGY.num_nodes(), 0);

    // ... and one IOAPIC which starts from GSI 0
    for (i, io_apic) in MACHINE_TOPOLOGY.io_apics().enumerate() {
        match i {
            0 => assert_eq!(io_apic.global_irq_base, 0, "GSI of I/O APIC is 0"),
            _ => assert_eq!(
                MACHINE_TOPOLOGY.io_apics().count(),
                1,
                "Found more than 1 IO APIC"
            ),
        };
    }

    arch::debug::shutdown(ExitReason::Ok);
}

/// Checks that we can initialize ACPI, query the ACPI tables
/// and construct a large NUMA topology.
///
/// # Note
/// This test is supposed to spawn on a topology with 2 sockets, 1 core each
/// 2 numa nodes (one per socket) with 512 MiB RAM each.
#[cfg(all(feature = "integration-test", feature = "test-acpi-topology"))]
pub fn xmain() {
    use topology::MACHINE_TOPOLOGY;

    // We have 80 cores ...
    assert_eq!(MACHINE_TOPOLOGY.num_threads(), 80);
    // ... no SMT ...
    assert_eq!(MACHINE_TOPOLOGY.num_cores(), 80);
    // ... 8 sockets ...
    assert_eq!(MACHINE_TOPOLOGY.num_packages(), 8);
    // ... on 8 numa-nodes ...
    assert_eq!(MACHINE_TOPOLOGY.num_nodes(), 8);

    // ... with 512 MiB of RAM per NUMA node ...
    for (nid, node) in MACHINE_TOPOLOGY.nodes().enumerate() {
        match nid {
            0 => assert_eq!(node.memory().count(), 2),
            _ => assert_eq!(node.memory().count(), 1),
        };

        let bytes_per_node: u64 = node.memory().map(|ma| ma.length).sum();

        if nid > 0 {
            assert_eq!(
                bytes_per_node,
                1024 * 1024 * 512,
                "Node#{} has 512 MiB of RAM",
                nid
            );
        } else {
            // First node has a bit less...
            assert!(
                bytes_per_node >= 1024 * 1024 * 511,
                "Node#0 has almost 512 MiB of RAM"
            );
        }
    }

    // ... and 10 cores per node ...
    for node in MACHINE_TOPOLOGY.nodes() {
        assert_eq!(node.cores().count(), 10);
    }

    // ... and 10 cores/threads per package ...
    for package in MACHINE_TOPOLOGY.packages() {
        assert_eq!(package.cores().count(), 10);
        assert_eq!(package.threads().count(), 10);
    }

    // ... and each core has 10 siblings ...
    for core in MACHINE_TOPOLOGY.cores() {
        assert_eq!(core.siblings().count(), 10);
    }

    // ... and one IOAPIC which starts from GSI 0
    for (i, io_apic) in MACHINE_TOPOLOGY.io_apics().enumerate() {
        match i {
            0 => assert_eq!(io_apic.global_irq_base, 0, "GSI of I/O APIC is 0"),
            _ => assert_eq!(
                MACHINE_TOPOLOGY.io_apics().count(),
                1,
                "Found more than 1 IO APIC"
            ),
        };
    }

    arch::debug::shutdown(ExitReason::Ok);
}

/// Tests core bring-up.
///
/// Boots a single core, checks we can print and pass correct arguments.
#[cfg(all(feature = "integration-test", feature = "test-coreboot-smoke"))]
pub fn xmain() {
    use arch::coreboot;
    use topology;
    use x86::apic::{ApicControl, ApicId};

    // A simple stack for the app core (non bootstrap core)
    static mut COREBOOT_STACK: [u8; 4096 * 32] = [0; 4096 * 32];

    // Entry point for app. This function is called from start_ap.S:
    pub extern "C" fn bespin_init_ap(
        arg1: *mut u64,
        arg2: *mut u64,
        arg3: *mut u64,
        initialized: *mut u64,
    ) {
        crate::arch::enable_sse();
        crate::arch::enable_fsgsbase();

        // Check that we can pass arguments:
        assert_eq!(arg1, 1 as *mut u64);
        assert_eq!(arg2, 2 as *mut u64);
        assert_eq!(arg3, 3 as *mut u64);

        // Don't change this string otherwise the test will fail:
        sprintln!("Hello from the other side");

        unsafe { core::ptr::write_volatile(initialized, 1) };
        loop {}
    }

    assert_eq!(topology::MACHINE_TOPOLOGY.num_threads(), 2, "No 2nd core?");

    let bsp_thread = topology::MACHINE_TOPOLOGY.current_thread();
    let thread_to_boot = topology::MACHINE_TOPOLOGY
        .threads()
        .find(|t| t != &bsp_thread)
        .expect("Didn't find an application core to boot...");

    unsafe {
        let mut initialized: u64 = 0;

        coreboot::initialize(
            thread_to_boot.apic_id(),
            bespin_init_ap,
            (
                1 as *mut u64,
                2 as *mut u64,
                3 as *mut u64,
                &mut initialized as *mut u64,
            ),
            &mut COREBOOT_STACK,
        );

        // Wait until core is up or we time out
        let mut is_initialized = 0;
        let timeout = x86::time::rdtsc() + 10_000_000;
        loop {
            // Did the core signal us initialization completed?
            is_initialized = core::ptr::read_volatile(&mut initialized);
            if is_initialized == 1 {
                break;
            }

            // Have we waited long enough?
            if x86::time::rdtsc() > timeout {
                break;
            }
        }

        if is_initialized == 1 {
            // Don't change this string otherwise the test will fail:
            info!("Core has started");
        } else {
            panic!("Core didn't boot properly...");
        }
    }

    arch::debug::shutdown(ExitReason::Ok);
}

/// Tests booting of a system of multiple cores and using the node-replication
/// log to communicate information.
#[cfg(all(feature = "integration-test", feature = "test-coreboot-nrlog"))]
pub fn xmain() {
    use arch::coreboot;
    use topology;
    use x86::apic::{ApicControl, ApicId};

    static mut COREBOOT_STACK: [u8; 4096 * 32] = [0; 4096 * 32];

    // Entry point for app. This function is called from start_ap.S:
    pub extern "C" fn bespin_init_ap(
        arg1: *mut u64,
        arg2: *mut u64,
        arg3: *mut u64,
        initialized: *mut u64,
    ) {
        crate::arch::enable_sse();
        crate::arch::enable_fsgsbase();

        // Check that we can pass arguments:
        assert_eq!(arg1, 1 as *mut u64);
        assert_eq!(arg2, 2 as *mut u64);
        assert_eq!(arg3, 3 as *mut u64);

        // Don't change this string otherwise the test will fail:
        sprintln!("Hello from the other side");

        unsafe { core::ptr::write_volatile(initialized, 1) };
        loop {}
    }

    assert_eq!(topology::MACHINE_TOPOLOGY.num_threads(), 4, "Need 4 cores");

    let bsp_thread = topology::MACHINE_TOPOLOGY.current_thread();
    let threads_to_boot = topology::MACHINE_TOPOLOGY
        .threads()
        .filter(|t| t != &bsp_thread);

    unsafe {
        for thread in threads_to_boot {
            let mut initialized: u64 = 0;

            coreboot::initialize(
                thread.apic_id(),
                bespin_init_ap,
                (
                    1 as *mut u64,
                    2 as *mut u64,
                    3 as *mut u64,
                    &mut initialized as *mut u64,
                ),
                &mut COREBOOT_STACK,
            );

            // Wait until core is up or we time out
            let mut is_initialized = 0;
            let timeout = x86::time::rdtsc() + 10_000_000;
            loop {
                // Did the core signal us initialization completed?
                is_initialized = core::ptr::read_volatile(&mut initialized);
                if is_initialized == 1 {
                    break;
                }

                // Have we waited long enough?
                if x86::time::rdtsc() > timeout {
                    break;
                }
            }

            if is_initialized == 1 {
                // Don't change this string otherwise the test will fail:
                info!("Core has started");
            } else {
                panic!("Core didn't boot properly...");
            }
        }
    }

    arch::debug::shutdown(ExitReason::Ok);
}

#[cfg(all(feature = "integration-test", feature = "test-scheduler"))]
pub fn xmain() {
    let cpuid = x86::cpuid::CpuId::new();
    assert!(
        cpuid
            .get_extended_feature_info()
            .map_or(false, |ef| ef.has_fsgsbase()),
        "FS/GS base instructions supported"
    );
    use lineup::tls::Environment;

    let mut s = lineup::Scheduler::new(lineup::DEFAULT_UPCALLS);
    s.spawn(
        4096,
        |arg| {
            let _r = Environment::thread().relinquish();
            info!("lwt1 {:?}", Environment::tid());
        },
        core::ptr::null_mut(),
    );

    s.spawn(
        4096,
        |arg| {
            info!("lwt2 {:?}", Environment::tid());
        },
        core::ptr::null_mut(),
    );

    s.run();
    s.run();
    s.run();
    s.run();

    arch::debug::shutdown(ExitReason::Ok);
}

#[cfg(all(feature = "integration-test", feature = "test-userspace"))]
pub fn xmain() {
    let init_module = kcb::try_get_kcb()
        .map(|kcb| kcb.kernel_args().modules[1].clone())
        .expect("Need to have an init module.");

    trace!("init {:?}", init_module);
    let mut process = alloc::boxed::Box::new(
        arch::process::Process::from(init_module).expect("Couldn't load init."),
    );

    info!("Created the init process, about to go there...");
    let no = kcb::get_kcb().swap_current_process(process);
    assert!(no.is_none());

    unsafe {
        let rh = kcb::get_kcb().current_process().as_mut().map(|p| p.start());
        rh.unwrap().resume();
    }

    arch::debug::shutdown(ExitReason::Ok);
}

#[cfg(all(feature = "integration-test", feature = "test-sse"))]
pub fn xmain() {
    info!("division = {}", 10.0 / 2.19);
    info!("division by zero = {}", 10.0 / 0.0);
    arch::debug::shutdown(ExitReason::Ok);
}
