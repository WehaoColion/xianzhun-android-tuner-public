use crate::capture::{
    run_capture, CaptureAttempt, CaptureConfig, CaptureOutcome, CaptureSignalStatus,
    DigitalSilenceWatchdog, FrameWatchdog,
};
use crate::input::InputFrame;
use crate::dial;
use crate::pitch::{PitchAnalyzer, PitchReading};
use crate::selection::select_target;
use crate::tuning::STRINGS;
use crate::ui_state::{
    permission_request_should_settle, progress_accessibility_description, AccessibilityState,
    InputStatus, UiPhase, UiSnapshot, UiState,
};
use jni::errors::Result as JniResult;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jobject};
use jni::{JNIEnv, JavaVM};
use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_long, c_void, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const RECORD_AUDIO: &str = "android.permission.RECORD_AUDIO";
const PERMISSION_REQUEST_CODE: i32 = 3110;
const FRAME_SIZE: usize = 4096;
const ANALYSIS_HOP: usize = 2048;
const UI_POLL_MILLIS: c_long = 16;
const WINDOW_FORMAT_RGBA_8888: i32 = 1;
const AUDIO_READ_NON_BLOCKING: i32 = 1;
const AUDIO_IDLE_POLL: Duration = Duration::from_millis(8);
const MATCH_PARENT: i32 = -1;
const WRAP_CONTENT: i32 = -2;
const VERTICAL: i32 = 1;
const HORIZONTAL: i32 = 0;
const GRAVITY_CENTER: i32 = 17;
const GRAVITY_CENTER_HORIZONTAL: i32 = 1;
const TEXT_UNIT_SP: i32 = 2;
const INPUT_QUEUE_IDENT: i32 = 7;
const ALOOPER_PREPARE_ALLOW_NON_CALLBACKS: i32 = 1;
const AINPUT_EVENT_TYPE_MOTION: i32 = 2;
const AMOTION_EVENT_ACTION_MASK: i32 = 0xff;
const AMOTION_EVENT_ACTION_DOWN: i32 = 0;
const AMOTION_EVENT_ACTION_UP: i32 = 1;
const AMOTION_EVENT_ACTION_MOVE: i32 = 2;
const AMOTION_EVENT_ACTION_CANCEL: i32 = 3;
const AMOTION_EVENT_ACTION_POINTER_DOWN: i32 = 5;

// Warm-wood and brass palette follows the classical-instrument concept.
// Contrast and actual tuning state are prioritised over photo-real textures.
const MIDNIGHT: i32 = color(0x16, 0x0e, 0x0c);
const WOOD_TOP: i32 = color(0x31, 0x1e, 0x17);
const PANEL: i32 = color(0x28, 0x19, 0x15);
const PANEL_STROKE: i32 = color(0x82, 0x5b, 0x3d);
const WARM_WHITE: i32 = color(0xf9, 0xe9, 0xd0);
const MUTED_TEXT: i32 = color(0xc3, 0xa8, 0x91);
const BRASS: i32 = color(0xc0, 0x82, 0x44);
const BRASS_SOFT: i32 = color(0xf0, 0xca, 0x8c);
const TUNED_GREEN: i32 = color(0x99, 0xcf, 0x82);
const FLAT_SHARP: i32 = color(0xed, 0xa0, 0x84);
const CHIP_IDLE: i32 = color(0x35, 0x24, 0x1e);
const CHIP_ACTIVE: i32 = color(0x85, 0x4c, 0x2b);
const CHIP_TUNED: i32 = color(0x2d, 0x4c, 0x31);
const CHIP_DETECTED: i32 = color(0x5d, 0x43, 0x2d);

const fn color(red: u8, green: u8, blue: u8) -> i32 {
    ((0xff_u32 << 24) | ((red as u32) << 16) | ((green as u32) << 8) | blue as u32) as i32
}

#[repr(C)]
pub struct ANativeActivity {
    callbacks: *mut ANativeActivityCallbacks,
    vm: *mut jni::sys::JavaVM,
    env: *mut jni::sys::JNIEnv,
    clazz: jobject,
    internal_data_path: *const c_char,
    external_data_path: *const c_char,
    sdk_version: i32,
    instance: *mut c_void,
    asset_manager: *mut c_void,
    obb_path: *const c_char,
}

#[repr(C)]
struct ANativeActivityCallbacks {
    on_start: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    on_resume: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    on_save_instance_state:
        Option<unsafe extern "C" fn(*mut ANativeActivity, *mut usize) -> *mut c_void>,
    on_pause: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    on_stop: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    on_destroy: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    on_window_focus_changed: Option<unsafe extern "C" fn(*mut ANativeActivity, c_int)>,
    on_native_window_created:
        Option<unsafe extern "C" fn(*mut ANativeActivity, *mut ANativeWindow)>,
    on_native_window_resized:
        Option<unsafe extern "C" fn(*mut ANativeActivity, *mut ANativeWindow)>,
    on_native_window_redraw_needed:
        Option<unsafe extern "C" fn(*mut ANativeActivity, *mut ANativeWindow)>,
    on_native_window_destroyed:
        Option<unsafe extern "C" fn(*mut ANativeActivity, *mut ANativeWindow)>,
    on_input_queue_created: Option<unsafe extern "C" fn(*mut ANativeActivity, *mut AInputQueue)>,
    on_input_queue_destroyed: Option<unsafe extern "C" fn(*mut ANativeActivity, *mut AInputQueue)>,
    on_content_rect_changed: Option<unsafe extern "C" fn(*mut ANativeActivity, *const ARect)>,
    on_configuration_changed: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    on_low_memory: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
}

#[repr(C)]
struct ARect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

enum ANativeWindow {}
enum AInputQueue {}
enum AInputEvent {}
enum ALooper {}
enum AChoreographer {}

#[link(name = "android")]
extern "C" {
    fn ANativeActivity_setWindowFormat(activity: *mut ANativeActivity, format: i32);
    fn ALooper_prepare(opts: c_int) -> *mut ALooper;
    fn ALooper_pollOnce(
        timeout_millis: c_int,
        out_fd: *mut c_int,
        out_events: *mut c_int,
        out_data: *mut *mut c_void,
    ) -> c_int;
    fn AInputQueue_attachLooper(
        queue: *mut AInputQueue,
        looper: *mut ALooper,
        ident: c_int,
        callback: Option<extern "C" fn(c_int, c_int, *mut c_void) -> c_int>,
        data: *mut c_void,
    );
    fn AInputQueue_detachLooper(queue: *mut AInputQueue);
    fn AInputQueue_getEvent(queue: *mut AInputQueue, out_event: *mut *mut AInputEvent) -> c_int;
    fn AInputQueue_preDispatchEvent(queue: *mut AInputQueue, event: *mut AInputEvent) -> c_int;
    fn AInputQueue_finishEvent(queue: *mut AInputQueue, event: *mut AInputEvent, handled: c_int);
    fn AInputEvent_getType(event: *const AInputEvent) -> c_int;
    fn AMotionEvent_getAction(event: *const AInputEvent) -> c_int;
    fn AMotionEvent_getX(event: *const AInputEvent, pointer_index: usize) -> f32;
    fn AMotionEvent_getY(event: *const AInputEvent, pointer_index: usize) -> f32;
    fn AMotionEvent_getPointerCount(event: *const AInputEvent) -> usize;
    fn AChoreographer_getInstance() -> *mut AChoreographer;
    fn AChoreographer_postFrameCallbackDelayed(
        choreographer: *mut AChoreographer,
        callback: unsafe extern "C" fn(c_long, *mut c_void),
        data: *mut c_void,
        delay_millis: c_long,
    );
}

