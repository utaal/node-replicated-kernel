//! This module provides a very basic address-space management
//! API from user-space and implements a [`core::alloc::GlobalAlloc`]
//! type for doing memory allocation in user-space.

use core::alloc::{GlobalAlloc, Layout};
use core::mem::transmute;

use log::{debug, error, info};
use spin::Mutex;
use x86::current::paging::{PAddr, VAddr};

use kpi::SystemCallError;

use slabmalloc::{ObjectPage, PageProvider, ZoneAllocator};

macro_rules! round_up {
    ($num:expr, $s:expr) => {
        (($num + $s - 1) / $s) * $s
    };
}

/// A static reference to our silly pager.
pub static PAGER: Mutex<Pager> = Mutex::new(Pager(0xddabfff000));

/// A silly pager.
pub struct Pager(u64);

impl<'a> PageProvider<'a> for Pager {
    /// Allocates a page for use with slabmalloc.
    fn allocate_page(&mut self) -> Option<&'a mut ObjectPage<'a>> {
        unsafe {
            let r = crate::vspace(crate::VSpaceOperation::Map, self.0, 0x1000);
            let sp: &'a mut ObjectPage = transmute(self.0);
            self.0 += 0x1000;
            Some(sp)
        }
    }

    /// Releases a page back to slabmalloc.
    fn release_page(&mut self, page: &'a mut ObjectPage<'a>) {}
}

impl Pager {
    pub(crate) fn allocate_new(
        &mut self,
        layout: Layout,
    ) -> Result<(VAddr, PAddr), SystemCallError> {
        let size = round_up!(layout.size(), 4096) as u64;
        self.0 = round_up!(self.0 as usize, core::cmp::max(layout.align(), 4096)) as u64;

        unsafe { crate::vspace(crate::VSpaceOperation::Map, self.0, size) }
    }

    /// Allocates an arbitray layout (> 4K) in the address space.
    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        //debug!("layout {:?}", layout);

        let size = round_up!(layout.size(), 4096) as u64;
        self.0 = round_up!(self.0 as usize, core::cmp::max(layout.align(), 4096)) as u64;

        unsafe {
            let r = crate::vspace(crate::VSpaceOperation::Map, self.0, size);
            let sp: *mut u8 = transmute(self.0);
            self.0 += size;
            sp
        }
    }
}

/// The SafeZoneAllocator is just a Mutex wrapped version
/// of the [`slabmalloc::ZoneAllocator`].
pub struct SafeZoneAllocator(Mutex<ZoneAllocator<'static>>);

impl SafeZoneAllocator {
    pub const fn new(provider: &'static Mutex<PageProvider>) -> SafeZoneAllocator {
        SafeZoneAllocator(Mutex::new(ZoneAllocator::new(provider)))
    }
}

unsafe impl GlobalAlloc for SafeZoneAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() <= ZoneAllocator::MAX_ALLOC_SIZE {
            self.0.lock().allocate(layout)
        } else {
            PAGER.lock().allocate(layout)
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.size() <= ZoneAllocator::MAX_ALLOC_SIZE {
            self.0.lock().deallocate(ptr, layout);
        } else {
            //panic!("NYI dealloc");
            //error!("NYI dealloc");
        }
    }
}
