//! Windows: a `SunlitEarth` value under `HKCU\...\CurrentVersion\Run`.
//!
//! The data is the executable, always quoted and never canonicalized, so the
//! junction Scoop puts at `current` is what gets written, and the arguments.
//! Task Manager and Settings switch an entry off by writing a `REG_BINARY` of
//! the same name under `Explorer\StartupApproved\Run` whose first byte is odd;
//! an even first byte, or no such value, is on. Turning the entry on removes
//! that value, as Electron does, so an old switch-off cannot keep a new entry
//! off.

use std::path::{Path, PathBuf};

use super::{ARGS, Autostart};

/// The name of the value under both keys.
pub const VALUE_NAME: &str = "SunlitEarth";

/// The command line a `Run` value holds for `exe`.
///
/// :param exe: the executable, as `current_exe()` gave it
/// :returns: the command line, or why it cannot be written
pub fn command_line(exe: &Path) -> Result<String, String> {
    let exe = exe.to_string_lossy();
    if exe.contains('"') {
        return Err(format!(
            "The path `{exe}` holds a quotation mark, which a startup entry cannot carry."
        ));
    }
    Ok(format!("\"{exe}\" {}", ARGS.join(" ")))
}

/// Split a command line the way the C runtime does for `argv`.
///
/// The program name follows its own rule: a quoted one runs to the next quote
/// with no escapes, an unquoted one to the first space or tab. The arguments
/// after it follow the usual one: `2n` backslashes before a quote are `n`
/// backslashes and the quote toggles quoting, `2n + 1` are `n` and a literal
/// quote, and `""` inside quotes is a literal quote.
#[must_use]
pub fn parse_command_line(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = line.chars().peekable();

    let mut program = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
        for c in chars.by_ref() {
            if c == '"' {
                break;
            }
            program.push(c);
        }
    } else {
        while let Some(&c) = chars.peek() {
            if c == ' ' || c == '\t' {
                break;
            }
            program.push(c);
            chars.next();
        }
    }
    args.push(program);

    loop {
        while matches!(chars.peek(), Some(' ' | '\t')) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut arg = String::new();
        let mut quoted = false;
        while let Some(&c) = chars.peek() {
            match c {
                ' ' | '\t' if !quoted => break,
                '\\' => {
                    let mut backslashes = 0;
                    while chars.peek() == Some(&'\\') {
                        backslashes += 1;
                        chars.next();
                    }
                    if chars.peek() == Some(&'"') {
                        arg.extend(std::iter::repeat_n('\\', backslashes / 2));
                        if backslashes % 2 == 1 {
                            arg.push('"');
                            chars.next();
                        }
                    } else {
                        arg.extend(std::iter::repeat_n('\\', backslashes));
                    }
                }
                '"' => {
                    chars.next();
                    if quoted && chars.peek() == Some(&'"') {
                        arg.push('"');
                        chars.next();
                    } else {
                        quoted = !quoted;
                    }
                }
                _ => {
                    arg.push(c);
                    chars.next();
                }
            }
        }
        args.push(arg);
    }
    args
}

/// Whether a `StartupApproved` value leaves the entry on.
///
/// :param approved: the value's bytes, `None` where there is no value
/// :returns: `true` for no value or an even first byte
#[must_use]
pub fn approved(approved: Option<&[u8]>) -> bool {
    match approved {
        None => true,
        Some(bytes) => bytes.first().is_some_and(|first| first % 2 == 0),
    }
}

/// The state a `Run` value and its `StartupApproved` value come to.
fn state(run: Option<&str>, approval: Option<&[u8]>) -> Autostart {
    let Some(run) = run else {
        return Autostart::Off;
    };
    let exe = parse_command_line(run)
        .into_iter()
        .next()
        .filter(|program| !program.is_empty())
        .map_or_else(|| PathBuf::from(run), PathBuf::from);
    if approved(approval) {
        Autostart::On { exe }
    } else {
        Autostart::TurnedOffBySystem { exe }
    }
}

/// Where the entry lives: two keys under `HKEY_CURRENT_USER`.
#[derive(Clone, Debug)]
pub struct Location {
    /// The key that holds the `Run` value.
    pub run_key: String,
    /// The key that holds the `StartupApproved` value.
    pub approved_key: String,
}