#[link(name = "log")]
extern "C" {
    fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

struct OverlayViews {
    root: GlobalRef,
    scroll: GlobalRef,
    body: GlobalRef,
    hero: GlobalRef,
    action: GlobalRef,
    auto: GlobalRef,
    note: GlobalRef,
    caption: GlobalRef,
    frequency: GlobalRef,
    cents: GlobalRef,
    status: GlobalRef,
    level: GlobalRef,
    microphone: GlobalRef,
    gauge: GlobalRef,
    needle: GlobalRef,
    pivot: GlobalRef,
    gauge_labels: [GlobalRef; 5],
    chips: [GlobalRef; 6],
    touch_slop_px: f32,
}

struct SyncedViews {
    snapshot: UiSnapshot,
    action_checked: bool,
    action_enabled: bool,
}

struct AppState {
    vm: JavaVM,
    activity: GlobalRef,
    views: OverlayViews,
    ui: Mutex<UiState>,
    last_synced: Mutex<Option<SyncedViews>>,
    running: AtomicBool,
    resumed: AtomicBool,
    main_loop_active: AtomicBool,
    listening_requested: AtomicBool,
    permission_attempts: AtomicU32,
    permission_request_in_flight: AtomicBool,
    permission_window_left: AtomicBool,
    permission_request_started_at: Mutex<Option<Instant>>,
    audio_retry_allowed: AtomicBool,
    audio: Mutex<Option<AudioHandle>>,
    input: Mutex<Option<InputHandle>>,
    input_queue: Mutex<Option<usize>>,
    pending_inputs: Mutex<VecDeque<PendingInput>>,
    accessibility: Mutex<AccessibilityState>,
}

struct AudioControl {
    running: AtomicBool,
}

struct AudioHandle {
    control: Arc<AudioControl>,
    thread: Option<JoinHandle<()>>,
}

struct InputHandle {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct MainLoopContext {
    app: Weak<AppState>,
    choreographer: *mut AChoreographer,
}

enum PendingInput {
    Press { x: f32, y: f32 },
    ClearPress,
    Tap { down_x: f32, down_y: f32, x: f32, y: f32 },
    Scroll { down_x: f32, down_y: f32, delta: f32 },
}

#[derive(Default)]
struct TouchGesture {
    active: bool,
    down_x: f32,
    down_y: f32,
    last_y: f32,
    moved: bool,
}

impl AppState {
    fn new(vm: JavaVM, activity: GlobalRef, views: OverlayViews) -> Arc<Self> {
        Arc::new(Self {
            vm,
            activity,
            views,
            ui: Mutex::new(UiState::new()),
            last_synced: Mutex::new(None),
            running: AtomicBool::new(true),
            resumed: AtomicBool::new(false),
            main_loop_active: AtomicBool::new(false),
            listening_requested: AtomicBool::new(false),
            permission_attempts: AtomicU32::new(0),
            permission_request_in_flight: AtomicBool::new(false),
            permission_window_left: AtomicBool::new(false),
            permission_request_started_at: Mutex::new(None),
            audio_retry_allowed: AtomicBool::new(true),
            audio: Mutex::new(None),
            input: Mutex::new(None),
            input_queue: Mutex::new(None),
            pending_inputs: Mutex::new(VecDeque::new()),
            accessibility: Mutex::new(AccessibilityState::default()),
        })
    }

    fn start_main_loop(app: &Arc<Self>) {
        if !app.running.load(Ordering::SeqCst)
            || app
                .main_loop_active
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return;
        }
        let choreographer = unsafe { AChoreographer_getInstance() };
        if choreographer.is_null() {
            app.main_loop_active.store(false, Ordering::SeqCst);
            return;
        }
        let context = Box::new(MainLoopContext {
            app: Arc::downgrade(app),
            choreographer,
        });
        unsafe {
            AChoreographer_postFrameCallbackDelayed(
                choreographer,
                main_loop_tick,
                Box::into_raw(context) as *mut c_void,
                0,
            );
        }
    }

    fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.main_loop_active.store(false, Ordering::SeqCst);
        self.listening_requested.store(false, Ordering::SeqCst);
        self.stop_audio();
        *self.input_queue.lock().unwrap() = None;
        self.stop_input();
    }

    fn on_resume(app: &Arc<Self>) {
        app.resumed.store(true, Ordering::SeqCst);
        Self::start_main_loop(app);
        Self::start_input_if_ready(app);
        let Ok(mut env) = app.vm.attach_current_thread() else {
            return;
        };
        if app.has_audio_permission(&mut env) {
            if app.listening_requested.load(Ordering::SeqCst) {
                app.audio_retry_allowed.store(true, Ordering::SeqCst);
                app.ui.lock().unwrap().show_listening();
                Self::start_audio_if_ready(app);
            } else {
                app.ui.lock().unwrap().show_ready();
            }
        } else {
            if !app.permission_request_in_flight.load(Ordering::SeqCst) {
                app.listening_requested.store(false, Ordering::SeqCst);
            }
            app.update_permission_ui(&mut env);
        }
        app.sync_views(&mut env, true);
    }

    fn on_pause(&self) {
        self.resumed.store(false, Ordering::SeqCst);
        self.stop_audio();
        self.stop_input();
        self.pending_inputs.lock().unwrap().clear();
        if let Ok(mut env) = self.vm.attach_current_thread() { self.update_pressed(&mut env, None); }
        if !self.permission_request_in_flight.load(Ordering::SeqCst) {
            self.listening_requested.store(false, Ordering::SeqCst);
            self.ui.lock().unwrap().show_paused();
            if let Ok(mut env) = self.vm.attach_current_thread() {
                self.sync_views(&mut env, true);
            }
        }
    }

    fn on_window_focus(app: &Arc<Self>, focused: bool) {
        if !focused {
            if app.permission_request_in_flight.load(Ordering::SeqCst) {
                app.permission_window_left.store(true, Ordering::SeqCst);
            }
            return;
        }
        if !app.permission_window_left.swap(false, Ordering::SeqCst)
            || !app
                .permission_request_in_flight
                .swap(false, Ordering::SeqCst)
        {
            return;
        }
        *app.permission_request_started_at.lock().unwrap() = None;
        let Ok(mut env) = app.vm.attach_current_thread() else {
            return;
        };
        Self::resolve_permission_result(app, &mut env);
    }

    fn resolve_permission_result(app: &Arc<Self>, env: &mut JNIEnv) {
        if app.has_audio_permission(env) {
            if app.listening_requested.load(Ordering::SeqCst) {
                app.audio_retry_allowed.store(true, Ordering::SeqCst);
                app.ui.lock().unwrap().show_listening();
                Self::start_audio_if_ready(app);
            } else {
                app.ui.lock().unwrap().show_ready();
            }
        } else {
            app.listening_requested.store(false, Ordering::SeqCst);
            app.update_permission_ui(env);
        }
        app.sync_views(env, true);
    }

    fn settle_permission_request_if_ready(app: &Arc<Self>, env: &mut JNIEnv) {
        if !app.permission_request_in_flight.load(Ordering::SeqCst) {
            return;
        }
        let elapsed = app
            .permission_request_started_at
            .lock()
            .unwrap()
            .map(|started| started.elapsed());
        if !permission_request_should_settle(true, app.has_window_focus(env), elapsed) {
            return;
        }
        if app
            .permission_request_in_flight
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        *app.permission_request_started_at.lock().unwrap() = None;
        Self::resolve_permission_result(app, env);
    }

    fn main_tick(app: &Arc<Self>, env: &mut JNIEnv) {
        if !app.resumed.load(Ordering::SeqCst) {
            return;
        }
        app.reap_finished_audio();
        Self::settle_permission_request_if_ready(app, env);
        Self::poll_controls(app, env);
        Self::process_pending_inputs(app, env);
        app.ui.lock().unwrap().expire_reading(Instant::now());

        if app.listening_requested.load(Ordering::SeqCst)
            && app.audio_retry_allowed.load(Ordering::SeqCst)
            && app.audio.lock().unwrap().is_none()
            && !app.permission_request_in_flight.load(Ordering::SeqCst)
            && app.has_audio_permission(env)
        {
            Self::start_audio_if_ready(app);
        }
        app.sync_views(env, false);
    }

    // Only keyboard/accessibility activations take the framework toggle route
    // when the native input queue is present. Both routes dispatch the same
    // commands, never infer recording changes from an old pitch snapshot.
    fn poll_controls(app: &Arc<Self>, env: &mut JNIEnv) {
        let Some((last_action, last_selected)) = app.last_synced.lock().unwrap()
            .as_ref().map(|s| (s.action_checked, s.snapshot.selected_target)) else { return; };
        let read_checked = |env: &mut JNIEnv, view: &JObject| -> Option<bool> {
            let value = env.call_method(view, "isChecked", "()Z", &[]).and_then(|value| value.z()).ok();
            clear_exception(env);
            value
        };
        if let Some(action) = read_checked(env, app.views.action.as_obj()) {
            if !app.permission_request_in_flight.load(Ordering::SeqCst)
                && action != last_action
                && app.listening_requested.load(Ordering::SeqCst) == last_action
            {
                Self::handle_action(app, env);
            }
        }
        let current = app.ui.lock().unwrap().selected_target;
        if current == last_selected {
            let mut selected = None;
            for (index, chip) in app.views.chips.iter().enumerate() {
                if Some(index) != current && read_checked(env, chip.as_obj()) == Some(true) {
                    selected = Some(index);
                }
            }
            if let Some(index) = selected {
                Self::handle_target_selection(app, Some(index));
            } else if current.is_some() && read_checked(env, app.views.auto.as_obj()) == Some(true) {
                Self::handle_target_selection(app, None);
            }
            // Tapping the already selected option is idempotent. Restore its
            // checked appearance, rather than interpreting uncheck as unlock.
        }
        app.sync_views(env, false);
        app.reconcile_checked_controls(env);
    }

    fn reconcile_checked_controls(&self, env: &mut JNIEnv) {
        let selected = self.ui.lock().unwrap().selected_target;
        for (view, expected) in std::iter::once((&self.views.auto, selected.is_none()))
            .chain(self.views.chips.iter().enumerate().map(|(i, view)| (view, selected == Some(i))))
        {
            if env.call_method(view.as_obj(), "isChecked", "()Z", &[])
                .and_then(|value| value.z()).ok() != Some(expected) {
                clear_exception(env);
                let _ = env.call_method(view.as_obj(), "setChecked", "(Z)V",
                    &[JValue::Bool(jboolean::from(expected))]);
                clear_exception(env);
            }
        }
    }

    fn process_pending_inputs(app: &Arc<Self>, env: &mut JNIEnv) {
        let events = std::mem::take(&mut *app.pending_inputs.lock().unwrap());
        for event in events {
            if !app.resumed.load(Ordering::SeqCst) { break; }
            match event {
                PendingInput::Press { x, y } => {
                    app.update_pressed(env, Some((x, y)));
                }
                PendingInput::ClearPress => app.update_pressed(env, None),
                PendingInput::Tap { down_x, down_y, x, y } => {
                    app.update_pressed(env, None);
                    let hit = |env: &mut JNIEnv, view: &JObject| {
                        view_contains_point(env, view, down_x, down_y)
                            && view_contains_point(env, view, x, y)
                    };
                    if hit(env, app.views.action.as_obj()) {
                        Self::handle_action(app, env);
                    } else if hit(env, app.views.auto.as_obj()) {
                        Self::handle_target_selection(app, None);
                    } else {
                        for (index, chip) in app.views.chips.iter().enumerate() {
                            if hit(env, chip.as_obj()) {
                                Self::handle_target_selection(app, Some(index));
                                break;
                            }
                        }
                    }
                    app.sync_views(env, false);
                }
                PendingInput::Scroll { down_x, down_y, delta } => {
                    app.update_pressed(env, None);
                    if view_contains_point(env, app.views.scroll.as_obj(), down_x, down_y) {
                        let _ = env.call_method(app.views.scroll.as_obj(), "scrollBy", "(II)V",
                            &[JValue::Int(0), JValue::Int(delta.round() as i32)]);
                        clear_exception(env);
                    }
                }
            }
        }
    }

    fn update_pressed(&self, env: &mut JNIEnv, point: Option<(f32, f32)>) {
        for view in std::iter::once(&self.views.action).chain(std::iter::once(&self.views.auto)).chain(self.views.chips.iter()) {
            let pressed = point.is_some_and(|(x, y)| view_contains_point(env, view.as_obj(), x, y));
            let _ = env.call_method(view.as_obj(), "setPressed", "(Z)V", &[JValue::Bool(jboolean::from(pressed))]);
            clear_exception(env);
        }
    }

    fn handle_target_selection(app: &Arc<Self>, selected: Option<usize>) {
        select_target(&mut app.ui.lock().unwrap(), selected);
    }

    fn handle_action(app: &Arc<Self>, env: &mut JNIEnv) {
        if !app.resumed.load(Ordering::SeqCst)
            || app.permission_request_in_flight.load(Ordering::SeqCst)
        {
            return;
        }
        if app.has_audio_permission(env) {
            if app.listening_requested.swap(false, Ordering::SeqCst) {
                app.stop_audio();
                app.ui.lock().unwrap().show_stopped();
                return;
            }
            app.listening_requested.store(true, Ordering::SeqCst);
            app.audio_retry_allowed.store(true, Ordering::SeqCst);
            app.ui.lock().unwrap().show_listening();
            Self::start_audio_if_ready(app);
            return;
        }

        app.listening_requested.store(true, Ordering::SeqCst);
        let attempts = app.permission_attempts.load(Ordering::SeqCst);
        let rationale = app.should_show_permission_rationale(env);
        if attempts > 0 && !rationale {
            app.open_app_settings(env);
        } else {
            app.request_audio_permission(env);
        }
    }

    fn start_audio_if_ready(app: &Arc<Self>) {
        if !app.running.load(Ordering::SeqCst)
            || !app.resumed.load(Ordering::SeqCst)
            || !app.listening_requested.load(Ordering::SeqCst)
            || !app.audio_retry_allowed.load(Ordering::SeqCst)
        {
            return;
        }
        let mut slot = app.audio.lock().unwrap();
        if slot.is_some() {
            return;
        }

        let control = Arc::new(AudioControl {
            running: AtomicBool::new(true),
        });
        let worker_control = Arc::clone(&control);
        let worker = Arc::clone(app);
        let handle = thread::spawn(move || audio_loop(worker, worker_control));
        *slot = Some(AudioHandle {
            control,
            thread: Some(handle),
        });
    }

    fn reap_finished_audio(&self) {
        let finished = self
            .audio
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|handle| handle.thread.as_ref())
            .map(JoinHandle::is_finished)
            .unwrap_or(false);
        if finished {
            if let Some(mut handle) = self.audio.lock().unwrap().take() {
                if let Some(thread) = handle.thread.take() {
                    let _ = thread.join();
                }
            }
        }
    }

    fn stop_audio(&self) {
        // The worker owns AudioRecord through release.  Never wait for a vendor
        // audio call on the activity thread, and retain its slot until release
        // so a quick stop/start cannot create two microphone owners.
        if let Some(handle) = self.audio.lock().unwrap().as_ref() {
            handle.control.running.store(false, Ordering::SeqCst);
        }
    }

    fn set_input_queue(app: &Arc<Self>, queue: *mut AInputQueue) {
        app.stop_input();
        *app.input_queue.lock().unwrap() = Some(queue as usize);
        Self::start_input_if_ready(app);
    }

    fn start_input_if_ready(app: &Arc<Self>) {
        if !app.running.load(Ordering::SeqCst) || !app.resumed.load(Ordering::SeqCst) {
            return;
        }
        let Some(queue_addr) = *app.input_queue.lock().unwrap() else {
            return;
        };
        let mut slot = app.input.lock().unwrap();
        if slot.is_some() {
            return;
        }
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let worker = Arc::clone(app);
        let handle = thread::spawn(move || input_loop(worker, worker_running, queue_addr));
        *slot = Some(InputHandle {
            running,
            thread: Some(handle),
        });
    }

    fn clear_input_queue(&self, queue: *mut AInputQueue) {
        let queue_addr = queue as usize;
        let should_clear = *self.input_queue.lock().unwrap() == Some(queue_addr);
        if should_clear {
            self.stop_input();
            *self.input_queue.lock().unwrap() = None;
        }
    }

    fn stop_input(&self) {
        if let Some(mut handle) = self.input.lock().unwrap().take() {
            handle.running.store(false, Ordering::SeqCst);
            if let Some(thread) = handle.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn has_audio_permission(&self, env: &mut JNIEnv) -> bool {
        let result = env.with_local_frame(8, |env| -> JniResult<bool> {
            let permission = env.new_string(RECORD_AUDIO)?;
            let permission_obj: JObject = permission.into();
            Ok(env
                .call_method(
                    self.activity.as_obj(),
                    "checkSelfPermission",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&permission_obj)],
                )?
                .i()?
                == 0)
        });
        clear_exception(env);
        result.unwrap_or(false)
    }

    fn should_show_permission_rationale(&self, env: &mut JNIEnv) -> bool {
        let result = env.with_local_frame(8, |env| -> JniResult<bool> {
            let permission = env.new_string(RECORD_AUDIO)?;
            let permission_obj: JObject = permission.into();
            env.call_method(
                self.activity.as_obj(),
                "shouldShowRequestPermissionRationale",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&permission_obj)],
            )?
            .z()
        });
        clear_exception(env);
        result.unwrap_or(false)
    }

    fn has_window_focus(&self, env: &mut JNIEnv) -> bool {
        let result = env
            .call_method(self.activity.as_obj(), "hasWindowFocus", "()Z", &[])
            .and_then(|value| value.z());
        clear_exception(env);
        result.unwrap_or(false)
    }

    fn update_permission_ui(&self, env: &mut JNIEnv) {
        let attempts = self.permission_attempts.load(Ordering::SeqCst);
        let rationale = self.should_show_permission_rationale(env);
        let permanent = attempts > 0 && !rationale;
        self.ui
            .lock()
            .unwrap()
            .show_permission_needed(permanent, rationale);
    }

    fn request_audio_permission(&self, env: &mut JNIEnv) {
        self.permission_window_left.store(false, Ordering::SeqCst);
        *self.permission_request_started_at.lock().unwrap() = Some(Instant::now());
        self.permission_request_in_flight
            .store(true, Ordering::SeqCst);
        let result = env.with_local_frame(16, |env| -> JniResult<()> {
            let string_class: JClass = env.find_class("java/lang/String")?;
            let permission = env.new_string(RECORD_AUDIO)?;
            let permissions = env.new_object_array(1, string_class, JObject::null())?;
            env.set_object_array_element(&permissions, 0, permission)?;
            env.call_method(
                self.activity.as_obj(),
                "requestPermissions",
                "([Ljava/lang/String;I)V",
                &[
                    JValue::Object(permissions.as_ref()),
                    JValue::Int(PERMISSION_REQUEST_CODE),
                ],
            )?;
            Ok(())
        });
        if result.is_ok() {
            self.permission_attempts.fetch_add(1, Ordering::SeqCst);
        } else {
            clear_exception(env);
            self.permission_request_in_flight
                .store(false, Ordering::SeqCst);
            *self.permission_request_started_at.lock().unwrap() = None;
            self.listening_requested.store(false, Ordering::SeqCst);
            self.update_permission_ui(env);
        }
        clear_exception(env);
    }

    fn open_app_settings(&self, env: &mut JNIEnv) {
        self.permission_window_left.store(false, Ordering::SeqCst);
        *self.permission_request_started_at.lock().unwrap() = Some(Instant::now());
        self.permission_request_in_flight
            .store(true, Ordering::SeqCst);
        let result = env.with_local_frame(24, |env| -> JniResult<()> {
            let action = env.new_string("android.settings.APPLICATION_DETAILS_SETTINGS")?;
            let action_obj: JObject = action.into();
            let intent = env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&action_obj)],
            )?;
            // Resolve the installed identity rather than hard-coding the
            // production package: PR previews use a separate application ID.
            let package_name_obj = env
                .call_method(
                    self.activity.as_obj(),
                    "getPackageName",
                    "()Ljava/lang/String;",
                    &[],
                )?
                .l()?;
            let package_name: String = env
                .get_string(&JString::from(package_name_obj))?
                .into();
            let package_uri = env.new_string(format!("package:{package_name}"))?;
            let package_uri_obj: JObject = package_uri.into();
            let uri = env
                .call_static_method(
                    "android/net/Uri",
                    "parse",
                    "(Ljava/lang/String;)Landroid/net/Uri;",
                    &[JValue::Object(&package_uri_obj)],
                )?
                .l()?;
            env.call_method(
                &intent,
                "setData",
                "(Landroid/net/Uri;)Landroid/content/Intent;",
                &[JValue::Object(&uri)],
            )?;
            env.call_method(
                self.activity.as_obj(),
                "startActivity",
                "(Landroid/content/Intent;)V",
                &[JValue::Object(&intent)],
            )?;
            Ok(())
        });
        if result.is_err() {
            clear_exception(env);
            self.permission_request_in_flight
                .store(false, Ordering::SeqCst);
            *self.permission_request_started_at.lock().unwrap() = None;
            self.listening_requested.store(false, Ordering::SeqCst);
            self.update_permission_ui(env);
        }
        clear_exception(env);
    }

    fn apply_input_frame(
        &self,
        control: &AudioControl,
        frame: InputFrame,
        reading: Option<PitchReading>,
        system_silenced: bool,
        target_epoch: u64,
    ) {
        let mut ui = self.ui.lock().unwrap();
        // Stop/pause sets its flag before taking this same lock.  Late worker
        // frames cannot restore a reading after the UI has been reset.
        if capture_active(self, control) && ui.target_epoch == target_epoch {
            ui.apply_input_frame(frame, reading, system_silenced);
        }
    }

    fn show_capture_status(&self, control: &AudioControl, system_silenced: bool) {
        let mut ui = self.ui.lock().unwrap();
        if capture_active(self, control) {
            if system_silenced {
                ui.show_system_silenced();
            } else {
                ui.show_capture_starting();
            }
        }
    }

    fn set_microphone_error(&self, control: &AudioControl) {
        let mut ui = self.ui.lock().unwrap();
        if capture_active(self, control) {
            self.listening_requested.store(false, Ordering::SeqCst);
            self.audio_retry_allowed.store(false, Ordering::SeqCst);
            ui.show_microphone_error();
        }
    }

    fn sync_views(&self, env: &mut JNIEnv, force: bool) {
        let mut last_synced = self.last_synced.lock().unwrap();
        let ui = self.ui.lock().unwrap();
        let action_checked = self.listening_requested.load(Ordering::SeqCst);
        let action_enabled = !self.permission_request_in_flight.load(Ordering::SeqCst);
        if !force
            && last_synced.as_ref().is_some_and(|last| {
                last.snapshot.revision == ui.revision
                    && last.action_checked == action_checked
                    && last.action_enabled == action_enabled
            })
        {
            return;
        }
        let snapshot = ui.snapshot();
        drop(ui);
        let previous = last_synced.as_ref().filter(|_| !force);
        let old = previous.map(|last| &last.snapshot);
        let action_changed = previous.is_none_or(|last| last.action_checked != action_checked);
        let enabled_changed = previous.is_none_or(|last| last.action_enabled != action_enabled);
        let button_changed = old.is_none_or(|old| old.button != snapshot.button);
        let reading_changed = old.is_none_or(|old| {
            old.in_tune != snapshot.in_tune
                || old.phase != snapshot.phase
                || old.needle_cents.is_some() != snapshot.needle_cents.is_some()
                || old.needle_cents.is_some_and(|c| c.abs() > 4.0)
                    != snapshot.needle_cents.is_some_and(|c| c.abs() > 4.0)
        });
        let announcement = if self.resumed.load(Ordering::SeqCst) {
            self.accessibility
                .lock()
                .unwrap()
                .next_announcement(&snapshot)
        } else {
            None
        };

        let sync_result = env.with_local_frame(96, |env| -> JniResult<()> {
            let density = display_density(env, self.activity.as_obj()).max(1.0);
            if button_changed {
                set_toggle_text(env, self.views.action.as_obj(), &snapshot.button)?;
            }
            if action_changed {
                env.call_method(
                    self.views.action.as_obj(),
                    "setChecked",
                    "(Z)V",
                    &[JValue::Bool(jboolean::from(action_checked))],
                )?;
            }
            if enabled_changed {
                env.call_method(
                    self.views.action.as_obj(),
                    "setEnabled",
                    "(Z)V",
                    &[JValue::Bool(jboolean::from(action_enabled))],
                )?;
            }
            if action_changed {
                set_tactile_background(
                    env,
                    self.views.action.as_obj(),
                    if action_checked { &[CHIP_ACTIVE, PANEL] } else { &[BRASS_SOFT, BRASS] },
                    BRASS_SOFT,
                    20.0 * density,
                    density.round() as i32,
                )?;
                env.call_method(
                    self.views.action.as_obj(),
                    "setTextColor",
                    "(I)V",
                    &[JValue::Int(if action_checked { WARM_WHITE } else { MIDNIGHT })],
                )?;
            }
            if button_changed || action_changed || enabled_changed {
                let description = if !action_enabled {
                    "等待麦克风授权。当前未录音".to_string()
                } else if action_checked {
                    "停止调音。麦克风正在使用".to_string()
                } else {
                    format!("{}。当前未录音", snapshot.button)
                };
                set_content_description(env, self.views.action.as_obj(), &description)?;
            }
            for (view, text, previous_text) in [
                (&self.views.note, &snapshot.note, old.map(|old| &old.note)),
                (
                    &self.views.caption,
                    &snapshot.caption,
                    old.map(|old| &old.caption),
                ),
                (
                    &self.views.frequency,
                    &snapshot.frequency,
                    old.map(|old| &old.frequency),
                ),
                (
                    &self.views.cents,
                    &snapshot.cents,
                    old.map(|old| &old.cents),
                ),
                (
                    &self.views.status,
                    &snapshot.status,
                    old.map(|old| &old.status),
                ),
                (
                    &self.views.level,
                    &snapshot.level,
                    old.map(|old| &old.level),
                ),
            ] {
                if previous_text != Some(text) {
                    set_text(env, view.as_obj(), text)?;
                }
            }
            // The first usable reading can arrive before the gauge receives
            // its measured width. Retry after layout even if cents are stable.
            let needle_waiting_for_layout = snapshot.needle_cents.is_some()
                && env
                    .call_method(self.views.needle.as_obj(), "getVisibility", "()I", &[])?
                    .i()?
                    != 0;
            if needle_waiting_for_layout
                || old.is_none_or(|old| old.needle_cents != snapshot.needle_cents)
            {
                sync_needle(
                    env,
                    self.views.gauge.as_obj(),
                    self.views.needle.as_obj(),
                    snapshot.needle_cents,
                    old.is_some_and(|old| old.needle_cents.is_some())
                        && !needle_waiting_for_layout,
                )?;
            }
            if old.is_none_or(|old| old.note != snapshot.note || old.cents != snapshot.cents)
                || reading_changed
            {
                set_content_description(
                    env,
                    self.views.gauge.as_obj(),
                    &progress_accessibility_description(&snapshot),
                )?;
            }
            if reading_changed {
                env.call_method(self.views.gauge_labels[2].as_obj(), "setTextColor", "(I)V",
                    &[JValue::Int(if snapshot.in_tune { TUNED_GREEN } else { MUTED_TEXT })])?;
                env.call_method(self.views.needle.as_obj(), "setAlpha", "(F)V",
                    &[JValue::Float(if snapshot.phase == UiPhase::Fading { 0.38 } else { 1.0 })])?;
                let accent = if snapshot.in_tune {
                    TUNED_GREEN
                } else if snapshot.needle_cents.is_some_and(|v| v.abs() > 4.0) {
                    FLAT_SHARP
                } else if snapshot.needle_cents.is_some() {
                    BRASS_SOFT
                } else {
                    MUTED_TEXT
                };
                env.call_method(
                    self.views.cents.as_obj(),
                    "setTextColor",
                    "(I)V",
                    &[JValue::Int(accent)],
                )?;
                set_gradient_background(
                    env,
                    self.views.needle.as_obj(),
                    &[accent, BRASS],
                    BRASS_SOFT,
                    3.0 * density,
                    0,
                )?;
            }
            if old.is_none_or(|old| old.input_status != snapshot.input_status) {
                let (text, accent) = microphone_indicator(snapshot.input_status);
                set_text(env, self.views.microphone.as_obj(), text)?;
                env.call_method(
                    self.views.microphone.as_obj(),
                    "setTextColor",
                    "(I)V",
                    &[JValue::Int(accent)],
                )?;
            }
            if old.is_none_or(|old| old.selected_target.is_none() != snapshot.selected_target.is_none()) {
                set_toggle_text(env, self.views.auto.as_obj(), if snapshot.selected_target.is_none() { "自动识别 · 已选" } else { "自动识别" })?;
                env.call_method(
                    self.views.auto.as_obj(),
                    "setChecked",
                    "(Z)V",
                    &[JValue::Bool(jboolean::from(snapshot.selected_target.is_none()))],
                )?;
                set_tactile_background(
                    env,
                    self.views.auto.as_obj(),
                    if snapshot.selected_target.is_none() {
                        &[CHIP_ACTIVE, PANEL]
                    } else {
                        &[CHIP_IDLE, PANEL]
                    },
                    if snapshot.selected_target.is_none() { BRASS_SOFT } else { PANEL_STROKE },
                    15.0 * density,
                    density.round() as i32,
                )?;
            }
            for (index, chip) in self.views.chips.iter().enumerate() {
                let selected = snapshot.selected_target == Some(index);
                if old.is_none_or(|old| (old.selected_target == Some(index)) != selected) {
                    env.call_method(
                        chip.as_obj(),
                        "setChecked",
                        "(Z)V",
                        &[JValue::Bool(jboolean::from(selected))],
                    )?;
                }
                let tint = chip_tint(&snapshot, index);
                if old.is_none_or(|old| chip_tint(old, index) != tint) {
                    set_tactile_background(
                        env,
                        chip.as_obj(),
                        if tint == CHIP_TUNED {
                            &[CHIP_TUNED, PANEL]
                        } else if tint == CHIP_ACTIVE {
                            &[BRASS, CHIP_ACTIVE]
                        } else if tint == CHIP_DETECTED {
                            &[CHIP_DETECTED, PANEL]
                        } else {
                            &[CHIP_IDLE, PANEL]
                        },
                        if tint == CHIP_TUNED { TUNED_GREEN } else if tint == CHIP_ACTIVE || tint == CHIP_DETECTED {
                            BRASS_SOFT
                        } else {
                            PANEL_STROKE
                        },
                        13.0 * density,
                        density.round() as i32,
                    )?;
                }
            }
            if old.is_none_or(|old| old.keep_screen_on != snapshot.keep_screen_on) {
                env.call_method(
                    self.views.root.as_obj(),
                    "setKeepScreenOn",
                    "(Z)V",
                    &[JValue::Bool(jboolean::from(snapshot.keep_screen_on))],
                )?;
            }
            if let Some(message) = announcement.as_deref() {
                announce_for_accessibility(env, self.views.status.as_obj(), message)?;
            }
            Ok(())
        });
        clear_exception(env);
        if sync_result.is_ok() {
            *last_synced = Some(SyncedViews {
                snapshot,
                action_checked,
                action_enabled,
            });
        }
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.control.running.store(false, Ordering::SeqCst);
    }
}

