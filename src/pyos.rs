//! The handful of `os.path` and `pathlib` answers the Python release relied
//! on, reproduced rather than approximated: `basename("a/")` is "" and
//! `Path("a/").name` is "a", and both of those show up on screen.

#[cfg(windows)]
fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

#[cfg(not(windows))]
fn is_sep(c: char) -> bool {
    c == '/'
}

/// `os.path.basename(path)`.
pub fn basename(path: &str) -> &str {
    #[cfg(windows)]
    let path = splitdrive(path).1;
    match path.rfind(is_sep) {
        Some(index) => &path[index + 1..],
        None => path,
    }
}

#[cfg(windows)]
fn splitdrive(path: &str) -> (&str, &str) {
    let chars: Vec<char> = path.chars().take(3).collect();
    if chars.len() >= 2 && chars[1] == ':' {
        let split = chars[0].len_utf8() + 1;
        return (&path[..split], &path[split..]);
    }
    if path.starts_with("\\\\") || path.starts_with("//") {
        // \\server\share: the drive is the first two components.
        let rest = &path[2..];
        if let Some(first) = rest.find(is_sep) {
            let after = &rest[first + 1..];
            let second = after.find(is_sep).map(|i| 2 + first + 1 + i).unwrap_or(path.len());
            return (&path[..second], &path[second..]);
        }
    }
    ("", path)
}

#[cfg(not(windows))]
fn passwd_home() -> Option<String> {
    // getpwuid_r: this runs on worker threads, where getpwuid is unsafe.
    let mut size = 1024usize;
    loop {
        let mut buffer = vec![0 as libc::c_char; size];
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let code = unsafe { libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr(), size, &mut found) };
        if code == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if code != 0 || found.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) };
        return Some(dir.to_string_lossy().into_owned());
    }
}

/// `os.path.expanduser(path)`: a leading `~` or `~user`, as posixpath does it.
pub fn expanduser(path: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    let end = path.find('/').unwrap_or(path.len());
    let user = &path[1..end];
    let home = if user.is_empty() {
        match std::env::var_os("HOME") {
            Some(value) => value.to_string_lossy().into_owned(),
            None => match passwd_home() {
                Some(home) => home,
                None => return path.to_string(),
            },
        }
    } else {
        match named_home(user) {
            Some(home) => home,
            None => return path.to_string(),
        }
    };
    let trimmed = home.trim_end_matches('/');
    let out = format!("{}{}", trimmed, &path[end..]);
    if out.is_empty() {
        "/".to_string()
    } else {
        out
    }
}

/// The home directory of a named user, from the password database.
fn named_home(user: &str) -> Option<String> {
    let name = std::ffi::CString::new(user).ok()?;
    let mut size = 1024usize;
    loop {
        let mut buffer = vec![0 as libc::c_char; size];
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let code = unsafe { libc::getpwnam_r(name.as_ptr(), &mut entry, buffer.as_mut_ptr(), size, &mut found) };
        if code == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if code != 0 || found.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) };
        return Some(dir.to_string_lossy().into_owned());
    }
}

/// `os.path.expandvars(path)`: `$name` and `${name}`, unknown ones left alone.
pub fn expandvars(path: &str) -> String {
    if !path.contains('$') {
        return path.to_string();
    }
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut index = 0;
    let mut copied = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        // `\$(\w+|\{[^}]*\})` under re.ASCII.
        let start = index + 1;
        let (name, end) = if start < bytes.len() && bytes[start] == b'{' {
            match path[start + 1..].find('}') {
                Some(close) => (&path[start + 1..start + 1 + close], start + 1 + close + 1),
                None => {
                    index += 1;
                    continue;
                }
            }
        } else {
            let mut stop = start;
            while stop < bytes.len() && (bytes[stop].is_ascii_alphanumeric() || bytes[stop] == b'_') {
                stop += 1;
            }
            if stop == start {
                index += 1;
                continue;
            }
            (&path[start..stop], stop)
        };
        let value =
            if name.is_empty() || name.contains('=') || name.contains('\0') { None } else { std::env::var_os(name) };
        match value {
            Some(value) => {
                out.push_str(&path[copied..index]);
                out.push_str(&value.to_string_lossy());
                copied = end;
                index = end;
            }
            None => index = end,
        }
    }
    out.push_str(&path[copied..]);
    out
}

/// `os.path.dirname(path)`.
pub fn dirname(path: &str) -> String {
    let cut = path.rfind('/').map(|index| index + 1).unwrap_or(0);
    let head = &path[..cut];
    if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/').to_string()
    } else {
        head.to_string()
    }
}

/// `os.path.join(a, b)`.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        return b.to_string();
    }
    if a.is_empty() || a.ends_with('/') {
        return format!("{}{}", a, b);
    }
    format!("{}/{}", a, b)
}

/// `os.path.isabs(path)`.
pub fn isabs(path: &str) -> bool {
    path.starts_with('/')
}

/// `os.getcwd()`, or "" when the cwd has gone.
pub fn getcwd() -> String {
    crate::lookups::ask(|| std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
}

fn split(path: &str) -> (String, String) {
    let cut = path.rfind('/').map(|index| index + 1).unwrap_or(0);
    let (head, tail) = path.split_at(cut);
    let head = if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/').to_string()
    } else {
        head.to_string()
    };
    (head, tail.to_string())
}

/// `os.path.realpath(path)`: every symlink followed, a missing tail kept as
/// written rather than an error, as posixpath does it with `strict=False`.
#[cfg(test)]
pub fn realpath(path: &str) -> String {
    realpath_with(path, &mut std::collections::HashSet::new())
}

