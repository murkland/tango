use super::c;

#[repr(transparent)]
pub struct ARMCoreRef<'a> {
    pub(super) ptr: *const c::ARMCore,
    pub(super) _lifetime: std::marker::PhantomData<&'a ()>,
}

impl<'a> ARMCoreRef<'a> {
    pub fn gpr(&self, r: usize) -> i32 {
        unsafe { (*self.ptr).__bindgen_anon_1.__bindgen_anon_1.gprs[r] }
    }
}

#[repr(transparent)]
pub struct ARMCoreMutRef<'a> {
    pub(super) ptr: *mut c::ARMCore,
    pub(super) _lifetime: std::marker::PhantomData<&'a ()>,
}

impl<'a> ARMCoreMutRef<'a> {
    pub fn as_ref(&self) -> ARMCoreRef {
        ARMCoreRef {
            ptr: self.ptr,
            _lifetime: self._lifetime,
        }
    }

    pub unsafe fn components_mut(&self) -> &mut [*mut c::mCPUComponent] {
        std::slice::from_raw_parts_mut(
            (*self.ptr).components,
            c::mCPUComponentType_CPU_COMPONENT_MAX as usize,
        )
    }

    pub fn set_gpr(&self, r: usize, v: i32) {
        unsafe {
            (*self.ptr).__bindgen_anon_1.__bindgen_anon_1.gprs[r] = v;
        }
    }

    pub fn set_pc(&self, v: u32) {
        self.set_gpr(15, v as i32);
        self.thumb_write_pc();
    }

    fn thumb_write_pc(&self) {
        unsafe {
            // uint32_t pc = cpu->gprs[ARM_PC] & -WORD_SIZE_THUMB;
            let mut pc = (self.as_ref().gpr(c::ARM_PC as usize)
                & -(c::WordSize_WORD_SIZE_THUMB as i32)) as u32;

            // cpu->memory.setActiveRegion(cpu, pc);
            (*self.ptr).memory.setActiveRegion.unwrap()(self.ptr, pc as u32);

            // LOAD_16(cpu->prefetch[0], pc & cpu->memory.activeMask, cpu->memory.activeRegion);
            (*self.ptr).prefetch[0] = *(((*self.ptr).memory.activeRegion as *const u8)
                .add((pc & (*self.ptr).memory.activeMask) as usize)
                as *const u16) as u32;

            // pc += WORD_SIZE_THUMB;
            pc += c::WordSize_WORD_SIZE_THUMB;

            // LOAD_16(cpu->prefetch[1], pc & cpu->memory.activeMask, cpu->memory.activeRegion);
            (*self.ptr).prefetch[1] = *(((*self.ptr).memory.activeRegion as *const u8)
                .add((pc & (*self.ptr).memory.activeMask) as usize)
                as *const u16) as u32;

            // cpu->gprs[ARM_PC] = pc;
            self.set_gpr(c::ARM_PC as usize, pc as i32);
        }
    }
}