impl Drop for InputHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

fn microphone_indicator(status: InputStatus) -> (&'static str, i32) {
    match status {
        InputStatus::Idle => ("○ 麦克风待机", MUTED_TEXT),
        InputStatus::Starting => ("◌ 正在连接", BRASS_SOFT),
        InputStatus::Receiving => ("● 已收到声音", BRASS_SOFT),
        InputStatus::DigitalSilence => ("○ 已连接 · 无波形", MUTED_TEXT),
        InputStatus::Stalled => ("○ 输入暂未更新", MUTED_TEXT),
        InputStatus::SystemSilenced => ("! 系统静音", BRASS_SOFT),
    }
}

/// An indicator, not an editable seek bar. A missing reading is invisible,
/// never presented as a centered (and apparently tuned) needle.
fn sync_needle(
    env: &mut JNIEnv,
    gauge: &JObject,
    needle: &JObject,
    cents: Option<f32>,
    _animate: bool,
) -> JniResult<()> {
    let Some(cents) = cents else {
        let animator = env.call_method(
            needle,
            "animate",
            "()Landroid/view/ViewPropertyAnimator;",
            &[],
        )?.l()?;
        env.call_method(&animator, "cancel", "()V", &[])?;
        env.call_method(needle, "setRotation", "(F)V", &[JValue::Float(0.0)])?;
        env.call_method(needle, "setVisibility", "(I)V", &[JValue::Int(4)])?;
        return Ok(());
    };
    // A warm analogue dial rotates around a fixed, measured pivot. The arc
    // occupies ±75°; keep the pointer just inside its endpoints.
    let width = env.call_method(gauge, "getWidth", "()I", &[])?.i()?;
    let needle_height = env.call_method(needle, "getHeight", "()I", &[])?.i()?;
    if width <= 0 || needle_height <= 0 {
        return Ok(());
    }
    let angle = dial::angle(cents);
    // Tracking already smooths accepted measurements. Apply the exact angle
    // of the same snapshot used by the text rather than restarting animations.
    env.call_method(needle, "setRotation", "(F)V", &[JValue::Float(angle)])?;
    env.call_method(needle, "setVisibility", "(I)V", &[JValue::Int(0)])?;
    Ok(())
}