impl Location {
    /// The keys Windows reads at logon.
    #[must_use]
    pub fn system() -> Self {
        Self {
            run_key: r"Software\Microsoft\Windows\CurrentVersion\Run".to_owned(),
            approved_key: r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
                .to_owned(),
        }
    }
}

#[cfg(windows)]
impl Location {
    /// Read the entry.
    #[must_use]
    pub fn read(&self) -> Autostart {
        let run = match registry::read_string(&self.run_key, VALUE_NAME) {
            Ok(run) => run,
            Err(e) => {
                tracing::warn!("the startup entry could not be read: {e}");
                return Autostart::Off;
            }
        };
        let approval = registry::read_binary(&self.approved_key, VALUE_NAME).unwrap_or_else(|e| {
            tracing::warn!("the startup entry's approval could not be read: {e}");
            None
        });
        state(run.as_deref(), approval.as_deref())
    }

    /// Write the entry for `exe` and clear any switch-off of it.
    ///
    /// :returns: why it could not be written, as a sentence for the user
    pub fn enable(&self, exe: &Path) -> Result<(), String> {
        let line = command_line(exe)?;
        registry::write_string(&self.run_key, VALUE_NAME, &line)
            .map_err(|e| format!("The startup entry could not be written: {e}"))?;
        registry::delete_value(&self.approved_key, VALUE_NAME)
            .map_err(|e| format!("The startup entry's old switch-off could not be cleared: {e}"))
    }

    /// Remove the entry and its approval.
    ///
    /// :returns: why it could not be removed, as a sentence for the user
    pub fn disable(&self) -> Result<(), String> {
        registry::delete_value(&self.run_key, VALUE_NAME)
            .map_err(|e| format!("The startup entry could not be removed: {e}"))?;
        registry::delete_value(&self.approved_key, VALUE_NAME)
            .map_err(|e| format!("The startup entry's switch-off could not be removed: {e}"))
    }
}

#[cfg(windows)]
pub(crate) mod registry {
    //! The few registry calls the entry needs, all under `HKEY_CURRENT_USER`.

    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, ERROR_UNSUPPORTED_TYPE, WIN32_ERROR,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
        RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
        RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn error(code: WIN32_ERROR) -> String {
        std::io::Error::from_raw_os_error(code.cast_signed()).to_string()
    }

