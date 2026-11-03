//! Single-instance hand-off over a per-user Unix socket.
//!
//! A second launch connects to the running instance, sends its launch
//! arguments as one JSON array line and exits. If the socket cannot be created
//! or reached, the application continues normally: the mechanism is an
//! optimization, never a launch blocker.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixListener},
};

/// The per-user control socket.
pub(crate) fn socket_path() -> PathBuf {
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    directory.join(format!("astrolith-{}.sock", user_tag()))
}

fn user_tag() -> String {
    let identity = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USER"))
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    // FNV-1a keeps the socket name stable without a hashing dependency.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in identity.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The bound listener type; [`Infallible`](std::convert::Infallible) where the
/// platform has no single-instance mechanism.
#[cfg(unix)]
pub(crate) type Listener = std::os::unix::net::UnixListener;

/// The bound listener type; [`Infallible`](std::convert::Infallible) where the
/// platform has no single-instance mechanism.
#[cfg(not(unix))]
pub(crate) type Listener = std::convert::Infallible;

/// Encode launch arguments for the running instance.
pub(crate) fn encode(arguments: &[String]) -> String {
    let mut line = serde_json::to_string(arguments).unwrap_or_else(|_| "[]".to_owned());
    line.push('\n');
    line
}

/// Decode one hand-off line; malformed messages are ignored.
pub(crate) fn decode(line: &str) -> Vec<String> {
    serde_json::from_str(line.trim()).unwrap_or_default()
}

/// Send launch arguments to a running instance.
///
/// Returns `true` when a running instance accepted the message; `false` when no
/// instance is listening and this launch owns the window.
#[cfg(unix)]
pub(crate) fn forward(socket: &Path, arguments: &[String]) -> bool {
    use std::os::unix::net::UnixStream;
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.write_all(encode(arguments).as_bytes());
    true
}

/// Try to become the listening instance.
///
/// A stale socket left by a crashed process is replaced. The socket is user
/// only (0600) because it carries local paths.
#[cfg(unix)]
pub(crate) fn bind(socket: &Path) -> io::Result<UnixListener> {
    let _ = fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

#[cfg(not(unix))]
pub(crate) fn forward(_socket: &Path, _arguments: &[String]) -> bool {
    false
}

#[cfg(not(unix))]
pub(crate) fn bind(_socket: &Path) -> io::Result<Listener> {
    Err(io::Error::other("single instance is unsupported"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_off_messages_round_trip_and_ignore_garbage() {
        let arguments = vec!["astrolith://open?path=%2Ftmp%2Fsite".to_owned()];
        assert_eq!(decode(&encode(&arguments)), arguments);
        assert!(decode("not json").is_empty());
        assert!(decode("").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_second_launch_forwards_to_the_bound_socket() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("astrolith.sock");
        let listener = bind(&socket).unwrap();
        assert!(forward(&socket, &["one".to_owned()]));
        let (mut stream, _) = listener.accept().unwrap();
        use std::io::Read;
        let mut line = String::new();
        stream.read_to_string(&mut line).unwrap();
        assert_eq!(decode(&line), ["one".to_owned()]);

        std::fs::remove_file(&socket).unwrap();
        assert!(!forward(&socket, &[]));
    }
}