fn chip_tint(snapshot: &UiSnapshot, index: usize) -> i32 {
    let active = snapshot.active_target == Some(index);
    if active && snapshot.in_tune {
        CHIP_TUNED
    } else if snapshot.selected_target == Some(index) {
        CHIP_ACTIVE
    } else if active && snapshot.needle_cents.is_some() && snapshot.phase == UiPhase::Reading {
        CHIP_DETECTED
    } else {
        CHIP_IDLE
    }
}

unsafe extern "C" fn main_loop_tick(_frame_time_nanos: c_long, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let context = &*(data as *mut MainLoopContext);
    let Some(app) = context.app.upgrade() else {
        drop(Box::from_raw(data as *mut MainLoopContext));
        return;
    };
    if !app.running.load(Ordering::SeqCst) {
        app.main_loop_active.store(false, Ordering::SeqCst);
        drop(app);
        drop(Box::from_raw(data as *mut MainLoopContext));
        return;
    }
    if !app.resumed.load(Ordering::SeqCst) {
        app.main_loop_active.store(false, Ordering::SeqCst);
        if app.resumed.load(Ordering::SeqCst) {
            AppState::start_main_loop(&app);
        }
        drop(app);
        drop(Box::from_raw(data as *mut MainLoopContext));
        return;
    }

    if let Ok(mut env) = app.vm.attach_current_thread() {
        AppState::main_tick(&app, &mut env);
    }
    if !app.running.load(Ordering::SeqCst) || !app.resumed.load(Ordering::SeqCst) {
        app.main_loop_active.store(false, Ordering::SeqCst);
        if app.running.load(Ordering::SeqCst) && app.resumed.load(Ordering::SeqCst) {
            AppState::start_main_loop(&app);
        }
        drop(app);
        drop(Box::from_raw(data as *mut MainLoopContext));
        return;
    }
    AChoreographer_postFrameCallbackDelayed(
        context.choreographer,
        main_loop_tick,
        data,
        UI_POLL_MILLIS,
    );
}

fn report_microphone_error(app: &AppState, control: &AudioControl) {
    app.set_microphone_error(control);
}

fn capture_active(app: &AppState, control: &AudioControl) -> bool {
    control.running.load(Ordering::SeqCst)
        && app.running.load(Ordering::SeqCst)
        && app.resumed.load(Ordering::SeqCst)
        && app.listening_requested.load(Ordering::SeqCst)
}

fn audio_loop(app: Arc<AppState>, control: Arc<AudioControl>) {
    let Ok(mut env) = app.vm.attach_current_thread() else {
        startup_log(6, "audio could not attach the capture worker to the JVM");
        report_microphone_error(&app, &control);
        return;
    };
    let outcome = run_capture(
        || capture_active(&app, &control),
        |configuration| capture_audio_attempt(&mut env, &app, &control, configuration),
    );
    if outcome == CaptureOutcome::Unavailable {
        startup_log(6, "audio exhausted all source/sample-rate configurations");
        report_microphone_error(&app, &control);
    }
}

fn capture_audio_attempt(
    env: &mut JNIEnv,
    app: &AppState,
    control: &AudioControl,
    configuration: CaptureConfig,
) -> CaptureAttempt {
    // Keep all JNI locals for one attempt in a bounded frame. A failed recorder
    // is stopped and released before run_capture may try the next source.
    let result = env.with_local_frame(32, |env| -> JniResult<CaptureAttempt> {
        app.show_capture_status(control, false);
        let (record, sample_rate) = match prepare_audio_record(env, app, control, configuration) {
            Ok(prepared) => prepared,
            Err(error) => {
                log_audio_error(env, configuration, "prepare", &error);
                return Ok(CaptureAttempt::Retry);
            }
        };
        let outcome =
            match read_audio_record(env, app, control, &record, sample_rate, configuration) {
                Ok(outcome) => outcome,
                Err(error) => {
                    log_audio_error(env, configuration, "read", &error);
                    CaptureAttempt::Retry
                }
            };
        release_audio_record(env, &record);
        Ok(outcome)
    });
    match result {
        Ok(outcome) => outcome,
        Err(error) => {
            log_audio_error(env, configuration, "local-frame", &error);
            CaptureAttempt::Retry
        }
    }
}

fn read_audio_record(
    env: &mut JNIEnv,
    app: &AppState,
    control: &AudioControl,
    record: &JObject,
    sample_rate: i32,
    configuration: CaptureConfig,
) -> JniResult<CaptureAttempt> {
    if !capture_active(app, control) {
        return Ok(CaptureAttempt::Stopped);
    }
    let buffer = env.new_short_array(FRAME_SIZE as i32)?;
    let mut samples = vec![0_i16; FRAME_SIZE];
    let mut filled = 0;
    let mut target_epoch = app.ui.lock().unwrap().target_epoch;
    let mut skip_window = false;
    let mut analyzer = PitchAnalyzer::default();
    let started_at = Instant::now();
    let mut watchdog = FrameWatchdog::new(Duration::ZERO);
    let mut silence_watchdog = DigitalSilenceWatchdog::new();
    let sdk = android_sdk_version(env);
    let mut mute_checked_at = None;
    let mut system_silenced = false;
    let mut mute_observation = None;
    let mut frames = 0_u64;
    let mut diagnostic_at = Duration::ZERO;

    while capture_active(app, control) {
        let now = started_at.elapsed();
        let current_epoch = app.ui.lock().unwrap().target_epoch;
        if current_epoch != target_epoch {
            // No analysis-window PCM from the former selection enters the new target.
            filled = 0;
            skip_window = true; // discard queued recorder PCM from the old selection
            target_epoch = current_epoch;
        }
        if mute_checked_at.is_none_or(|last| now.saturating_sub(last) >= Duration::from_millis(500))
        {
            mute_observation = capture_is_system_silenced(env, app, record, sdk);
            let was_silenced = system_silenced;
            if let Some(muted) = mute_observation {
                system_silenced = muted;
            }
            mute_checked_at = Some(now);
            if was_silenced != system_silenced {
                filled = 0;
                skip_window = !system_silenced;
                silence_watchdog.observe_frame(now, &[], true, false);
                // A previously proven source remains proven across system mute.
            }
            if system_silenced {
                app.show_capture_status(control, true);
            } else if was_silenced {
                // Give the resumed input its own complete-frame deadline.
                watchdog = FrameWatchdog::new(now);
                app.show_capture_status(control, false);
            }
        }
        if !system_silenced && watchdog.timed_out(now) {
            startup_log(
                5,
                "audio timed out waiting for a complete PCM frame; trying fallback",
            );
            return Ok(CaptureAttempt::Retry);
        }
        let remaining = FRAME_SIZE - filled;
        let read = env
            .call_method(
                record,
                "read",
                "([SIII)I",
                &[
                    JValue::Object(buffer.as_ref()),
                    JValue::Int(0),
                    JValue::Int(remaining as i32),
                    JValue::Int(AUDIO_READ_NON_BLOCKING),
                ],
            )?
            .i()?;

        if read > 0 && read as usize <= remaining {
            let read_len = read as usize;
            env.get_short_array_region(&buffer, 0, &mut samples[filled..filled + read_len])?;
            filled += read_len;
            if filled == FRAME_SIZE {
                if skip_window {
                    skip_window = false;
                    filled = 0;
                    watchdog.frame_received(started_at.elapsed());
                    continue;
                }
                let frame = InputFrame::from_pcm(&samples).expect("complete PCM frame");
                let signal = silence_watchdog.observe_frame(
                    now,
                    &samples,
                    system_silenced,
                    configuration.can_retry_digital_silence(),
                );
                if signal == CaptureSignalStatus::ZeroSignalTimeout {
                    startup_log(
                        5,
                        "audio startup delivered only constant PCM; trying fallback",
                    );
                    return Ok(CaptureAttempt::NoWaveform);
                }
                let reading = if system_silenced {
                    None
                } else {
                    analyzer.analyze_pcm(&samples, sample_rate as f32)
                };
                app.apply_input_frame(control, frame, reading, system_silenced, target_epoch);
                frames = frames.saturating_add(1);
                if frames == 1 || now.saturating_sub(diagnostic_at) >= Duration::from_secs(5) {
                    // Aggregate metadata only; PCM is never written or logged.
                    startup_log(4, &format!(
                        "audio input frames={frames} rms={:.6} peak={:.6} constant={} mute={mute_observation:?} pitch={}",
                        frame.rms, frame.peak, frame.digital_silence, reading.is_some()
                    ));
                    diagnostic_at = now;
                }
                samples.copy_within(ANALYSIS_HOP..FRAME_SIZE, 0);
                filled = FRAME_SIZE - ANALYSIS_HOP;
                // Complete frames and waveform delivery are distinct. Only
                // digitally constant PCM triggers signal fallback; a quiet
                // varying waveform or missing pitch never changes sources.
                watchdog.frame_received(started_at.elapsed());
            }
        } else if read == 0 {
            thread::sleep(AUDIO_IDLE_POLL);
        } else {
            startup_log(5, &format!("audio read returned {read}; trying fallback"));
            return Ok(CaptureAttempt::Retry);
        }
    }
    Ok(CaptureAttempt::Stopped)
}

