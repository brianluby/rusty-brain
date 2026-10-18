Harden the daemon's socket directory setup, `prepare_socket_dir` in `crates/rb-daemon/src/server.rs`. Make it public and re-export it at the rb_daemon crate root as `pub fn prepare_socket_dir(socket_path: &Path) -> rb_types::Result<()>`.

Required behavior for the socket's parent directory:
1. If it does not exist, create it (and any missing parents) as 0700 such that no concurrent caller can ever observe it with other permissions; concurrent first starts must all succeed.
2. If it exists and is a real directory owned by the current user with mode exactly 0700, accept it.
3. Otherwise refuse with an error and never chmod or repair: a directory with any other mode, a directory owned by another user, a non-directory, or a symlink (including a dangling one; never create through it). The error for a symlink must contain the word "symlink".
4. Keep existing callers working.
