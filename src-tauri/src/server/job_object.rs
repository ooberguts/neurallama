//! Windows Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. Every llama-server we spawn
//! is assigned to it; when the last handle closes (app exit or crash) Windows kills the
//! child, so it can never outlive Neurallama.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

pub struct JobObject(HANDLE);

// The handle is an opaque kernel object handle; it is safe to use from any thread.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return Err(format!("CreateJobObject failed: {}", std::io::Error::last_os_error()));
            }
            let job = JobObject(h);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return Err(format!("SetInformationJobObject failed: {}", std::io::Error::last_os_error()));
            }
            Ok(job)
        }
    }

    /// Assign a process (raw process handle, e.g. from `tokio::process::Child::raw_handle`).
    pub fn assign(&self, process: *mut c_void) -> Result<(), String> {
        let ok = unsafe { AssignProcessToJobObject(self.0, process as HANDLE) };
        if ok == 0 {
            Err(format!("AssignProcessToJobObject failed: {}", std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
