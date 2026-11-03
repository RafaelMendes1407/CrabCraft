//! Files the system asks CrabCraft to open: Finder's "Open With" and double-clicks on
//! associated files (macOS sends them as an Apple Event, not as arguments), and the ones later
//! `crabcraft` calls hand over (see `instance`), queued for the UI.
//!
//! winit has no API for it, so `application:openURLs:` is added to `NSObject` before the app
//! starts: winit's application delegate inherits it and AppKit delivers the files there, also
//! the ones that launch the app (they arrive before the first frame and wait in the queue).

use std::sync::{Mutex, OnceLock};

use crate::cli::OpenRequest;

static PENDING: Mutex<Vec<OpenRequest>> = Mutex::new(Vec::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Wakes the UI when files arrive while it is idle.
pub fn set_context(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
}

/// Files received since the last call.
pub fn take() -> Vec<OpenRequest> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

pub fn push(requests: Vec<OpenRequest>) {
    if requests.is_empty() {
        return;
    }
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).extend(requests);
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

#[cfg(target_os = "macos")]
pub fn install() {
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::sel;
    use objc2_foundation::{NSArray, NSURL};

    unsafe extern "C-unwind" fn open_urls(_this: &AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *const NSArray<NSURL>) {
        let Some(urls) = (unsafe { urls.as_ref() }) else { return };
        let paths = urls.iter().filter_map(|u| u.path()).map(|p| OpenRequest::path(std::path::PathBuf::from(p.to_string()))).collect();
        push(paths);
    }

    let Some(class) = AnyClass::get(c"NSObject") else { return };
    let imp: Imp = unsafe { std::mem::transmute::<unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject, *const NSArray<NSURL>), Imp>(open_urls) };
    // SAFETY: the type encoding matches the function (void, self, _cmd, two objects); adding a
    // method only fails when the class already has one, and then nothing changes.
    unsafe {
        objc2::ffi::class_addMethod(class as *const AnyClass as *mut AnyClass, sel!(application:openURLs:), imp, c"v@:@@".as_ptr());
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install() {}

#[cfg(test)]
mod tests {
    #[test]
    fn queued_files_are_taken_once() {
        use crate::cli::OpenRequest;
        super::push(vec![OpenRequest::path("/tmp/a.txt"), OpenRequest { path: "/tmp/b.json".into(), line: Some(2), column: None }]);
        assert_eq!(super::take(), vec![OpenRequest::path("/tmp/a.txt"), OpenRequest { path: "/tmp/b.json".into(), line: Some(2), column: None }]);
        assert!(super::take().is_empty());
    }
}
