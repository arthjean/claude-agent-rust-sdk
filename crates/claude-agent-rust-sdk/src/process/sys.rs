//! Operating-system calls that keep a child process tree under control,
//! and the user database lookups they and the resume need.
//!
//! This is the only module of the crate allowed to contain `unsafe` code.
//! Every `unsafe` block is a direct call to an operating-system function
//! whose arguments are plain integers, handles, or buffers owned by this
//! module, or the read of a user database entry that such a call filled
//! in, while its buffer is alive.

#![allow(unsafe_code)]

#[cfg(all(unix, any(target_os = "macos", test)))]
pub(crate) use imp::current_user_name;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use imp::system_search_path;
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

    /// Makes the child run as the named user: its user id, its primary
    /// group, and its supplementary groups, as `sudo -u` or a login gives
    /// them. The user and its groups are looked up in the databases here,
    /// before the fork; nothing is spawned.
    ///
    /// The standard library switches the user before the closures of
    /// `pre_exec` run and clears the supplementary groups of a privileged
    /// parent, so the child would lose groups such as `docker`. The switch
    /// is made in one closure instead, registered before the others of the
    /// crate so the parent-death signal is armed after it (Linux clears
    /// that signal when the credentials change): `setgroups`, then
    /// `setgid`, then `setuid` (see [`switch_user`]).
    pub(crate) fn run_as_user(command: &mut Command, user: &str) -> io::Result<()> {
        let (user_id, group_id) = user_ids(user)?;
        let name = CString::new(user).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("unknown user: {user}"))
        })?;
        let groups = supplementary_groups(&name, group_id)?;
        unsafe {
            command.pre_exec(move || switch_user(user_id, group_id, &groups));
        }
        Ok(())
    }

    /// Runs in the forked child before `exec`. It only calls `setgroups`,
    /// `getuid`, `getgid`, `setgid`, and `setuid`, which are
    /// async-signal-safe, reads a list built before the fork, and
    /// allocates nothing.
    ///
    /// `setgroups` is always tried, so a parent with the capability to set
    /// groups but not root uses it too. It may only be refused (`EPERM`)
    /// when the child keeps the user and group it already has: a parent
    /// able to change the identity but not the groups would otherwise hand
    /// its own groups to another user, so the spawn fails instead.
    fn switch_user(
        user_id: libc::uid_t,
        group_id: libc::gid_t,
        groups: &[libc::gid_t],
    ) -> io::Result<()> {
        if unsafe { libc::setgroups(groups.len() as _, groups.as_ptr()) } != 0 {
            let error = io::Error::last_os_error();
            let same_identity =
                unsafe { libc::getuid() } == user_id && unsafe { libc::getgid() } == group_id;
            if error.raw_os_error() != Some(libc::EPERM) || !same_identity {
                return Err(error);
            }
        }
        if unsafe { libc::setgid(group_id) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::setuid(user_id) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The groups of a user, its primary group included, from the group
    /// database with `getgrouplist`, growing the list until it fits. The
    /// name and the list outlive each call.
    pub(crate) fn supplementary_groups(
        name: &std::ffi::CStr,
        group_id: libc::gid_t,
    ) -> io::Result<Vec<libc::gid_t>> {
        let mut capacity: usize = 64;
        loop {
            let mut groups: Vec<libc::gid_t> = vec![0; capacity];
            let mut count = libc::c_int::try_from(capacity).unwrap_or(libc::c_int::MAX);
            let status = unsafe {
                libc::getgrouplist(
                    name.as_ptr(),
                    group_id as _,
                    groups.as_mut_ptr().cast(),
                    &raw mut count,
                )
            };
            let filled = usize::try_from(count).unwrap_or(0);
            if status >= 0 && filled <= capacity {
                groups.truncate(filled);
                return Ok(groups);
            }
            if capacity >= USER_LOOKUP_BUFFER_LIMIT {
                return Err(io::Error::other("the user belongs to too many groups"));
            }
            capacity = (capacity * 2).max(filled);
        }
    }

    /// Looks a name up in the user database with `getpwnam_r`; the name
    /// outlives the call.
    fn user_ids(user: &str) -> io::Result<(libc::uid_t, libc::gid_t)> {
        let unknown =
            || io::Error::new(io::ErrorKind::InvalidInput, format!("unknown user: {user}"));
        let name = CString::new(user).map_err(|_| unknown())?;
        let ids = lookup_user(
            |entry, buffer, length, found| unsafe {
                libc::getpwnam_r(name.as_ptr(), entry, buffer, length, found)
            },
            |entry| (entry.pw_uid, entry.pw_gid),
        )?;
        ids.ok_or_else(unknown)
    }

    /// The login name of the user this process runs as, from the user
    /// database (`getpwuid_r` on `getuid`), as Python's `getpass.getuser`
    /// finds it when no variable names the user. `None` when the database
    /// has no entry for the user id or the name is not UTF-8.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn current_user_name() -> Option<String> {
        let user_id = unsafe { libc::getuid() };
        let name = lookup_user(
            |entry, buffer, length, found| unsafe {
                libc::getpwuid_r(user_id, entry, buffer, length, found)
            },
            |entry| {
                if entry.pw_name.is_null() {
                    return None;
                }
                let name = unsafe { std::ffi::CStr::from_ptr(entry.pw_name) };
                name.to_str().ok().map(str::to_owned)
            },
        );
        name.ok().flatten().flatten()
    }

    /// Runs one `getpw*_r` call with an entry and a buffer that both live
    /// until `read` returns, growing the buffer on `ERANGE`. The entry is
    /// read only when the system reports a match, which is when it has been
    /// filled in, and its strings point into the buffer, so `read` copies
    /// what it keeps. `Ok(None)` when the database has no match.
    fn lookup_user<T>(
        call: impl Fn(
            *mut libc::passwd,
            *mut libc::c_char,
            libc::size_t,
            *mut *mut libc::passwd,
        ) -> libc::c_int,
        read: impl FnOnce(&libc::passwd) -> T,
    ) -> io::Result<Option<T>> {
        let mut buffer: Vec<libc::c_char> = vec![0; USER_LOOKUP_BUFFER_START];
        loop {
            let mut entry = MaybeUninit::<libc::passwd>::uninit();
            let mut found: *mut libc::passwd = std::ptr::null_mut();
            let status = call(
                entry.as_mut_ptr(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &raw mut found,
            );
            if status == libc::ERANGE && buffer.len() < USER_LOOKUP_BUFFER_LIMIT {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status));
            }
            if found.is_null() {
                return Ok(None);
            }
            let entry = unsafe { entry.assume_init() };
            return Ok(Some(read(&entry)));
        }
    }

    /// The search path of the system, `confstr(_CS_PATH)`, which Python's
    /// `shutil.which` searches when `PATH` is not set. The buffer is sized
    /// by a first call and outlives the second. `None` when the system
    /// gives none or it is not UTF-8.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn system_search_path() -> Option<String> {
        let length = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
        if length == 0 {
            return None;
        }
        let mut buffer: Vec<u8> = vec![0; length];
        let written = unsafe { libc::confstr(libc::_CS_PATH, buffer.as_mut_ptr().cast(), length) };
        if written == 0 || written > length {
            return None;
        }
        buffer.truncate(written - 1);
        String::from_utf8(buffer).ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        #[test]
        fn the_system_search_path_names_usr_bin() {
            let path = system_search_path().expect("the system has a search path");
            assert!(path.split(':').any(|entry| entry == "/usr/bin"), "{path}");
        }

        #[tokio::test]
        async fn a_group_is_tracked_for_the_exit_hook_until_it_is_swept() {
            let mut command = Command::new("sleep");
            command.arg("30");
            prepare(&mut command);
            let mut child = command.spawn().unwrap();
            let mut tree = ProcessTree::attach(&child);
            let group = tree.group.unwrap();
            assert!(exit_registry::tracked(group));
            tree.kill(&mut child);
            child.wait().await.unwrap();
            tree.sweep(std::time::Duration::from_millis(100));
            assert!(!exit_registry::tracked(group));
        }

        #[test]
        fn the_groups_of_a_user_are_the_ones_the_group_database_lists_for_it() {
            let name = current_user_name().expect("the user database has the test user");
            let (_, group_id) = user_ids(&name).unwrap();
            let c_name = CString::new(name.clone()).unwrap();
            let mut groups = supplementary_groups(&c_name, group_id).unwrap();
            groups.sort_unstable();
            groups.dedup();
            let listed = std::process::Command::new("id")
                .args(["-G", &name])
                .output()
                .unwrap();
            let mut expected: Vec<libc::gid_t> = String::from_utf8(listed.stdout)
                .unwrap()
                .split_whitespace()
                .map(|id| id.parse().unwrap())
                .collect();
            expected.sort_unstable();
            expected.dedup();
            assert_eq!(groups, expected);
            assert!(groups.contains(&group_id));
        }

        #[test]
        fn the_current_user_name_maps_back_to_the_current_user_id() {
            let name = current_user_name().expect("the user database has the test user");
            assert!(!name.is_empty());
            let (user_id, _) = user_ids(&name).unwrap();
            assert_eq!(user_id, unsafe { libc::getuid() });
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
            if let Some(group) = group {
                exit_registry::track(group);
            }
            Self { group }
        }

        pub(crate) fn terminate(&self, _child: &mut Child) {
            self.signal(libc::SIGTERM);
        }

        pub(crate) fn kill(&self, child: &mut Child) {
            self.signal(libc::SIGKILL);
            let _ = child.start_kill();
        }

        /// Ends the processes left in the group once the child has exited:
        /// `SIGTERM` at once, then `SIGKILL` after `grace` to those still
        /// there, in a task of its own so that the exit of the child is
        /// reported without waiting. The group is polled every 50 ms and
        /// never signaled again once it is empty, so a group id the system
        /// reuses later is not touched.
        pub(crate) fn sweep(&mut self, grace: std::time::Duration) {
            let Some(group) = self.group.take() else {
                return;
            };
            exit_registry::untrack(group);
            if !signal_group(group, libc::SIGTERM) {
                return;
            }
            tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + grace;
                while tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    if !signal_group(group, 0) {
                        return;
                    }
                }
                signal_group(group, libc::SIGKILL);
            });
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
            if let Some(group) = self.group {
                exit_registry::untrack(group);
            }
        }
    }

    /// The process groups still alive, which get `SIGTERM` when the
    /// program exits through `exit` (returning from `main` or
    /// `std::process::exit`), as the `atexit` hook of the Python SDK
    /// signals its live children. Destructors do not run on that path, so
    /// without it a CLI the program did not close would be orphaned, on
    /// macOS above all, which has no parent-death signal. An abort or a
    /// kill of the program runs no hook, as in Python.
    pub(crate) mod exit_registry {
        use std::collections::BTreeSet;
        use std::sync::{Mutex, Once, PoisonError, TryLockError};

        static LIVE: Mutex<BTreeSet<libc::pid_t>> = Mutex::new(BTreeSet::new());
        static HOOK: Once = Once::new();

        /// Records a live group, installing the exit hook on first use.
        pub(crate) fn track(group: libc::pid_t) {
            HOOK.call_once(|| unsafe {
                libc::atexit(end_live_groups);
            });
            LIVE.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(group);
        }

        /// Forgets a group that has ended or been signaled.
        pub(crate) fn untrack(group: libc::pid_t) {
            LIVE.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&group);
        }

        /// Whether a group is recorded.
        #[cfg(test)]
        pub(crate) fn tracked(group: libc::pid_t) -> bool {
            LIVE.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&group)
        }

        /// Runs at exit. It never blocks: when another thread holds the
        /// registry while the program exits, the groups are left to the
        /// parent-death signal where the system has one.
        extern "C" fn end_live_groups() {
            let live = match LIVE.try_lock() {
                Ok(live) => live,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => return,
            };
            for group in live.iter() {
                unsafe {
                    libc::killpg(*group, libc::SIGTERM);
                }
            }
        }
    }

    /// Sends `signal` to the group, `0` only checking that it still has a
    /// member. False when no process of the group is left.
    fn signal_group(group: libc::pid_t, signal: libc::c_int) -> bool {
        unsafe { libc::killpg(group, signal) == 0 }
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
                        tracing::warn!(%error, "could not place Claude Code in a Job Object");
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

        /// Ends the processes left in the job once the child has exited.
        /// Windows has no signal a console-less process can handle, so
        /// they are terminated at once, whatever `grace` is.
        pub(crate) fn sweep(&mut self, _grace: std::time::Duration) {
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
