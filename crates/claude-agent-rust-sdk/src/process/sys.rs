//! Operating-system calls that keep a child process tree under control.
//!
//! This is the only module of the crate allowed to contain `unsafe` code.
//! Every `unsafe` block is a direct call to an operating-system function
//! whose arguments are plain integers, handles, or buffers owned by this
//! module.

#![allow(unsafe_code)]

pub(crate) use imp::{ProcessTree, prepare, run_as_user};

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::mem::MaybeUninit;

    use tokio::process::{Child, Command};

    const FIRST_SIGNALABLE_GROUP: libc::pid_t = 2;
    const USER_LOOKUP_BUFFER_START: usize = 1024;
    const USER_LOOKUP_BUFFER_LIMIT: usize = 1024 * 1024;

    /// Makes the child run with the user id and the primary group id of the
    /// named user. The name is looked up in the user database; nothing is
    /// spawned here.
    ///
    /// The group is set as well as the user so that a privileged parent
    /// does not leave its own group on the child. When the parent may
    /// change groups, the standard library also clears the supplementary
    /// groups before it switches the user.
    pub(crate) fn run_as_user(command: &mut Command, user: &str) -> io::Result<()> {
        let (user_id, group_id) = user_ids(user)?;
        command.gid(group_id).uid(user_id);
        Ok(())
    }

    /// Calls `getpwnam_r` with a name, an entry, and a buffer that all live
    /// until it returns. The entry is read only when the system reports a
    /// match, which is when it has been filled in.
    fn user_ids(user: &str) -> io::Result<(libc::uid_t, libc::gid_t)> {
        let unknown =
            || io::Error::new(io::ErrorKind::InvalidInput, format!("unknown user: {user}"));
        let name = CString::new(user).map_err(|_| unknown())?;
        let mut buffer: Vec<libc::c_char> = vec![0; USER_LOOKUP_BUFFER_START];
        loop {
            let mut entry = MaybeUninit::<libc::passwd>::uninit();
            let mut found: *mut libc::passwd = std::ptr::null_mut();
            let status = unsafe {
                libc::getpwnam_r(
                    name.as_ptr(),
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &raw mut found,
                )
            };
            if status == libc::ERANGE && buffer.len() < USER_LOOKUP_BUFFER_LIMIT {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status));
            }
            if found.is_null() {
                return Err(unknown());
            }
            let entry = unsafe { entry.assume_init() };
            return Ok((entry.pw_uid, entry.pw_gid));
        }
    }

    /// Makes the child the leader of a new process group and, on Linux, asks
    /// the kernel to send it `SIGTERM` when the spawning thread dies.
    pub(crate) fn prepare(command: &mut Command) {
        command.process_group(0);
        #[cfg(target_os = "linux")]
        {
            let parent = std::process::id();
            unsafe {
                command.pre_exec(move || arm_parent_death_signal(parent));
            }
        }
    }

    /// Runs in the forked child before `exec`. It only calls `prctl` and
    /// `getppid`, which are async-signal-safe, and allocates nothing. The
    /// `getppid` comparison closes the window where the parent died between
    /// `fork` and `prctl`.
    #[cfg(target_os = "linux")]
    fn arm_parent_death_signal(parent: u32) -> std::io::Result<()> {
        let armed = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) };
        if armed != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let current_parent = unsafe { libc::getppid() };
        if u32::try_from(current_parent).ok() != Some(parent) {
            return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
        }
        Ok(())
    }

    /// Process group led by the child.
    ///
    /// Dropping it before [`ProcessTree::sweep`] sends `SIGTERM` to the whole
    /// group. That happens when the supervisor is torn down early, such as
    /// when the program returns from `main` with a session still open: the
    /// CLI then writes the end of its transcript and exits, as on the
    /// `SIGTERM` the Python SDK sends at interpreter exit, instead of being
    /// killed in the middle of the write.
    #[derive(Debug)]
    pub(crate) struct ProcessTree {
        group: Option<libc::pid_t>,
    }

    impl ProcessTree {
        pub(crate) fn attach(child: &Child) -> Self {
            let group = child
                .id()
                .and_then(|id| libc::pid_t::try_from(id).ok())
                .filter(|group| *group >= FIRST_SIGNALABLE_GROUP);
            Self { group }
        }

        pub(crate) fn terminate(&self, _child: &mut Child) {
            self.signal(libc::SIGTERM);
        }

        pub(crate) fn kill(&self, child: &mut Child) {
            self.signal(libc::SIGKILL);
            let _ = child.start_kill();
        }

        pub(crate) fn sweep(&mut self) {
            self.signal(libc::SIGKILL);
            self.group = None;
        }

        fn signal(&self, signal: libc::c_int) {
            if let Some(group) = self.group {
                unsafe {
                    libc::killpg(group, signal);
                }
            }
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            self.signal(libc::SIGTERM);
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

    use tokio::process::{Child, Command};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};

    const FORCED_EXIT_CODE: u32 = 1;

    /// Starts the child without a console window and in its own console
    /// process group.
    pub(crate) fn prepare(command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }

    /// Running the child as another user is a Unix facility; Windows
    /// refuses it instead of ignoring the request.
    pub(crate) fn run_as_user(_command: &mut Command, _user: &str) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "the user option is not supported on this platform",
        ))
    }

    /// Job Object holding the child and its descendants.
    ///
    /// The job is created with kill-on-close, so closing its handle, which
    /// happens when this value is dropped or when this process dies, ends
    /// every process still in it.
    #[derive(Debug)]
    pub(crate) struct ProcessTree {
        job: Option<OwnedHandle>,
    }

    impl ProcessTree {
        pub(crate) fn attach(child: &Child) -> Self {
            let job = child.raw_handle().and_then(|process| {
                create_job(process)
                    .inspect_err(|error| {
                        tracing::warn!(%error, "could not place the Claude CLI in a Job Object");
                    })
                    .ok()
            });
            Self { job }
        }

        pub(crate) fn terminate(&self, child: &mut Child) {
            self.terminate_job();
            let _ = child.start_kill();
        }

        pub(crate) fn kill(&self, child: &mut Child) {
            self.terminate(child);
        }

        pub(crate) fn sweep(&mut self) {
            self.terminate_job();
            self.job = None;
        }

        fn terminate_job(&self) {
            if let Some(job) = &self.job {
                unsafe {
                    TerminateJobObject(job.as_raw_handle(), FORCED_EXIT_CODE);
                }
            }
        }
    }

    /// Creates a kill-on-close job and assigns `process` to it. The job
    /// handle returned by the system is wrapped at once in an `OwnedHandle`,
    /// which closes it on every path; `process` is a live handle borrowed
    /// from the `Child` for the duration of the call.
    fn create_job(process: RawHandle) -> std::io::Result<OwnedHandle> {
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .map_err(std::io::Error::other)?;
        let configured = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size,
            )
        };
        if configured == 0 {
            return Err(std::io::Error::last_os_error());
        }

        let assigned = unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) };
        if assigned == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }
}