/// `realpath`, trusting `real` to hold paths an earlier call found to exist
/// and not be symlinks, and adding the ones this call finds. Paths that share
/// a parent then cost one lstat each rather than one per component.
pub fn realpath_with(path: &str, real: &mut std::collections::HashSet<String>) -> String {
    crate::lookups::ask(|| realpath_uncounted(path, real))
}

fn realpath_uncounted(path: &str, real: &mut std::collections::HashSet<String>) -> String {
    let mut seen: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    let (resolved, _) = join_realpath(String::new(), path, &mut seen, real);
    abspath(&resolved)
}

fn join_realpath(
    mut path: String,
    rest: &str,
    seen: &mut std::collections::HashMap<String, Option<String>>,
    real: &mut std::collections::HashSet<String>,
) -> (String, bool) {
    let mut rest = rest.to_string();
    if isabs(&rest) {
        rest = rest[1..].to_string();
        path = "/".to_string();
    }
    while !rest.is_empty() {
        let (name, remainder) = match rest.find('/') {
            Some(index) => (rest[..index].to_string(), rest[index + 1..].to_string()),
            None => (rest.clone(), String::new()),
        };
        rest = remainder;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            if !path.is_empty() {
                let (head, tail) = split(&path);
                path = head;
                if tail == ".." {
                    path = join(&join(&path, ".."), "..");
                }
            } else {
                path = "..".to_string();
            }
            continue;
        }
        let newpath = join(&path, &name);
        if real.contains(&newpath) {
            path = newpath;
            continue;
        }
        match std::fs::symlink_metadata(&newpath) {
            Ok(meta) if meta.file_type().is_symlink() => {}
            Ok(_) => {
                real.insert(newpath.clone());
                path = newpath;
                continue;
            }
            Err(_) => {
                path = newpath;
                continue;
            }
        }
        if let Some(known) = seen.get(&newpath) {
            match known {
                Some(resolved) => {
                    path = resolved.clone();
                    continue;
                }
                // Not resolved yet, so this is a symlink loop.
                None => return (join(&newpath, &rest), false),
            }
        }
        seen.insert(newpath.clone(), None);
        let target = match std::fs::read_link(&newpath) {
            Ok(target) => target.to_string_lossy().into_owned(),
            Err(_) => {
                path = newpath;
                continue;
            }
        };
        let (resolved, ok) = join_realpath(path, &target, seen, real);
        path = resolved;
        if !ok {
            return (join(&path, &rest), false);
        }
        seen.insert(newpath, Some(path.clone()));
    }
    (path, true)
}

/// `os.path.abspath(path)`: absolute, normalised, without touching the disk.
pub fn abspath(path: &str) -> String {
    let joined = if is_absolute(path) {
        path.to_string()
    } else {
        let cwd = getcwd();
        if cwd.ends_with(is_sep) {
            cwd + path
        } else {
            format!("{}{}{}", cwd, std::path::MAIN_SEPARATOR, path)
        }
    };
    normpath(&joined)
}

fn is_absolute(path: &str) -> bool {
    #[cfg(windows)]
    {
        let (drive, rest) = splitdrive(path);
        !drive.is_empty() && rest.starts_with(is_sep)
    }
    #[cfg(not(windows))]
    {
        path.starts_with('/')
    }
}

/// `os.path.normpath(path)`.
pub fn normpath(path: &str) -> String {
    #[cfg(not(windows))]
    {
        if path.is_empty() {
            return ".".into();
        }
        let initial = if path.starts_with("//") && !path.starts_with("///") {
            2
        } else if path.starts_with('/') {
            1
        } else {
            0
        };
        let mut parts: Vec<&str> = Vec::new();
        for part in path.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part != ".." || (initial == 0 && parts.is_empty()) || parts.last() == Some(&"..") {
                parts.push(part);
            } else if !parts.is_empty() {
                parts.pop();
            }
        }
        let body = parts.join("/");
        let out = "/".repeat(initial) + &body;
        if out.is_empty() {
            ".".into()
        } else {
            out
        }
    }
    #[cfg(windows)]
    {
        let normalized = path.replace('/', "\\");
        let (drive, rest) = splitdrive(&normalized);
        let rooted = rest.starts_with('\\');
        let mut parts: Vec<&str> = Vec::new();
        for part in rest.split('\\') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                if parts.last().map(|p| *p != "..").unwrap_or(false) {
                    parts.pop();
                } else if !rooted {
                    parts.push(part);
                }
            } else {
                parts.push(part);
            }
        }
        let mut out = drive.to_string();
        if rooted {
            out.push('\\');
        }
        out.push_str(&parts.join("\\"));
        if out.is_empty() {
            ".".into()
        } else {
            out
        }
    }
}

/// `shutil.which(name)` for a bare command name: the first file on PATH that
/// exists, is executable and is not a directory.
pub fn which(name: &str) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let path = match std::env::var_os("PATH") {
        Some(value) => value.to_string_lossy().into_owned(),
        None => {
            if cfg!(target_os = "macos") {
                "/usr/bin:/bin:/usr/sbin:/sbin".to_string()
            } else {
                "/bin:/usr/bin".to_string()
            }
        }
    };
    if path.is_empty() {
        return None;
    }
    let mut seen: Vec<&str> = Vec::new();
    for directory in path.split(':') {
        if seen.contains(&directory) {
            continue;
        }
        seen.push(directory);
        let candidate = join(directory, name);
        let Ok(meta) = std::fs::metadata(&candidate) else { continue };
        if meta.is_dir() {
            continue;
        }
        let Ok(c_path) = std::ffi::CString::new(std::ffi::OsStr::new(&candidate).as_bytes()) else { continue };
        if unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } == 0 {
            return Some(candidate);
        }
    }
    None
}
