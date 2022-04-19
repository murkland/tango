use super::c;
use super::core;

#[repr(transparent)]
pub struct Thread(std::sync::Arc<parking_lot::Mutex<Box<InnerThread>>>);

pub struct InnerThread {
    core: core::Core,
    raw: c::mCoreThread,
    frame_callback: Option<Box<dyn Fn(core::CoreMutRef, &[u8]) + Send + 'static>>,
    current_callback: std::cell::RefCell<Option<Box<dyn Fn(crate::core::CoreMutRef<'_>)>>>,
}

unsafe extern "C" fn c_frame_callback(ptr: *mut c::mCoreThread) {
    let t = &*((*ptr).userData as *mut InnerThread);
    if let Some(cb) = t.frame_callback.as_ref() {
        cb(
            core::CoreMutRef {
                ptr: t.raw.core,
                _lifetime: std::marker::PhantomData,
            },
            t.core.video_buffer().unwrap(),
        );
    }
}

pub struct AudioGuard<'a> {
    thread: parking_lot::MutexGuard<'a, Box<InnerThread>>,
}

impl<'a> AudioGuard<'a> {
    pub fn core(&self) -> core::CoreRef<'a> {
        core::CoreRef {
            ptr: self.thread.raw.core,
            _lifetime: std::marker::PhantomData::<&'a ()>,
        }
    }

    pub fn core_mut(&mut self) -> core::CoreMutRef<'a> {
        core::CoreMutRef {
            ptr: self.thread.raw.core,
            _lifetime: std::marker::PhantomData::<&'a ()>,
        }
    }
}

impl<'a> Drop for AudioGuard<'a> {
    fn drop(&mut self) {
        self.core_mut()
            .gba_mut()
            .sync_mut()
            .unwrap()
            .consume_audio()
    }
}

impl Thread {
    pub fn new(core: core::Core) -> Self {
        let core_ptr = core.ptr;
        let mut t = Box::new(InnerThread {
            core,
            raw: unsafe { std::mem::zeroed::<c::mCoreThread>() },
            frame_callback: None,
            current_callback: std::cell::RefCell::new(None),
        });
        t.raw.core = core_ptr;
        t.raw.logger.d = unsafe { *c::mLogGetContext() };
        t.raw.userData = &mut *t as *mut _ as *mut std::os::raw::c_void;
        t.raw.frameCallback = Some(c_frame_callback);
        Thread(std::sync::Arc::new(parking_lot::Mutex::new(t)))
    }

    pub fn set_frame_callback(&self, f: impl Fn(core::CoreMutRef, &[u8]) + Send + 'static) {
        self.0.lock().frame_callback = Some(Box::new(f));
    }

    pub fn handle(&self) -> Handle {
        Handle {
            thread: self.0.clone(),
            ptr: &mut self.0.lock().raw,
        }
    }

    pub fn start(&self) -> bool {
        unsafe { c::mCoreThreadStart(&mut self.0.lock().raw) }
    }

    pub fn join(&self) {
        unsafe { c::mCoreThreadJoin(&mut self.0.lock().raw) }
    }

    pub fn end(&self) {
        unsafe { c::mCoreThreadEnd(&mut self.0.lock().raw) }
    }
}

#[derive(Clone)]
pub struct Handle {
    thread: std::sync::Arc<parking_lot::Mutex<Box<InnerThread>>>,
    ptr: *mut c::mCoreThread,
}

unsafe extern "C" fn c_run_function(ptr: *mut c::mCoreThread) {
    let t = &mut *((*ptr).userData as *mut InnerThread);
    let mut cc = t.current_callback.borrow_mut();
    let cc = cc.as_mut().unwrap();
    cc(crate::core::CoreMutRef {
        ptr: t.raw.core,
        _lifetime: std::marker::PhantomData,
    });
}

impl Handle {
    pub fn pause(&self) {
        unsafe { c::mCoreThreadPause(self.ptr) }
    }

    pub fn unpause(&self) {
        unsafe { c::mCoreThreadUnpause(self.ptr) }
    }

    pub fn run_on_core(&self, f: impl Fn(crate::core::CoreMutRef<'_>) + Send + Sync + 'static) {
        let thread = self.thread.lock();
        *thread.current_callback.borrow_mut() = Some(Box::new(f));
        unsafe { c::mCoreThreadRunFunction(self.ptr, Some(c_run_function)) }
    }

    pub fn lock_audio(&self) -> AudioGuard {
        let thread = self.thread.lock();
        let mut core = core::CoreMutRef {
            ptr: thread.raw.core,
            _lifetime: std::marker::PhantomData,
        };
        core.gba_mut().sync_mut().unwrap().lock_audio();
        AudioGuard { thread }
    }
}
