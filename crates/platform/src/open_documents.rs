//! Z4K: files the operating system asks the app to open — a double-clicked `.fcproj` in the
//! Finder, a file dropped on the Dock icon, `open -a FilmCraft file`.
//!
//! On macOS these arrive as an "open documents" Apple Event, not as command-line arguments, and
//! the windowing library doesn't pass them on. [`install`] registers a handler for that event
//! (when the application is about to finish launching, so it also receives the files the app was
//! launched with); the UI collects them with [`take`] every frame, and [`set_waker`] lets the
//! handler wake an idle UI. Elsewhere the functions do nothing (other systems pass files as
//! arguments).

use std::sync::{Mutex, PoisonError};

static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WAKER: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

/// Most files kept between two frames (a hostile sender can't grow the queue without bound).
const MAX_QUEUED: usize = 256;

/// Queue `paths` for the UI and wake it.
fn push(paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    {
        let mut q = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
        for p in paths {
            if q.len() < MAX_QUEUED {
                q.push(p);
            }
        }
    }
    if let Some(w) = WAKER.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
        w();
    }
}

/// Files the system asked to open since the last call.
pub fn take() -> Vec<String> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Called when files arrive (e.g. to repaint an idle window). Call it on the main thread once
/// the application has launched: it also makes sure the handler is still the registered one.
pub fn set_waker(f: Box<dyn Fn() + Send>) {
    *WAKER.lock().unwrap_or_else(PoisonError::into_inner) = Some(f);
    #[cfg(target_os = "macos")]
    mac::reinstall();
}

/// Start receiving open-document requests. Call once, on the main thread, before the event loop
/// runs.
pub fn install() {
    #[cfg(target_os = "macos")]
    mac::install();
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{ClassType, define_class, msg_send, sel};
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSString};

    /// `kCoreEventClass` ('aevt'), `kAEOpenDocuments` ('odoc'), `keyDirectObject` ('----').
    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the class implements no Drop.
        #[unsafe(super(NSObject))]
        #[name = "FilmCraftOpenDocumentsHandler"]
        struct Handler;

        impl Handler {
            /// NSApplicationWillFinishLaunchingNotification: install the Apple Event handler now,
            /// after AppKit's own and before the launch's open-documents event is dispatched.
            #[unsafe(method(willFinishLaunching:))]
            fn will_finish_launching(&self, _note: &NSNotification) {
                log::info!("open documents: handler installed (will finish launching)");
                self.register();
            }

            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn handle_open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                let paths = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| paths_of(event))).unwrap_or_default();
                log::info!("open documents: {} file(s)", paths.len());
                super::push(paths);
            }
        }
    );

    impl Handler {
        /// Make this object the open-documents Apple Event handler.
        fn register(&self) {
            let manager = NSAppleEventManager::sharedAppleEventManager();
            // SAFETY: `-[NSAppleEventManager setEventHandler:andSelector:forEventClass:andEventID:]`
            // takes (id, SEL, AEEventClass = u32, AEEventID = u32) and returns void; `self`
            // lives for the rest of the process (leaked in `install`) and implements the
            // selector with the signature the manager calls.
            unsafe {
                let _: () = msg_send![
                    &*manager,
                    setEventHandler: self,
                    andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: OPEN_DOCUMENTS
                ];
            }
        }
    }

    thread_local! {
        static HANDLER: std::cell::OnceCell<Retained<Handler>> = const { std::cell::OnceCell::new() };
    }

    /// Register again (after launch, in case AppKit installed its own handler over ours).
    pub(super) fn reinstall() {
        HANDLER.with(|h| {
            if let Some(h) = h.get() {
                h.register();
                log::info!("open documents: handler installed (after launch)");
            }
        });
    }

    /// File paths in an open-documents event (its direct object: a list of file URLs, or one).
    fn paths_of(event: &NSAppleEventDescriptor) -> Vec<String> {
        // SAFETY: `-[NSAppleEventDescriptor paramDescriptorForKeyword:]` takes an AEKeyword (u32)
        // and returns a nullable, autoreleased descriptor.
        let list: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
        let Some(list) = list else { return Vec::new() };
        let path_of = |d: &NSAppleEventDescriptor| d.fileURLValue().and_then(|u| u.path()).map(|p| p.to_string());
        let n = list.numberOfItems().clamp(0, super::MAX_QUEUED as isize);
        if n == 0 {
            return path_of(&list).into_iter().collect();
        }
        // descriptors are numbered from 1
        (1..=n).filter_map(|i| list.descriptorAtIndex(i)).filter_map(|d| path_of(&d)).collect()
    }

    pub(super) fn install() {
        // SAFETY: `new` on an NSObject subclass without instance variables returns an initialised
        // object.
        let handler: Retained<Handler> = unsafe { msg_send![Handler::class(), new] };
        let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
        // SAFETY: the observer is never released (below), so the notification center never
        // messages a freed object; the selector takes one NSNotification argument.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(&handler, sel!(willFinishLaunching:), Some(&name), None);
        }
        // the handler lives for the rest of the process (main thread)
        HANDLER.with(|h| {
            let _ = h.set(handler);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_files_are_taken_once_and_the_queue_is_bounded() {
        let woke = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woke.clone();
        set_waker(Box::new(move || {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let _ = take();
        push(vec!["/a.fcproj".into(), "/b.mov".into()]);
        assert_eq!(take(), ["/a.fcproj", "/b.mov"]);
        assert!(take().is_empty());
        push(Vec::new());
        push((0..1000).map(|i| format!("/{i}")).collect());
        assert_eq!(take().len(), MAX_QUEUED);
        assert_eq!(woke.load(std::sync::atomic::Ordering::SeqCst), 2, "an empty push doesn't wake");
    }
}