    /// The bytes of a value of the given type, or `None` where the key or the
    /// value does not exist. A value of another type reads as no bytes.
    fn read_bytes(key: &str, value: &str, flags: u32) -> Result<Option<Vec<u8>>, String> {
        let (key, value) = (wide(key), wide(value));
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            let mut size = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
            let data = if buffer.is_empty() {
                std::ptr::null_mut()
            } else {
                buffer.as_mut_ptr().cast()
            };
            // SAFETY: both names are NUL-terminated UTF-16 that outlive the
            // call, `data` is null or `buffer`'s storage of `size` bytes, and
            // the call writes at most `size` bytes and the size it needs.
            #[allow(unsafe_code)]
            let status = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    flags,
                    std::ptr::null_mut(),
                    data,
                    &raw mut size,
                )
            };
            match status {
                ERROR_SUCCESS if !buffer.is_empty() || size == 0 => {
                    buffer.truncate(size as usize);
                    return Ok(Some(buffer));
                }
                ERROR_SUCCESS | ERROR_MORE_DATA => buffer.resize(size.max(1) as usize, 0),
                ERROR_FILE_NOT_FOUND => return Ok(None),
                ERROR_UNSUPPORTED_TYPE => return Ok(Some(Vec::new())),
                code => return Err(error(code)),
            }
        }
    }

    pub(crate) fn read_string(key: &str, value: &str) -> Result<Option<String>, String> {
        Ok(read_bytes(key, value, RRF_RT_REG_SZ)?.map(|bytes| {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .take_while(|&unit| unit != 0)
                .collect();
            String::from_utf16_lossy(&units)
        }))
    }

    pub(crate) fn read_binary(key: &str, value: &str) -> Result<Option<Vec<u8>>, String> {
        read_bytes(key, value, RRF_RT_REG_BINARY)
    }

    fn write(key: &str, value: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), String> {
        let (key, value) = (wide(key), wide(value));
        let mut handle: HKEY = std::ptr::null_mut();
        // SAFETY: the key name is NUL-terminated UTF-16 that outlives the
        // call, the class and the security attributes are optional and null,
        // and `handle` is a live local the call writes the opened key to.
        #[allow(unsafe_code)]
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &raw mut handle,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(error(status));
        }
        let size = u32::try_from(data.len()).map_err(|_| "the value is too long".to_owned())?;
        // SAFETY: `handle` was just opened for setting values, the name is
        // NUL-terminated UTF-16 and `data` is `size` readable bytes; the key
        // is closed once and not used after.
        #[allow(unsafe_code)]
        let status = unsafe {
            let status = RegSetValueExW(handle, value.as_ptr(), 0, kind, data.as_ptr(), size);
            RegCloseKey(handle);
            status
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(error(status))
        }
    }

    pub(crate) fn write_string(key: &str, value: &str, data: &str) -> Result<(), String> {
        let bytes: Vec<u8> = wide(data)
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        write(key, value, REG_SZ, &bytes)
    }

    #[cfg(test)]
    pub(crate) fn write_binary(key: &str, value: &str, data: &[u8]) -> Result<(), String> {
        use windows_sys::Win32::System::Registry::REG_BINARY;
        write(key, value, REG_BINARY, data)
    }

    /// Delete a value. A key or a value that is not there is already gone.
    pub(crate) fn delete_value(key: &str, value: &str) -> Result<(), String> {
        let (key, value) = (wide(key), wide(value));
        let mut handle: HKEY = std::ptr::null_mut();
        // SAFETY: the key name is NUL-terminated UTF-16 that outlives the
        // call and `handle` is a live local the call writes the key to.
        #[allow(unsafe_code)]
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                KEY_SET_VALUE,
                &raw mut handle,
            )
        };
        match status {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(()),
            code => return Err(error(code)),
        }
        // SAFETY: `handle` was just opened for setting values and the name is
        // NUL-terminated UTF-16; the key is closed once and not used after.
        #[allow(unsafe_code)]
        let status = unsafe {
            let status = RegDeleteValueW(handle, value.as_ptr());
            RegCloseKey(handle);
            status
        };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            code => Err(error(code)),
        }
    }

    /// Delete a key and everything under it.
    #[cfg(test)]
    pub(crate) fn delete_tree(key: &str) -> Result<(), String> {
        use windows_sys::Win32::System::Registry::{RegDeleteKeyW, RegDeleteTreeW};

        let key = wide(key);
        // SAFETY: the key name is NUL-terminated UTF-16 that outlives both
        // calls, which take the predefined root by value.
        #[allow(unsafe_code)]
        let status = unsafe {
            let status = RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr());
            if status == ERROR_SUCCESS {
                RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr())
            } else {
                status
            }
        };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            code => Err(error(code)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_round_trips_through_the_c_runtime_rules() {
        for exe in [
            r"C:\Users\me\scoop\apps\sunlit-earth\current\sunlit-earth.exe",
            r"C:\Program Files\Sunlit Earth\sunlit-earth.exe",
            r"D:\odd  spaces\and\tabs	here\sunlit-earth.exe",
        ] {
            let line = command_line(Path::new(exe)).expect("a path without quotes");
            let mut expected = vec![exe.to_owned()];
            expected.extend(ARGS.iter().map(|arg| (*arg).to_owned()));
            assert_eq!(parse_command_line(&line), expected, "{line}");
        }
    }

    #[test]
    fn a_path_with_a_quote_is_refused() {
        assert!(command_line(Path::new(r#"C:\a"b\sunlit-earth.exe"#)).is_err());
    }

    #[test]
    fn the_parser_follows_the_c_runtime_rules_for_arguments() {
        assert_eq!(
            parse_command_line(r#"prog "a b" c\\\"d e\\"f g" h""i"#),
            ["prog", "a b", r#"c\"d"#, r"e\f g", "hi"]
        );
        assert_eq!(parse_command_line(r#"p "a""b""#), ["p", r#"a"b"#]);
        assert_eq!(parse_command_line(r"p a\\b"), ["p", r"a\\b"]);
    }

    #[test]
    fn startup_approved_reads_the_low_bit_of_the_first_byte() {
        let mut filetime_off = vec![0x03, 0, 0, 0];
        filetime_off.extend(0x01DA_1234_5678_9ABC_u64.to_le_bytes());
        let mut on = vec![0x02];
        on.extend([0; 11]);
        let mut off_zeros = vec![0x03];
        off_zeros.extend([0; 11]);

        assert!(approved(None), "no value is on");
        assert!(approved(Some(&on)));
        assert!(!approved(Some(&filetime_off)));
        assert!(!approved(Some(&off_zeros)), "a zero FILETIME is still off");
        assert!(approved(Some(&[0x06, 0, 0])));
        assert!(!approved(Some(&[0x07, 0, 0])));
        assert!(
            !approved(Some(&[])),
            "a value with no first byte is not an even one"
        );
    }

    #[test]
    fn the_state_names_the_executable_the_entry_runs() {
        let line = r#""C:\Other Copy\sunlit-earth.exe" --tray-start hidden"#;
        assert_eq!(
            state(Some(line), None),
            Autostart::On {
                exe: PathBuf::from(r"C:\Other Copy\sunlit-earth.exe")
            }
        );
        assert_eq!(
            state(Some(line), Some(&[3])),
            Autostart::TurnedOffBySystem {
                exe: PathBuf::from(r"C:\Other Copy\sunlit-earth.exe")
            }
        );
        assert_eq!(state(None, Some(&[2])), Autostart::Off);
    }

    #[cfg(windows)]
    mod scratch {
        use super::super::registry;
        use super::*;

        /// Keys under `HKCU\Software\SunlitEarthTest-<pid>-<case>`, removed
        /// when the case ends however it ends.
        struct ScratchKeys {
            root: String,
            location: Location,
        }

        impl ScratchKeys {
            fn new(case: &str) -> Self {
                let root = format!(r"Software\SunlitEarthTest-{}-{case}", std::process::id());
                let _ = registry::delete_tree(&root);
                Self {
                    location: Location {
                        run_key: format!(r"{root}\Run"),
                        approved_key: format!(r"{root}\StartupApproved\Run"),
                    },
                    root,
                }
            }
        }

        impl Drop for ScratchKeys {
            fn drop(&mut self) {
                let _ = registry::delete_tree(&self.root);
            }
        }

        #[test]
        fn enabling_over_a_switch_off_leaves_the_entry_on() {
            let keys = ScratchKeys::new("enable");
            let exe = Path::new(r"C:\Program Files\Sunlit Earth\sunlit-earth.exe");
            registry::write_binary(&keys.location.approved_key, VALUE_NAME, &[3, 0, 0, 0])
                .expect("the scratch switch-off");
            keys.location.enable(exe).expect("the entry is written");
            assert_eq!(
                keys.location.read(),
                Autostart::On {
                    exe: exe.to_path_buf()
                }
            );
            assert_eq!(
                registry::read_binary(&keys.location.approved_key, VALUE_NAME),
                Ok(None),
                "the old switch-off is gone"
            );
        }

        #[test]
        fn disabling_removes_both_values() {
            let keys = ScratchKeys::new("disable");
            let exe = Path::new(r"C:\sunlit-earth.exe");
            keys.location.enable(exe).expect("the entry is written");
            registry::write_binary(&keys.location.approved_key, VALUE_NAME, &[2; 12])
                .expect("the scratch approval");
            keys.location.disable().expect("the entry is removed");
            assert_eq!(keys.location.read(), Autostart::Off);
            assert_eq!(
                registry::read_string(&keys.location.run_key, VALUE_NAME),
                Ok(None)
            );
            assert_eq!(
                registry::read_binary(&keys.location.approved_key, VALUE_NAME),
                Ok(None)
            );
            keys.location
                .disable()
                .expect("removing what is not there is not an error");
        }

        #[test]
        fn another_copys_entry_reads_as_on_with_its_path() {
            let keys = ScratchKeys::new("other");
            registry::write_string(
                &keys.location.run_key,
                VALUE_NAME,
                r#""D:\Other\sunlit-earth.exe" --tray-start hidden"#,
            )
            .expect("the scratch entry");
            assert_eq!(
                keys.location.read(),
                Autostart::On {
                    exe: PathBuf::from(r"D:\Other\sunlit-earth.exe")
                }
            );
            registry::write_binary(&keys.location.approved_key, VALUE_NAME, &[7, 0, 0])
                .expect("the scratch switch-off");
            assert_eq!(
                keys.location.read(),
                Autostart::TurnedOffBySystem {
                    exe: PathBuf::from(r"D:\Other\sunlit-earth.exe")
                }
            );
        }
    }
}
