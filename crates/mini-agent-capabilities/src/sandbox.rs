use std::io;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SandboxKind {
    #[default]
    Native,
    None,
    Docker,
}

impl SandboxKind {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.to_ascii_lowercase().as_str() {
            "native" | "os" | "local" => Ok(Self::Native),
            "none" | "unsandboxed" | "disabled" => Ok(Self::None),
            "docker" | "container" => Ok(Self::Docker),
            other => Err(format!("unknown sandbox kind: {other}")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::None => "none",
            Self::Docker => "docker",
        }
    }

    pub fn as_str(self) -> &'static str {
        self.name()
    }
}

impl std::fmt::Display for SandboxKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(windows)]
pub mod windows_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    type RawHandle = *mut c_void;
    type RawBool = i32;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x00002000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JobobjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct JobobjectExtendedLimitInformation {
        basic_limit_information: JobobjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_limit: usize,
        peak_job_memory_limit: usize,
    }

    unsafe extern "system" {
        fn CreateJobObjectW(lpJobAttributes: *const c_void, lpName: *const u16) -> RawHandle;
        fn SetInformationJobObject(
            hJob: RawHandle,
            JobObjectInformationClass: i32,
            lpJobObjectInformation: *const c_void,
            cbJobObjectInformationLength: u32,
        ) -> RawBool;
        fn AssignProcessToJobObject(hJob: RawHandle, hProcess: RawHandle) -> RawBool;
        fn TerminateJobObject(hJob: RawHandle, uExitCode: u32) -> RawBool;
        fn CloseHandle(hObject: RawHandle) -> RawBool;
    }

    pub struct JobObjectGuard {
        handle: RawHandle,
    }

    unsafe impl Send for JobObjectGuard {}
    unsafe impl Sync for JobObjectGuard {}

    impl JobObjectGuard {
        pub fn create() -> Option<Self> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let mut info: JobobjectExtendedLimitInformation = std::mem::zeroed();
                info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let success = SetInformationJobObject(
                    handle,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<JobobjectExtendedLimitInformation>() as u32,
                );
                if success == 0 {
                    CloseHandle(handle);
                    return None;
                }
                Some(Self { handle })
            }
        }

        pub fn assign_child(&self, child: &Child) -> bool {
            unsafe {
                let raw_handle = child.as_raw_handle() as RawHandle;
                AssignProcessToJobObject(self.handle, raw_handle) != 0
            }
        }

        pub fn terminate(&self, exit_code: u32) -> bool {
            unsafe { TerminateJobObject(self.handle, exit_code) != 0 }
        }
    }

    impl Drop for JobObjectGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    CloseHandle(self.handle);
                }
            }
        }
    }
}

pub struct ProcessSandbox {
    #[cfg(windows)]
    job_object: Option<windows_job::JobObjectGuard>,
}

impl ProcessSandbox {
    pub fn new(kind: SandboxKind) -> Self {
        #[cfg(windows)]
        let job_object = if kind == SandboxKind::Native {
            windows_job::JobObjectGuard::create()
        } else {
            None
        };

        Self {
            #[cfg(windows)]
            job_object,
        }
    }

    pub fn attach_child(&self, child: &Child) {
        #[cfg(windows)]
        if let Some(ref job) = self.job_object {
            job.assign_child(child);
        }
        #[cfg(not(windows))]
        let _ = child;
    }

    pub fn terminate(&self, child: &mut Child) -> io::Result<ExitStatus> {
        #[cfg(windows)]
        {
            if let Some(ref job) = self.job_object {
                job.terminate(1);
            } else {
                let process_id = child.id().to_string();
                let _ = Command::new("taskkill")
                    .args(["/PID", &process_id, "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        #[cfg(not(windows))]
        {
            let process_id = child.id().to_string();
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{process_id}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        if child.try_wait()?.is_none() {
            let _ = child.kill();
        }
        child.wait()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sandbox_kinds() {
        assert_eq!(SandboxKind::parse("native").unwrap(), SandboxKind::Native);
        assert_eq!(SandboxKind::parse("none").unwrap(), SandboxKind::None);
        assert_eq!(SandboxKind::parse("docker").unwrap(), SandboxKind::Docker);
        assert_eq!(SandboxKind::Native.name(), "native");
        assert!(SandboxKind::parse("invalid").is_err());
    }

    #[test]
    fn creates_and_attaches_sandbox_guard() {
        let sandbox = ProcessSandbox::new(SandboxKind::Native);
        #[cfg(windows)]
        assert!(sandbox.job_object.is_some());
    }
}
