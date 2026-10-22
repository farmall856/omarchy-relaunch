//! Trusted account lookup: a uid's current login name from the password
//! database. The anchor and the service both bind `signed.user` to this, so an
//! account deleted and recreated under a different name is caught (spec §9).

use std::ffi::CStr;

/// Current login name for `uid`, or `None` if the uid has no passwd entry.
/// Uses `getpwuid_r` with a growing buffer; never trusts a caller-supplied name.
pub fn name_for_uid(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 1024];
    loop {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                uid as libc::uid_t,
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(pwd.pw_name) };
        return name.to_str().ok().map(|s| s.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_uid_resolves_to_a_name() {
        let uid = unsafe { libc::getuid() } as u32;
        let name = name_for_uid(uid).expect("current uid has a passwd entry");
        assert!(!name.is_empty());
    }

    #[test]
    fn nonexistent_uid_is_none() {
        assert!(name_for_uid(4_294_967_000).is_none());
    }
}