fn input_loop(app: Arc<AppState>, running: Arc<AtomicBool>, queue_addr: usize) {
    let queue = queue_addr as *mut AInputQueue;
    let mut gesture = TouchGesture::default();
    unsafe {
        let looper = ALooper_prepare(ALOOPER_PREPARE_ALLOW_NON_CALLBACKS);
        if looper.is_null() {
            return;
        }
        AInputQueue_attachLooper(queue, looper, INPUT_QUEUE_IDENT, None, ptr::null_mut());
        while running.load(Ordering::SeqCst)
            && app.running.load(Ordering::SeqCst)
            && app.resumed.load(Ordering::SeqCst)
        {
            let ident = ALooper_pollOnce(16, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            if ident == INPUT_QUEUE_IDENT {
                drain_input_queue(&app, queue, &mut gesture);
            }
        }
        AInputQueue_detachLooper(queue);
    }
}

unsafe fn drain_input_queue(
    app: &Arc<AppState>,
    queue: *mut AInputQueue,
    gesture: &mut TouchGesture,
) {
    loop {
        let mut event: *mut AInputEvent = ptr::null_mut();
        if AInputQueue_getEvent(queue, &mut event) < 0 || event.is_null() {
            break;
        }
        if AInputQueue_preDispatchEvent(queue, event) != 0 {
            continue;
        }

        let mut handled = 0;
        if AInputEvent_getType(event) == AINPUT_EVENT_TYPE_MOTION {
            handled = 1;
            let action = AMotionEvent_getAction(event) & AMOTION_EVENT_ACTION_MASK;
            if !matches!(action, AMOTION_EVENT_ACTION_DOWN | AMOTION_EVENT_ACTION_UP | AMOTION_EVENT_ACTION_MOVE | AMOTION_EVENT_ACTION_CANCEL | AMOTION_EVENT_ACTION_POINTER_DOWN | 6) {
                AInputQueue_finishEvent(queue, event, 0); // hover/scroll accessibility dispatch
                continue;
            }
            if AMotionEvent_getPointerCount(event) != 1
                || action == AMOTION_EVENT_ACTION_POINTER_DOWN
            {
                gesture.active = false;
                app.pending_inputs.lock().unwrap().push_back(PendingInput::ClearPress);
                AInputQueue_finishEvent(queue, event, handled);
                continue;
            }
            let x = AMotionEvent_getX(event, 0);
            let y = AMotionEvent_getY(event, 0);
            match action {
                AMOTION_EVENT_ACTION_DOWN => {
                    gesture.active = true;
                    gesture.down_x = x;
                    gesture.down_y = y;
                    gesture.last_y = y;
                    gesture.moved = false;
                    app.pending_inputs.lock().unwrap().push_back(PendingInput::Press { x, y });
                }
                AMOTION_EVENT_ACTION_MOVE if gesture.active => {
                    let total_x = (x - gesture.down_x).abs();
                    let total_y = (y - gesture.down_y).abs();
                    let was_moved = gesture.moved;
                    if total_x > app.views.touch_slop_px || total_y > app.views.touch_slop_px {
                        gesture.moved = true;
                    }
                    let delta = if was_moved {
                        gesture.last_y - y
                    } else {
                        gesture.down_y - y
                    };
                    gesture.last_y = y;
                    if gesture.moved {
                        app.pending_inputs.lock().unwrap().push_back(PendingInput::Scroll {
                            down_x: gesture.down_x, down_y: gesture.down_y, delta,
                        });
                    }
                }
                AMOTION_EVENT_ACTION_UP if gesture.active => {
                    let total_x = (x - gesture.down_x).abs();
                    let total_y = (y - gesture.down_y).abs();
                    if !gesture.moved
                        && total_x <= app.views.touch_slop_px
                        && total_y <= app.views.touch_slop_px
                    {
                        app.pending_inputs.lock().unwrap().push_back(PendingInput::Tap {
                            down_x: gesture.down_x, down_y: gesture.down_y, x, y,
                        });
                    } else {
                        app.pending_inputs.lock().unwrap().push_back(PendingInput::Scroll {
                            down_x: gesture.down_x, down_y: gesture.down_y, delta: gesture.last_y - y,
                        });
                    }
                    gesture.active = false;
                }
                AMOTION_EVENT_ACTION_CANCEL => {
                    gesture.active = false;
                    app.pending_inputs.lock().unwrap().push_back(PendingInput::ClearPress);
                }
                _ => {}
            }
        }
        AInputQueue_finishEvent(queue, event, handled);
    }
}

fn prepare_audio_record<'local>(
    env: &mut JNIEnv<'local>,
    app: &AppState,
    control: &AudioControl,
    configuration: CaptureConfig,
) -> JniResult<(JObject<'local>, i32)> {
    let channel_in_mono = 16;
    let encoding_pcm_16bit = 2;
    let min_buffer = env
        .call_static_method(
            "android/media/AudioRecord",
            "getMinBufferSize",
            "(III)I",
            &[
                JValue::Int(configuration.sample_rate),
                JValue::Int(channel_in_mono),
                JValue::Int(encoding_pcm_16bit),
            ],
        )?
        .i()?;
    if min_buffer <= 0 {
        return Err(jni::errors::Error::NullPtr(
            "unsupported AudioRecord format",
        ));
    }
    let buffer_size = min_buffer
        .saturating_mul(2)
        .max(configuration.sample_rate / 5);
    let record = build_audio_record(env, configuration, buffer_size)?;

    let started = (|| -> JniResult<i32> {
        if env.call_method(&record, "getState", "()I", &[])?.i()? != 1 {
            return Err(jni::errors::Error::NullPtr("AudioRecord not initialized"));
        }
        if !capture_active(app, control) {
            return Err(jni::errors::Error::NullPtr("capture stopped before start"));
        }
        env.call_method(&record, "startRecording", "()V", &[])?;
        if env
            .call_method(&record, "getRecordingState", "()I", &[])?
            .i()?
            != 3
        {
            return Err(jni::errors::Error::NullPtr("AudioRecord did not start"));
        }
        let sample_rate = env.call_method(&record, "getSampleRate", "()I", &[])?.i()?;
        if !(16_000..=96_000).contains(&sample_rate) {
            return Err(jni::errors::Error::NullPtr(
                "unsupported actual sample rate",
            ));
        }
        Ok(sample_rate)
    })();
    match started {
        Ok(sample_rate) => {
            startup_log(
                4,
                &format!(
                    "audio recording source={} requested_rate={} actual_rate={sample_rate}",
                    configuration.source, configuration.sample_rate
                ),
            );
            Ok((record, sample_rate))
        }
        Err(error) => {
            // Preserve diagnostics before clearing the Java exception for
            // cleanup. Initialization alone must never commit us to a source.
            log_audio_error(env, configuration, "start", &error);
            release_audio_record(env, &record);
            Err(error)
        }
    }
}

fn android_sdk_version(env: &mut JNIEnv) -> i32 {
    let sdk = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|value| value.i())
        .unwrap_or(0);
    clear_exception(env);
    sdk
}

fn build_audio_record<'local>(
    env: &mut JNIEnv<'local>,
    configuration: CaptureConfig,
    buffer_size: i32,
) -> JniResult<JObject<'local>> {
    let format_builder = env.new_object("android/media/AudioFormat$Builder", "()V", &[])?;
    for (method, value) in [
        ("setSampleRate", configuration.sample_rate),
        ("setChannelMask", 16),
        ("setEncoding", 2),
    ] {
        env.call_method(
            &format_builder,
            method,
            "(I)Landroid/media/AudioFormat$Builder;",
            &[JValue::Int(value)],
        )?;
    }
    let format = env
        .call_method(
            &format_builder,
            "build",
            "()Landroid/media/AudioFormat;",
            &[],
        )?
        .l()?;
    let builder = env.new_object("android/media/AudioRecord$Builder", "()V", &[])?;
    env.call_method(
        &builder,
        "setAudioFormat",
        "(Landroid/media/AudioFormat;)Landroid/media/AudioRecord$Builder;",
        &[JValue::Object(&format)],
    )?;
    for (method, value) in [
        ("setAudioSource", configuration.source),
        ("setBufferSizeInBytes", buffer_size),
    ] {
        env.call_method(
            &builder,
            method,
            "(I)Landroid/media/AudioRecord$Builder;",
            &[JValue::Int(value)],
        )?;
    }
    if android_sdk_version(env) >= 30 {
        env.call_method(
            &builder,
            "setPrivacySensitive",
            "(Z)Landroid/media/AudioRecord$Builder;",
            &[JValue::Bool(1)],
        )?;
    }
    env.call_method(&builder, "build", "()Landroid/media/AudioRecord;", &[])?
        .l()
}

/// Null configurations and failed queries are unknown, not proof of unmuted
/// input. The adapter only reports system silencing when a public API says so.
fn capture_is_system_silenced(
    env: &mut JNIEnv,
    app: &AppState,
    record: &JObject,
    sdk: i32,
) -> Option<bool> {
    let result = env.with_local_frame(12, |env| -> JniResult<Option<bool>> {
        let service_name = env.new_string("audio")?;
        let service = env
            .call_method(
                app.activity.as_obj(),
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(service_name.as_ref())],
            )?
            .l()?;
        let global = if service.is_null() {
            None
        } else {
            Some(
                env.call_method(&service, "isMicrophoneMute", "()Z", &[])?
                    .z()?,
            )
        };
        if global == Some(true) {
            return Ok(Some(true));
        }
        if sdk >= 29 {
            let configuration = env
                .call_method(
                    record,
                    "getActiveRecordingConfiguration",
                    "()Landroid/media/AudioRecordingConfiguration;",
                    &[],
                )?
                .l()?;
            if !configuration.is_null() {
                return Ok(Some(
                    env.call_method(&configuration, "isClientSilenced", "()Z", &[])?
                        .z()?,
                ));
            }
        }
        Ok(if sdk < 29 { global } else { None })
    });
    clear_exception(env);
    result.unwrap_or(None)
}

fn log_audio_error(
    env: &mut JNIEnv,
    configuration: CaptureConfig,
    stage: &str,
    error: &jni::errors::Error,
) {
    startup_log(
        5,
        &format!(
            "audio {stage}: source={} requested_rate={}: {error}",
            configuration.source, configuration.sample_rate
        ),
    );
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
    }
    clear_exception(env);
}

fn release_audio_record(env: &mut JNIEnv, record: &JObject) {
    clear_exception(env);
    let recording_state = env
        .call_method(record, "getRecordingState", "()I", &[])
        .and_then(|value| value.i())
        .unwrap_or(0);
    clear_exception(env);
    if recording_state == 3 {
        let _ = env.call_method(record, "stop", "()V", &[]);
        clear_exception(env);
    }
    let _ = env.call_method(record, "release", "()V", &[]);
    clear_exception(env);
}

