#[cfg(any(target_os = "android", test))]
mod capture;
#[cfg(any(target_os = "android", test))]
mod input;
mod pitch;
#[cfg(any(target_os = "android", test))]
mod selection;
mod tuning;
#[cfg(any(target_os = "android", test))]
mod ui_state;

#[cfg(target_os = "android")]
mod native_app;