fn create_ui<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
) -> JniResult<OverlayViews> {
    let density = display_density(env, activity).max(1.0);
    let dp = |value: f32| (value * density).round() as i32;

    let shell = new_linear_layout(env, activity, VERTICAL)?;
    env.call_method(&shell, "setFitsSystemWindows", "(Z)V", &[JValue::Bool(1)])?;
    set_gradient_background(env, &shell, &[WOOD_TOP, PANEL, MIDNIGHT], PANEL_STROKE, 0.0, 0)?;

    let scroll = env.new_object(
        "android/widget/ScrollView",
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    env.call_method(&scroll, "setFillViewport", "(Z)V", &[JValue::Bool(1)])?;
    let root_description = env.new_string("弦准古典吉他调音器")?;
    let root_description_obj: JObject = root_description.into();
    env.call_method(
        &shell,
        "setContentDescription",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&root_description_obj)],
    )?;
    let body = new_linear_layout(env, activity, VERTICAL)?;
    let body_params = linear_params(env, MATCH_PARENT, 0, Some(1.0), [0; 4])?;
    add_view(env, &shell, &body, body_params)?;
    let hero = new_linear_layout(env, activity, VERTICAL)?;
    env.call_method(&hero, "setGravity", "(I)V", &[JValue::Int(GRAVITY_CENTER_HORIZONTAL)])?;
    add_with_height(env, &body, &hero, MATCH_PARENT, WRAP_CONTENT,
        [dp(12.0), dp(4.0), dp(12.0), dp(3.0)])?;
    let scroll_params = linear_params(env, MATCH_PARENT, 0, Some(1.0), [0; 4])?;
    add_view(env, &body, &scroll, scroll_params)?;

    let content = new_linear_layout(env, activity, VERTICAL)?;
    env.call_method(
        &content,
        "setGravity",
        "(I)V",
        &[JValue::Int(GRAVITY_CENTER_HORIZONTAL)],
    )?;
    env.call_method(
        &content,
        "setPadding",
        "(IIII)V",
        &[
            JValue::Int(dp(12.0)),
            JValue::Int(dp(3.0)),
            JValue::Int(dp(12.0)),
            JValue::Int(dp(8.0)),
        ],
    )?;
    let content_params = linear_params(env, MATCH_PARENT, WRAP_CONTENT, None, [0; 4])?;
    add_view(env, &scroll, &content, content_params)?;

    let header = new_linear_layout(env, activity, HORIZONTAL)?;
    env.call_method(&header, "setGravity", "(I)V", &[JValue::Int(GRAVITY_CENTER)])?;
    add_with_height(env, &hero, &header, MATCH_PARENT, WRAP_CONTENT, [0, 0, 0, dp(3.0)])?;
    let title = new_text_view(env, activity, "弦准", 19.0, BRASS_SOFT, true, GRAVITY_CENTER)?;
    set_serif_font(env, &title, true)?;
    let title_params = linear_params(env, 0, WRAP_CONTENT, Some(0.5), [0; 4])?;
    add_view(env, &header, &title, title_params)?;
    let microphone = new_text_view(
        env, activity, "○ 麦克风待机", 12.0, MUTED_TEXT, false, GRAVITY_CENTER,
    )?;
    env.call_method(&microphone, "setSingleLine", "(Z)V", &[JValue::Bool(1)])?;
    let mic_params = linear_params(env, 0, WRAP_CONTENT, Some(1.0), [0; 4])?;
    add_view(env, &header, &microphone, mic_params)?;

    let card = new_linear_layout(env, activity, VERTICAL)?;
    env.call_method(&card, "setGravity", "(I)V", &[JValue::Int(GRAVITY_CENTER)])?;
    env.call_method(
        &card, "setPadding", "(IIII)V",
        &[
            JValue::Int(dp(8.0)), JValue::Int(dp(4.0)),
            JValue::Int(dp(8.0)), JValue::Int(dp(4.0)),
        ],
    )?;
    set_gradient_background(
        env, &card, &[WOOD_TOP, PANEL, MIDNIGHT],
        PANEL_STROKE, dp(23.0) as f32, dp(1.0),
    )?;
    env.call_method(&card, "setElevation", "(F)V", &[JValue::Float(dp(4.0) as f32)])?;
    add_with_height(env, &hero, &card, MATCH_PARENT, WRAP_CONTENT, [0; 4])?;

    let note = new_text_view(env, activity, "待拨弦", 48.0, WARM_WHITE, true, GRAVITY_CENTER)?;
    set_serif_font(env, &note, true)?;
    env.call_method(&note, "setSingleLine", "(Z)V", &[JValue::Bool(1)])?;
    env.call_method(
        &note,
        "setAutoSizeTextTypeUniformWithConfiguration",
        "(IIII)V",
        &[JValue::Int(24), JValue::Int(48), JValue::Int(2), JValue::Int(TEXT_UNIT_SP)],
    )?;
    add_with_height(env, &card, &note, MATCH_PARENT, dp(52.0), [0; 4])?;

    let caption = new_text_view(
        env, activity, "自动识别", 15.0, BRASS_SOFT, false, GRAVITY_CENTER,
    )?;
    add_with_height(env, &card, &caption, MATCH_PARENT, WRAP_CONTENT, [0, 0, 0, dp(2.0)])?;

    let dial_width = responsive_dial_width(env, activity, density);
    let scale = dial_width as f32 / dial::WIDTH;
    let dial_height = (dial::HEIGHT * scale).round() as i32;
    let needle_height = (dial::NEEDLE_RADIUS * scale).round() as i32;
    let needle_bottom = ((dial::HEIGHT - dial::PIVOT_Y) * scale).round() as i32;

    let gauge = env.new_object(
        "android/widget/FrameLayout",
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    set_gradient_background(
        env, &gauge, &[PANEL, MIDNIGHT],
        PANEL_STROKE, dp(22.0) as f32, dp(1.0),
    )?;
    env.call_method(&gauge, "setClipChildren", "(Z)V", &[JValue::Bool(0)])?;
    let gauge_art = env.new_object(
        "android/widget/ImageView",
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    let vector_id = drawable_resource_id(env, activity, "tuner_arc_brass")?;
    if vector_id != 0 {
        env.call_method(
            &gauge_art, "setImageResource", "(I)V",
            &[JValue::Int(vector_id)],
        )?;
    }
    env.call_method(
        &gauge_art, "setImportantForAccessibility", "(I)V", &[JValue::Int(2)],
    )?;
    let art_params = env.new_object(
        "android/widget/FrameLayout$LayoutParams",
        "(III)V",
        &[
            JValue::Int(MATCH_PARENT), JValue::Int(MATCH_PARENT),
            JValue::Int(GRAVITY_CENTER),
        ],
    )?;
    add_view(env, &gauge, &gauge_art, art_params)?;

    let needle = env.new_object(
        "android/view/View",
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    let needle_width = dp(3.0);
    set_gradient_background(
        env, &needle, &[BRASS_SOFT, BRASS], BRASS_SOFT, dp(2.0) as f32, 0,
    )?;
    env.call_method(&needle, "setVisibility", "(I)V", &[JValue::Int(4)])?;
    env.call_method(
        &needle, "setImportantForAccessibility", "(I)V", &[JValue::Int(2)],
    )?;
    // The physical pivot is the needle's lower end (not its centre).
    env.call_method(
        &needle, "setPivotX", "(F)V", &[JValue::Float(needle_width as f32 / 2.0)],
    )?;
    env.call_method(
        &needle, "setPivotY", "(F)V", &[JValue::Float(needle_height as f32)],
    )?;
    let needle_params = env.new_object(
        "android/widget/FrameLayout$LayoutParams",
        "(III)V",
        &[
            JValue::Int(needle_width), JValue::Int(needle_height),
            JValue::Int(81), // BOTTOM | CENTER_HORIZONTAL
        ],
    )?;
    env.call_method(
        &needle_params, "setMargins", "(IIII)V",
        &[JValue::Int(0), JValue::Int(0), JValue::Int(0), JValue::Int(needle_bottom)],
    )?;
    add_view(env, &gauge, &needle, needle_params)?;

    let pivot = env.new_object(
        "android/view/View", "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    let pivot_diameter = dp(13.0);
    set_gradient_background(
        env, &pivot, &[BRASS_SOFT, CHIP_ACTIVE], BRASS_SOFT,
        (pivot_diameter as f32) / 2.0, dp(1.0),
    )?;
    env.call_method(
        &pivot, "setImportantForAccessibility", "(I)V", &[JValue::Int(2)],
    )?;
    let pivot_params = env.new_object(
        "android/widget/FrameLayout$LayoutParams", "(III)V",
        &[
            JValue::Int(pivot_diameter), JValue::Int(pivot_diameter), JValue::Int(81),
        ],
    )?;
    env.call_method(
        &pivot_params, "setMargins", "(IIII)V",
        &[
            JValue::Int(0), JValue::Int(0),
            JValue::Int(0), JValue::Int((needle_bottom - pivot_diameter / 2).max(0)),
        ],
    )?;
    add_view(env, &gauge, &pivot, pivot_params)?;
    add_with_height(env, &card, &gauge, dial_width, dial_height, [0, 0, 0, dp(2.0)])?;

    let mut label_refs = Vec::with_capacity(5);
    for (cents, label) in dial::LABEL_CENTS.into_iter().zip(["−50", "−25", "0", "+25", "+50"]) {
        let marker = new_text_view(env, activity, label, (dial_width as f32 / density / 29.0).clamp(7.0, 11.0), MUTED_TEXT, false, GRAVITY_CENTER)?;
        env.call_method(&marker, "setSingleLine", "(Z)V", &[JValue::Bool(1)])?;
        env.call_method(&marker, "setImportantForAccessibility", "(I)V", &[JValue::Int(2)])?;
        let params = dial_label_params(env, cents, dial_width)?;
        add_view(env, &gauge, &marker, params)?;
        label_refs.push(env.new_global_ref(&marker)?);
    }
    let gauge_labels: [GlobalRef; 5] = label_refs.try_into()
        .map_err(|_| jni::errors::Error::NullPtr("dial label array"))?;

    let cents = new_text_view(
        env, activity, "— 音分", 22.0, BRASS_SOFT, true, GRAVITY_CENTER,
    )?;
    env.call_method(&cents, "setSingleLine", "(Z)V", &[JValue::Bool(1)])?;
    env.call_method(
        &cents, "setAutoSizeTextTypeUniformWithConfiguration", "(IIII)V",
        &[JValue::Int(12), JValue::Int(22), JValue::Int(1), JValue::Int(TEXT_UNIT_SP)],
    )?;
    add_with_height(env, &card, &cents, MATCH_PARENT, dp(32.0), [0; 4])?;

    let status = new_text_view(
        env, activity, "点开始，再拨一根空弦", 15.0, WARM_WHITE, false, GRAVITY_CENTER,
    )?;
    env.call_method(&status, "setMinLines", "(I)V", &[JValue::Int(2)])?;
    env.call_method(&status, "setMaxLines", "(I)V", &[JValue::Int(2)])?;
    add_with_height(env, &card, &status, MATCH_PARENT, WRAP_CONTENT, [0; 4])?;

    let frequency = new_text_view(
        env, activity, "检测 — Hz  ·  目标 — Hz", 13.0, MUTED_TEXT, false, GRAVITY_CENTER,
    )?;
    set_gradient_background(
        env, &frequency, &[CHIP_IDLE, PANEL], PANEL_STROKE,
        dp(13.0) as f32, dp(1.0),
    )?;
    env.call_method(
        &frequency, "setPadding", "(IIII)V",
        &[
            JValue::Int(dp(8.0)), JValue::Int(dp(9.0)),
            JValue::Int(dp(8.0)), JValue::Int(dp(9.0)),
        ],
    )?;
    add_with_height(env, &content, &frequency, MATCH_PARENT, WRAP_CONTENT, [0, 0, 0, dp(3.0)])?;

    let level = new_text_view(
        env, activity, "等待声音", 12.0, MUTED_TEXT, false, GRAVITY_CENTER,
    )?;
    add_with_height(env, &content, &level, MATCH_PARENT, WRAP_CONTENT, [0, 0, 0, dp(5.0)])?;

    let select_title = new_text_view(
        env,
        activity,
        "目标琴弦",
        15.0,
        WARM_WHITE,
        true,
        GRAVITY_CENTER,
    )?;
    set_serif_font(env, &select_title, true)?;
    add_with_height(
        env,
        &content,
        &select_title,
        MATCH_PARENT,
        WRAP_CONTENT,
        [0, 0, 0, dp(2.0)],
    )?;
    let auto = new_toggle(env, activity, "自动识别", 15.0, WARM_WHITE, dp(48.0))?;
    set_tactile_background(
        env, &auto, &[CHIP_ACTIVE, PANEL], BRASS_SOFT, dp(16.0) as f32, dp(1.0),
    )?;
    set_content_description(env, &auto, "自动识别琴弦，不锁定特定琴弦")?;
    add_with_height(env, &content, &auto, MATCH_PARENT, WRAP_CONTENT, [0, 0, 0, dp(8.0)])?;

    let row_one = new_linear_layout(env, activity, HORIZONTAL)?;
    let row_two = new_linear_layout(env, activity, HORIZONTAL)?;
    add_with_height(
        env,
        &content,
        &row_one,
        MATCH_PARENT,
        WRAP_CONTENT,
        [0, 0, 0, dp(6.0)],
    )?;
    add_with_height(
        env,
        &content,
        &row_two,
        MATCH_PARENT,
        WRAP_CONTENT,
        [0, 0, 0, dp(12.0)],
    )?;

    let mut chip_refs = Vec::with_capacity(6);
    for (index, target) in STRINGS.iter().enumerate() {
        let label = format!("{}\n{}弦", target.note, STRINGS.len() - index);
        let chip = new_toggle(env, activity, &label, 15.0, WARM_WHITE, dp(48.0))?;
        env.call_method(&chip, "setMinWidth", "(I)V", &[JValue::Int(0)])?;
        env.call_method(&chip, "setMinimumWidth", "(I)V", &[JValue::Int(0)])?;
        env.call_method(
            &chip,
            "setPadding",
            "(IIII)V",
            &[
                JValue::Int(dp(4.0)),
                JValue::Int(dp(3.0)),
                JValue::Int(dp(4.0)),
                JValue::Int(dp(3.0)),
            ],
        )?;
        let description = env.new_string(format!(
            "{}，{}，目标 {:.2} 赫兹",
            target.description, target.note, target.frequency_hz
        ))?;
        let description_obj: JObject = description.into();
        env.call_method(
            &chip,
            "setContentDescription",
            "(Ljava/lang/CharSequence;)V",
            &[JValue::Object(&description_obj)],
        )?;
        set_tactile_background(
            env, &chip, &[CHIP_IDLE, PANEL], PANEL_STROKE, dp(13.0) as f32, dp(1.0),
        )?;
        let parent = if index < 3 { &row_one } else { &row_two };
        let chip_params = linear_params(env, 0, WRAP_CONTENT, Some(1.0), [dp(3.0), 0, dp(3.0), 0])?;
        add_view(env, parent, &chip, chip_params)?;
        chip_refs.push(env.new_global_ref(&chip)?);
    }

    let footer_text = format!("标准调弦 · v{}", env!("CARGO_PKG_VERSION"));
    let footer = new_text_view(
        env,
        activity,
        &footer_text,
        11.0,
        MUTED_TEXT,
        false,
        GRAVITY_CENTER,
    )?;
    add_with_height(env, &content, &footer, MATCH_PARENT, WRAP_CONTENT, [0; 4])?;

    // Keep the primary action reachable with one hand even when the content
    // scrolls on a smaller device or at a larger system font scale.
    let action = new_toggle(env, activity, "开始调音", 18.0, MIDNIGHT, dp(52.0))?;
    set_serif_font(env, &action, true)?;
    set_tactile_background(
        env, &action, &[BRASS_SOFT, BRASS], BRASS_SOFT, dp(20.0) as f32, dp(2.0),
    )?;
    env.call_method(
        &action, "setElevation", "(F)V", &[JValue::Float(dp(6.0) as f32)],
    )?;
    set_content_description(env, &action, "开始调音。当前未录音")?;
    add_with_height(
        env,
        &shell,
        &action,
        MATCH_PARENT,
        WRAP_CONTENT,
        [dp(12.0), dp(3.0), dp(12.0), dp(6.0)],
    )?;

    env.call_method(
        activity,
        "setContentView",
        "(Landroid/view/View;)V",
        &[JValue::Object(&shell)],
    )?;

    let chips: [GlobalRef; 6] = chip_refs
        .try_into()
        .map_err(|_| jni::errors::Error::NullPtr("chip array"))?;
    Ok(OverlayViews {
        root: env.new_global_ref(&shell)?,
        scroll: env.new_global_ref(&scroll)?,
        body: env.new_global_ref(&body)?,
        hero: env.new_global_ref(&hero)?,
        action: env.new_global_ref(&action)?,
        auto: env.new_global_ref(&auto)?,
        note: env.new_global_ref(&note)?,
        caption: env.new_global_ref(&caption)?,
        frequency: env.new_global_ref(&frequency)?,
        cents: env.new_global_ref(&cents)?,
        status: env.new_global_ref(&status)?,
        level: env.new_global_ref(&level)?,
        microphone: env.new_global_ref(&microphone)?,
        gauge: env.new_global_ref(&gauge)?,
        needle: env.new_global_ref(&needle)?,
        pivot: env.new_global_ref(&pivot)?,
        gauge_labels,
        chips,
        touch_slop_px: scaled_touch_slop(env, activity).unwrap_or(dp(8.0) as f32),
    })
}

fn new_linear_layout<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
    orientation: i32,
) -> JniResult<JObject<'local>> {
    let layout = env.new_object(
        "android/widget/LinearLayout",
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    env.call_method(
        &layout,
        "setOrientation",
        "(I)V",
        &[JValue::Int(orientation)],
    )?;
    Ok(layout)
}

fn new_text_view<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
    text: &str,
    size_sp: f32,
    text_color: i32,
    bold: bool,
    gravity: i32,
) -> JniResult<JObject<'local>> {
    let view = env.new_object(
        "android/widget/TextView",
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    set_text(env, &view, text)?;
    env.call_method(&view, "setTextColor", "(I)V", &[JValue::Int(text_color)])?;
    env.call_method(
        &view,
        "setTextSize",
        "(IF)V",
        &[JValue::Int(TEXT_UNIT_SP), JValue::Float(size_sp)],
    )?;
    env.call_method(&view, "setGravity", "(I)V", &[JValue::Int(gravity)])?;
    env.call_method(&view, "setIncludeFontPadding", "(Z)V", &[JValue::Bool(0)])?;
    if bold {
        env.call_method(
            &view,
            "setTypeface",
            "(Landroid/graphics/Typeface;I)V",
            &[JValue::Object(&JObject::null()), JValue::Int(1)],
        )?;
    }
    Ok(view)
}

fn new_toggle<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
    text: &str,
    size_sp: f32,
    text_color: i32,
    min_height: i32,
) -> JniResult<JObject<'local>> {
    let view = env.new_object(
        "android/widget/ToggleButton",
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;
    set_toggle_text(env, &view, text)?;
    env.call_method(&view, "setTextColor", "(I)V", &[JValue::Int(text_color)])?;
    env.call_method(
        &view,
        "setTextSize",
        "(IF)V",
        &[JValue::Int(TEXT_UNIT_SP), JValue::Float(size_sp)],
    )?;
    env.call_method(&view, "setGravity", "(I)V", &[JValue::Int(GRAVITY_CENTER)])?;
    env.call_method(&view, "setAllCaps", "(Z)V", &[JValue::Bool(0)])?;
    env.call_method(&view, "setMinHeight", "(I)V", &[JValue::Int(min_height)])?;
    Ok(view)
}

fn linear_params<'local>(
    env: &mut JNIEnv<'local>,
    width: i32,
    height: i32,
    weight: Option<f32>,
    margins: [i32; 4],
) -> JniResult<JObject<'local>> {
    let params = if let Some(weight) = weight {
        env.new_object(
            "android/widget/LinearLayout$LayoutParams",
            "(IIF)V",
            &[
                JValue::Int(width),
                JValue::Int(height),
                JValue::Float(weight),
            ],
        )?
    } else {
        env.new_object(
            "android/widget/LinearLayout$LayoutParams",
            "(II)V",
            &[JValue::Int(width), JValue::Int(height)],
        )?
    };
    env.call_method(
        &params,
        "setMargins",
        "(IIII)V",
        &[
            JValue::Int(margins[0]),
            JValue::Int(margins[1]),
            JValue::Int(margins[2]),
            JValue::Int(margins[3]),
        ],
    )?;
    Ok(params)
}

fn add_with_height(
    env: &mut JNIEnv,
    parent: &JObject,
    child: &JObject,
    width: i32,
    height: i32,
    margins: [i32; 4],
) -> JniResult<()> {
    let params = linear_params(env, width, height, None, margins)?;
    add_view(env, parent, child, params)
}

fn add_view(env: &mut JNIEnv, parent: &JObject, child: &JObject, params: JObject) -> JniResult<()> {
    env.call_method(
        parent,
        "addView",
        "(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V",
        &[JValue::Object(child), JValue::Object(&params)],
    )?;
    Ok(())
}

fn set_text(env: &mut JNIEnv, view: &JObject, text: &str) -> JniResult<()> {
    let value = env.new_string(text)?;
    let value_obj: JObject = value.into();
    env.call_method(
        view,
        "setText",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&value_obj)],
    )?;
    Ok(())
}

fn set_content_description(env: &mut JNIEnv, view: &JObject, text: &str) -> JniResult<()> {
    let value = env.new_string(text)?;
    let value_obj: JObject = value.into();
    env.call_method(
        view,
        "setContentDescription",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&value_obj)],
    )?;
    Ok(())
}

fn announce_for_accessibility(env: &mut JNIEnv, view: &JObject, message: &str) -> JniResult<()> {
    let value = env.new_string(message)?;
    let value_obj: JObject = value.into();
    env.call_method(
        view,
        "announceForAccessibility",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&value_obj)],
    )?;
    Ok(())
}

fn set_toggle_text(env: &mut JNIEnv, view: &JObject, text: &str) -> JniResult<()> {
    let value = env.new_string(text)?;
    let value_obj: JObject = value.into();
    for method in ["setText", "setTextOn", "setTextOff"] {
        env.call_method(
            view,
            method,
            "(Ljava/lang/CharSequence;)V",
            &[JValue::Object(&value_obj)],
        )?;
    }
    Ok(())
}

fn scaled_touch_slop(env: &mut JNIEnv, activity: &JObject) -> Option<f32> {
    let result = (|| -> JniResult<f32> {
        let configuration = env
            .call_static_method(
                "android/view/ViewConfiguration",
                "get",
                "(Landroid/content/Context;)Landroid/view/ViewConfiguration;",
                &[JValue::Object(activity)],
            )?
            .l()?;
        Ok(env
            .call_method(&configuration, "getScaledTouchSlop", "()I", &[])?
            .i()? as f32)
    })();
    clear_exception(env);
    result.ok().filter(|slop| *slop > 0.0)
}

// A GradientDrawable is a native Android drawable. A pressed-state pair
// preserves tactile button feedback without raster assets or extra Java code.
fn make_gradient_drawable<'local>(
    env: &mut JNIEnv<'local>,
    colors: &[i32],
    stroke: i32,
    radius: f32,
    stroke_width: i32,
) -> JniResult<JObject<'local>> {
    let drawable = env.new_object("android/graphics/drawable/GradientDrawable", "()V", &[])?;
    let array = env.new_int_array(colors.len() as i32)?;
    env.set_int_array_region(&array, 0, colors)?;
    env.call_method(
        &drawable,
        "setColors",
        "([I)V",
        &[JValue::Object(array.as_ref())],
    )?;
    env.call_method(&drawable, "setCornerRadius", "(F)V", &[JValue::Float(radius)])?;
    env.call_method(
        &drawable,
        "setStroke",
        "(II)V",
        &[JValue::Int(stroke_width), JValue::Int(stroke)],
    )?;
    Ok(drawable)
}

fn set_gradient_background(
    env: &mut JNIEnv,
    view: &JObject,
    colors: &[i32],
    stroke: i32,
    radius: f32,
    stroke_width: i32,
) -> JniResult<()> {
    let drawable = make_gradient_drawable(env, colors, stroke, radius, stroke_width)?;
    env.call_method(
        view,
        "setBackground",
        "(Landroid/graphics/drawable/Drawable;)V",
        &[JValue::Object(&drawable)],
    )?;
    Ok(())
}

fn set_tactile_background(
    env: &mut JNIEnv,
    view: &JObject,
    colors: &[i32],
    stroke: i32,
    radius_dp: f32,
    stroke_width: i32,
) -> JniResult<()> {
    let states = env.new_object("android/graphics/drawable/StateListDrawable", "()V", &[])?;
    let pressed = env.new_int_array(1)?;
    env.set_int_array_region(&pressed, 0, &[16842919])?; // state_pressed
    let empty = env.new_int_array(0)?;
    let normal = make_gradient_drawable(env, colors, stroke, radius_dp, stroke_width)?;
    let pressed_drawable = make_gradient_drawable(
        env,
        &[BRASS_SOFT, CHIP_ACTIVE],
        BRASS_SOFT,
        radius_dp,
        stroke_width,
    )?;
    env.call_method(
        &states,
        "addState",
        "([ILandroid/graphics/drawable/Drawable;)V",
        &[JValue::Object(pressed.as_ref()), JValue::Object(&pressed_drawable)],
    )?;
    env.call_method(
        &states,
        "addState",
        "([ILandroid/graphics/drawable/Drawable;)V",
        &[JValue::Object(empty.as_ref()), JValue::Object(&normal)],
    )?;
    env.call_method(
        view,
        "setBackground",
        "(Landroid/graphics/drawable/Drawable;)V",
        &[JValue::Object(&states)],
    )?;
    Ok(())
}

fn display_size(env: &mut JNIEnv, activity: &JObject, density: f32) -> (i32, i32) {
    let metrics = (|| -> JniResult<(i32, i32)> {
        let resources = env.call_method(activity, "getResources", "()Landroid/content/res/Resources;", &[])?.l()?;
        let metrics = env.call_method(&resources, "getDisplayMetrics", "()Landroid/util/DisplayMetrics;", &[])?.l()?;
        Ok((env.get_field(&metrics, "widthPixels", "I")?.i()?,
            env.get_field(&metrics, "heightPixels", "I")?.i()?))
    })();
    clear_exception(env);
    metrics.unwrap_or(((360.0*density) as i32, (700.0*density) as i32))
}

fn wide_body(width: i32, height: i32, density: f32) -> bool {
    width > height && width as f32 / density >= 520.0
}

fn responsive_dial_width(env: &mut JNIEnv, activity: &JObject, density: f32) -> i32 {
    let (width, height) = display_size(env, activity, density);
    let side = (40.0*density).round() as i32;
    let available_width = if wide_body(width, height, density) { width / 2 } else { width };
    (available_width - side).min((height as f32 * 0.46).round() as i32)
        .min((320.0*density) as i32).max(1)
}

fn dial_label_params<'local>(env: &mut JNIEnv<'local>, cents: f32, width: i32) -> JniResult<JObject<'local>> {
    let scale = width as f32 / dial::WIDTH;
    let label_width = (48.0 * scale).round() as i32;
    let label_height = (28.0 * scale).round() as i32;
    let (x, y) = dial::label_center(cents);
    let params = env.new_object("android/widget/FrameLayout$LayoutParams", "(III)V",
        &[JValue::Int(label_width), JValue::Int(label_height), JValue::Int(51)])?;
    env.call_method(&params, "setMargins", "(IIII)V", &[
        JValue::Int((x * scale).round() as i32 - label_width / 2),
        JValue::Int((y * scale).round() as i32 - label_height / 2),
        JValue::Int(0), JValue::Int(0),
    ])?;
    Ok(params)
}

fn resize_dial(env: &mut JNIEnv, app: &AppState) -> JniResult<()> {
    let density = display_density(env, app.activity.as_obj()).max(1.0);
    let (screen_w, screen_h) = display_size(env, app.activity.as_obj(), density);
    let wide = wide_body(screen_w, screen_h, density);
    let dp = |v: f32| (v * density).round() as i32;
    env.call_method(app.views.body.as_obj(), "setOrientation", "(I)V",
        &[JValue::Int(if wide { HORIZONTAL } else { VERTICAL })])?;
    let hero_params = linear_params(env, if wide { 0 } else { MATCH_PARENT }, WRAP_CONTENT,
        if wide { Some(1.0) } else { None }, [dp(12.0), dp(4.0), dp(12.0), dp(3.0)])?;
    let scroll_params = linear_params(env, if wide { 0 } else { MATCH_PARENT },
        if wide { MATCH_PARENT } else { 0 }, Some(1.0), [0; 4])?;
    for (view, params) in [(&app.views.hero, hero_params), (&app.views.scroll, scroll_params)] {
        env.call_method(view.as_obj(), "setLayoutParams", "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&params)])?;
    }
    for (view, h) in [(&app.views.note, if wide { 38.0 } else { 52.0 }),
        (&app.views.cents, if wide { 26.0 } else { 32.0 })] {
        let params = env.call_method(view.as_obj(), "getLayoutParams",
            "()Landroid/view/ViewGroup$LayoutParams;", &[])?.l()?;
        env.set_field(&params, "height", "I", JValue::Int(dp(h)))?;
        env.call_method(view.as_obj(), "setLayoutParams", "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&params)])?;
    }
    let width = responsive_dial_width(env, app.activity.as_obj(), density);
    let scale = width as f32 / dial::WIDTH;
    let h = (dial::HEIGHT * scale).round() as i32;
    let needle_h = (dial::NEEDLE_RADIUS * scale).round() as i32;
    let bottom = ((dial::HEIGHT - dial::PIVOT_Y) * scale).round() as i32;
    let diameter = (13.0*density).round() as i32;
    let needle_w = (3.0*density).round() as i32;
    let params = env.call_method(app.views.gauge.as_obj(), "getLayoutParams",
        "()Landroid/view/ViewGroup$LayoutParams;", &[])?.l()?;
    env.set_field(&params, "width", "I", JValue::Int(width))?;
    env.set_field(&params, "height", "I", JValue::Int(h))?;
    env.call_method(app.views.gauge.as_obj(), "setLayoutParams",
        "(Landroid/view/ViewGroup$LayoutParams;)V", &[JValue::Object(&params)])?;
    for (view, w, h, margin) in [
        (&app.views.needle, needle_w, needle_h, bottom),
        (&app.views.pivot, diameter, diameter, (bottom - diameter/2).max(0)),
    ] {
        let params = env.new_object("android/widget/FrameLayout$LayoutParams", "(III)V",
            &[JValue::Int(w), JValue::Int(h), JValue::Int(81)])?;
        env.call_method(&params, "setMargins", "(IIII)V",
            &[JValue::Int(0), JValue::Int(0), JValue::Int(0), JValue::Int(margin)])?;
        env.call_method(view.as_obj(), "setLayoutParams", "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&params)])?;
    }
    env.call_method(app.views.needle.as_obj(), "setPivotX", "(F)V", &[JValue::Float(needle_w as f32/2.0)])?;
    env.call_method(app.views.needle.as_obj(), "setPivotY", "(F)V", &[JValue::Float(needle_h as f32)])?;
    for (view, cents) in app.views.gauge_labels.iter().zip(dial::LABEL_CENTS) {
        let params = dial_label_params(env, cents, width)?;
        env.call_method(view.as_obj(), "setLayoutParams", "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&params)])?;
        env.call_method(view.as_obj(), "setTextSize", "(IF)V",
            &[JValue::Int(TEXT_UNIT_SP), JValue::Float((width as f32 / density / 29.0).clamp(7.0, 11.0))])?;
    }
    Ok(())
}

fn set_serif_font(env: &mut JNIEnv, view: &JObject, bold: bool) -> JniResult<()> {
    let family = env.new_string("serif")?;
    let family_obj: JObject = family.into();
    let typeface = env.call_static_method(
        "android/graphics/Typeface",
        "create",
        "(Ljava/lang/String;I)Landroid/graphics/Typeface;",
        &[JValue::Object(&family_obj), JValue::Int(if bold { 1 } else { 0 })],
    )?.l()?;
    env.call_method(
        view,
        "setTypeface",
        "(Landroid/graphics/Typeface;)V",
        &[JValue::Object(&typeface)],
    )?;
    Ok(())
}

fn drawable_resource_id(env: &mut JNIEnv, activity: &JObject, name: &str) -> JniResult<i32> {
    let resources = env.call_method(activity, "getResources", "()Landroid/content/res/Resources;", &[])?.l()?;
    let package = env.call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
    let drawable_name = env.new_string(name)?;
    let drawable_kind = env.new_string("drawable")?;
    let id = env.call_method(
        &resources,
        "getIdentifier",
        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
        &[
            JValue::Object(&JObject::from(drawable_name)),
            JValue::Object(&JObject::from(drawable_kind)),
            JValue::Object(&package),
        ],
    )?.i()?;
    Ok(id)
}

fn set_rounded_background(
    env: &mut JNIEnv,
    view: &JObject,
    fill: i32,
    stroke: i32,
    radius: f32,
    stroke_width: i32,
) -> JniResult<()> {
    let drawable = env.new_object("android/graphics/drawable/GradientDrawable", "()V", &[])?;
    env.call_method(&drawable, "setColor", "(I)V", &[JValue::Int(fill)])?;
    env.call_method(
        &drawable,
        "setCornerRadius",
        "(F)V",
        &[JValue::Float(radius)],
    )?;
    env.call_method(
        &drawable,
        "setStroke",
        "(II)V",
        &[JValue::Int(stroke_width), JValue::Int(stroke)],
    )?;
    env.call_method(
        view,
        "setBackground",
        "(Landroid/graphics/drawable/Drawable;)V",
        &[JValue::Object(&drawable)],
    )?;
    Ok(())
}

fn display_density(env: &mut JNIEnv, activity: &JObject) -> f32 {
    let result = (|| -> JniResult<f32> {
        let resources = env
            .call_method(
                activity,
                "getResources",
                "()Landroid/content/res/Resources;",
                &[],
            )?
            .l()?;
        let metrics = env
            .call_method(
                &resources,
                "getDisplayMetrics",
                "()Landroid/util/DisplayMetrics;",
                &[],
            )?
            .l()?;
        env.get_field(&metrics, "density", "F")?.f()
    })();
    clear_exception(env);
    result.unwrap_or(1.0)
}

fn view_contains_point(env: &mut JNIEnv, view: &JObject, x: f32, y: f32) -> bool {
    let result = env.with_local_frame(8, |env| -> JniResult<bool> {
        let location = env.new_int_array(2)?;
        env.call_method(
            view,
            "getLocationInWindow",
            "([I)V",
            &[JValue::Object(location.as_ref())],
        )?;
        let mut coordinates = [0_i32; 2];
        env.get_int_array_region(&location, 0, &mut coordinates)?;
        let bounds = env.new_object("android/graphics/Rect", "()V", &[])?;
        let visible = env
            .call_method(
                view,
                "getLocalVisibleRect",
                "(Landroid/graphics/Rect;)Z",
                &[JValue::Object(&bounds)],
            )?
            .z()?;
        if !visible {
            return Ok(false);
        }
        let left = env.get_field(&bounds, "left", "I")?.i()?;
        let top = env.get_field(&bounds, "top", "I")?.i()?;
        let right = env.get_field(&bounds, "right", "I")?.i()?;
        let bottom = env.get_field(&bounds, "bottom", "I")?.i()?;
        let local_x = x - coordinates[0] as f32;
        let local_y = y - coordinates[1] as f32;
        Ok(local_x >= left as f32
            && local_x < right as f32
            && local_y >= top as f32
            && local_y < bottom as f32)
    });
    clear_exception(env);
    result.unwrap_or(false)
}

fn clear_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

fn startup_log(priority: c_int, message: &str) {
    if let Ok(message) = CString::new(message.replace('\0', " ")) {
        unsafe {
            __android_log_write(priority, c"XianZhun".as_ptr(), message.as_ptr());
        }
    }
}

fn report_startup_error(env: &mut JNIEnv, stage: &str, error: &jni::errors::Error) {
    let throwable = env.exception_occurred().ok();
    clear_exception(env);
    let detail = throwable
        .filter(|throwable| !throwable.is_null())
        .and_then(|throwable| {
            env.with_local_frame(8, |env| -> JniResult<String> {
                let text = env
                    .call_method(&throwable, "toString", "()Ljava/lang/String;", &[])?
                    .l()?;
                let text = JString::from(text);
                let detail: String = env.get_string(&text)?.into();
                Ok(detail)
            })
            .ok()
        });
    clear_exception(env);
    startup_log(
        6,
        &format!(
            "startup {stage}: {error}; {}",
            detail.as_deref().unwrap_or("no Java exception")
        ),
    );
}

fn prepare_view_window(env: &mut JNIEnv, activity: &JObject) -> JniResult<()> {
    let window = env
        .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
        .l()?;
    // Return the surface for framework View rendering, but keep NativeActivity's
    // input queue for ordered Rust touch commands. Handled motion events are
    // not delivered again to ToggleButton; keyboard/accessibility still work
    // through the framework fallback.
    env.call_method(
        &window,
        "takeSurface",
        "(Landroid/view/SurfaceHolder$Callback2;)V",
        &[JValue::Object(&JObject::null())],
    )?;
    env.call_method(
        &window,
        "takeInputQueue",
        "(Landroid/view/InputQueue$Callback;)V",
        &[JValue::Object(activity)],
    )?;
    startup_log(
        4,
        "startup window uses framework drawing and ordered native touch dispatch",
    );
    Ok(())
}

fn show_startup_error(env: &mut JNIEnv, activity: &JObject) {
    let result = env.with_local_frame(24, |env| -> JniResult<()> {
        let activity = env.new_local_ref(activity)?;
        prepare_view_window(env, &activity)?;
        let message = new_text_view(
            env,
            &activity,
            "界面未能加载\n请关闭应用后重新打开",
            18.0,
            WARM_WHITE,
            false,
            GRAVITY_CENTER,
        )?;
        env.call_method(
            &message,
            "setBackgroundColor",
            "(I)V",
            &[JValue::Int(MIDNIGHT)],
        )?;
        env.call_method(
            &activity,
            "setContentView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&message)],
        )?;
        Ok(())
    });
    if let Err(error) = result {
        report_startup_error(env, "error-screen", &error);
    }
}

unsafe fn app_from_activity(activity: *mut ANativeActivity) -> Option<Arc<AppState>> {
    if activity.is_null() || (*activity).instance.is_null() {
        return None;
    }
    let app = &*((*activity).instance as *const Arc<AppState>);
    Some(Arc::clone(app))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ANativeActivity_onCreate(
    activity: *mut ANativeActivity,
    _saved_state: *mut c_void,
    _saved_state_size: usize,
) {
    if activity.is_null() {
        return;
    }
    ANativeActivity_setWindowFormat(activity, WINDOW_FORMAT_RGBA_8888);

    let callbacks = (*activity).callbacks;
    if !callbacks.is_null() {
        (*callbacks).on_start = Some(on_start);
        (*callbacks).on_resume = Some(on_resume);
        (*callbacks).on_pause = Some(on_pause);
        (*callbacks).on_stop = Some(on_stop);
        (*callbacks).on_destroy = Some(on_destroy);
        (*callbacks).on_window_focus_changed = Some(on_window_focus_changed);
        (*callbacks).on_input_queue_created = Some(on_input_queue_created);
        (*callbacks).on_input_queue_destroyed = Some(on_input_queue_destroyed);
        (*callbacks).on_configuration_changed = Some(on_configuration_changed);
        (*callbacks).on_low_memory = Some(on_low_memory);
    }

    let vm = match JavaVM::from_raw((*activity).vm) {
        Ok(vm) => vm,
        Err(error) => {
            startup_log(6, &format!("startup JavaVM: {error}"));
            return;
        }
    };
    let mut env = match JNIEnv::from_raw((*activity).env) {
        Ok(env) => env,
        Err(error) => {
            startup_log(6, &format!("startup JNIEnv: {error}"));
            return;
        }
    };
    let activity_obj = JObject::from_raw((*activity).clazz);
    if let Err(error) = env.with_local_frame(8, |env| prepare_view_window(env, &activity_obj)) {
        report_startup_error(&mut env, "window-mode", &error);
        show_startup_error(&mut env, &activity_obj);
        return;
    }
    let activity_ref = match env.new_global_ref(&activity_obj) {
        Ok(activity_ref) => activity_ref,
        Err(error) => {
            report_startup_error(&mut env, "activity-reference", &error);
            show_startup_error(&mut env, &activity_obj);
            return;
        }
    };
    let views = match env.with_local_frame(256, |env| create_ui(env, &activity_obj)) {
        Ok(views) => views,
        Err(error) => {
            report_startup_error(&mut env, "create-ui", &error);
            show_startup_error(&mut env, &activity_obj);
            return;
        }
    };

    let app = AppState::new(vm, activity_ref, views);
    if resize_dial(&mut env, &app).is_err() { clear_exception(&mut env); }
    app.sync_views(&mut env, true);
    AppState::start_main_loop(&app);
    (*activity).instance = Box::into_raw(Box::new(app)) as *mut c_void;
    startup_log(4, concat!("startup ui-ready v", env!("CARGO_PKG_VERSION")));
}

unsafe extern "C" fn on_start(_activity: *mut ANativeActivity) {}

unsafe extern "C" fn on_resume(activity: *mut ANativeActivity) {
    if let Some(app) = app_from_activity(activity) {
        AppState::on_resume(&app);
    }
}

unsafe extern "C" fn on_pause(activity: *mut ANativeActivity) {
    if let Some(app) = app_from_activity(activity) {
        app.on_pause();
    }
}

unsafe extern "C" fn on_stop(activity: *mut ANativeActivity) {
    if let Some(app) = app_from_activity(activity) {
        app.on_pause();
    }
}

unsafe extern "C" fn on_destroy(activity: *mut ANativeActivity) {
    if activity.is_null() || (*activity).instance.is_null() {
        return;
    }
    let boxed = Box::from_raw((*activity).instance as *mut Arc<AppState>);
    (*activity).instance = ptr::null_mut();
    boxed.shutdown();
}

unsafe extern "C" fn on_window_focus_changed(activity: *mut ANativeActivity, focused: c_int) {
    if let Some(app) = app_from_activity(activity) {
        AppState::on_window_focus(&app, focused != 0);
    }
}

unsafe extern "C" fn on_input_queue_created(
    activity: *mut ANativeActivity,
    queue: *mut AInputQueue,
) {
    if queue.is_null() {
        return;
    }
    if let Some(app) = app_from_activity(activity) {
        AppState::set_input_queue(&app, queue);
    }
}

unsafe extern "C" fn on_input_queue_destroyed(
    activity: *mut ANativeActivity,
    queue: *mut AInputQueue,
) {
    if let Some(app) = app_from_activity(activity) {
        app.clear_input_queue(queue);
    }
}

unsafe extern "C" fn on_configuration_changed(activity: *mut ANativeActivity) {
    if let Some(app) = app_from_activity(activity) {
        if let Ok(mut env) = app.vm.attach_current_thread() {
            app.pending_inputs.lock().unwrap().clear();
            app.update_pressed(&mut env, None);
            if resize_dial(&mut env, &app).is_err() { clear_exception(&mut env); }
            app.sync_views(&mut env, true);
        }
    }
}

unsafe extern "C" fn on_low_memory(_activity: *mut ANativeActivity) {}
