use crate::input::InputFrame;
use crate::pitch::PitchReading;
use crate::tuning::{cents_between, classify_reading, STRINGS};
use std::time::{Duration, Instant};

const READING_HOLD: Duration = Duration::from_millis(900);
const LIVE_READING_TIMEOUT: Duration = Duration::from_millis(180);
const NEEDLE_HOLD: Duration = Duration::from_millis(380);
const PCM_TIMEOUT: Duration = Duration::from_millis(350);
const ACCESSIBILITY_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(1_500);
pub(crate) const PERMISSION_RESULT_SETTLE_DELAY: Duration = Duration::from_millis(1_500);
const MIN_TUNING_CONFIDENCE: f32 = 0.72;
const TUNED_CENTS_TOLERANCE: f32 = 4.0;
pub(crate) const TUNED_CONFIRMATION_FRAMES: u8 = 5;
const TUNED_ENTRY_CENTS: f32 = 3.0;
const TARGET_CONFIRMATION_FRAMES: u8 = 3;
const OUTLIER_CENTS: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputStatus {
    Idle,
    Starting,
    Receiving,
    DigitalSilence,
    Stalled,
    SystemSilenced,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiPhase {
    Ready,
    AwaitingSound,
    Reading,
    Fading,
    Stopped,
    Paused,
    Permission,
    MicrophoneError,
}

#[derive(Clone)]
pub(crate) struct UiSnapshot {
    pub(crate) phase: UiPhase,
    pub(crate) input_status: InputStatus,
    pub(crate) revision: u64,
    pub(crate) button: String,
    pub(crate) note: String,
    pub(crate) caption: String,
    pub(crate) frequency: String,
    pub(crate) cents: String,
    pub(crate) status: String,
    pub(crate) level: String,
    pub(crate) selected_target: Option<usize>,
    pub(crate) active_target: Option<usize>,
    pub(crate) in_tune: bool,
    pub(crate) needle_cents: Option<f32>,
    pub(crate) keep_screen_on: bool,
}

pub(crate) struct UiState {
    pub(crate) phase: UiPhase,
    pub(crate) input_status: InputStatus,
    pub(crate) revision: u64,
    pub(crate) selected_target: Option<usize>,
    pub(crate) active_target: Option<usize>,
    pub(crate) smoothed_frequency_hz: Option<f32>,
    pub(crate) last_target: Option<usize>,
    pub(crate) last_reading_at: Option<Instant>,
    pub(crate) last_pcm_at: Option<Instant>,
    pub(crate) target_epoch: u64,
    pending_target: Option<(usize, u8)>,
    pending_jump: Option<f32>,
    pub(crate) tuned_candidate_target: Option<usize>,
    pub(crate) tuned_candidate_frames: u8,
    pub(crate) note: String,
    pub(crate) caption: String,
    pub(crate) frequency: String,
    pub(crate) cents: String,
    pub(crate) status: String,
    pub(crate) level: String,
    pub(crate) button: String,
    pub(crate) needle_cents: Option<f32>,
    pub(crate) in_tune: bool,
    pub(crate) keep_screen_on: bool,
}

#[derive(Default)]
pub(crate) struct AccessibilityState {
    pub(crate) last_message: String,
    pub(crate) last_announcement_at: Option<Instant>,
    pub(crate) last_note: String,
    pub(crate) last_in_tune: bool,
    pub(crate) last_had_reading: bool,
}

impl UiState {
    pub(crate) fn new() -> Self {
        Self {
            phase: UiPhase::Ready,
            input_status: InputStatus::Idle,
            revision: 1,
            selected_target: None,
            active_target: None,
            smoothed_frequency_hz: None,
            last_target: None,
            last_reading_at: None,
            last_pcm_at: None,
            target_epoch: 0,
            pending_target: None,
            pending_jump: None,
            tuned_candidate_target: None,
            tuned_candidate_frames: 0,
            note: "待拨弦".to_string(),
            caption: "自动识别".to_string(),
            frequency: "检测 — Hz  ·  目标 — Hz".to_string(),
            cents: "— 音分".to_string(),
            status: "点开始，再拨一根空弦".to_string(),
            level: "等待声音".to_string(),
            button: "开始调音".to_string(),
            needle_cents: None,
            in_tune: false,
            keep_screen_on: false,
        }
    }

    pub(crate) fn snapshot(&self) -> UiSnapshot {
        UiSnapshot {
            phase: self.phase,
            input_status: self.input_status,
            revision: self.revision,
            button: self.button.clone(),
            note: self.note.clone(),
            caption: self.caption.clone(),
            frequency: self.frequency.clone(),
            cents: self.cents.clone(),
            status: self.status.clone(),
            level: self.level.clone(),
            selected_target: self.selected_target,
            active_target: self.active_target,
            in_tune: self.in_tune,
            needle_cents: self.needle_cents,
            keep_screen_on: self.keep_screen_on,
        }
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }

    pub(crate) fn show_permission_needed(&mut self, permanent: bool, rationale: bool) {
        self.phase = UiPhase::Permission;
        self.button = if permanent {
            "打开系统设置".to_string()
        } else {
            "允许麦克风".to_string()
        };
        self.status = if permanent {
            "请在系统设置中允许麦克风".to_string()
        } else if rationale {
            "允许麦克风后即可调音".to_string()
        } else {
            "调音需要麦克风权限".to_string()
        };
        self.keep_screen_on = false;
        self.show_idle_without_status();
        self.changed();
    }

    pub(crate) fn show_listening(&mut self) {
        self.phase = UiPhase::AwaitingSound;
        self.button = "停止调音".to_string();
        self.show_idle_without_status();
        self.input_status = InputStatus::Starting;
        self.level = "正在连接麦克风".to_string();
        self.status = self.listening_prompt();
        self.keep_screen_on = true;
        self.changed();
    }

    pub(crate) fn show_ready(&mut self) {
        self.phase = UiPhase::Ready;
        self.button = "开始调音".to_string();
        self.show_idle_without_status();
        self.status = self.ready_prompt();
        self.keep_screen_on = false;
        self.changed();
    }

    pub(crate) fn show_stopped(&mut self) {
        self.phase = UiPhase::Stopped;
        self.button = "开始调音".to_string();
        self.show_idle_without_status();
        self.status = "已停止".to_string();
        self.keep_screen_on = false;
        self.changed();
    }

    pub(crate) fn show_microphone_error(&mut self) {
        self.phase = UiPhase::MicrophoneError;
        self.button = "重试录音".to_string();
        self.status = "麦克风不可用，请结束其他录音后重试".to_string();
        self.keep_screen_on = false;
        self.show_idle_without_status();
        self.changed();
    }

    pub(crate) fn show_paused(&mut self) {
        self.phase = UiPhase::Paused;
        self.button = "开始调音".to_string();
        self.show_idle_without_status();
        self.keep_screen_on = false;
        self.status = "已暂停，点开始继续".to_string();
        self.changed();
    }

    // A bad measurement is not a new measurement. Keep a visibly historical
    // needle briefly, revoke live tune immediately and expire even if PCM stalls.
    fn lose_reading(&mut self, now: Instant) {
        self.pending_jump = None;
        self.pending_target = None;
        self.tuned_candidate_target = None;
        self.tuned_candidate_frames = 0;
        self.mark_historical(now);
    }

    fn mark_historical(&mut self, now: Instant) {
        self.in_tune = false;
        self.tuned_candidate_target = None;
        self.tuned_candidate_frames = 0;
        let age = self.last_reading_at.map(|last| now.saturating_duration_since(last));
        match age {
            Some(age) if age < READING_HOLD => {
                let was_fading = self.phase == UiPhase::Fading;
                let hide = age >= NEEDLE_HOLD && self.needle_cents.is_some();
                self.phase = UiPhase::Fading;
                if hide { self.needle_cents = None; }
                if !was_fading {
                    self.status = "信号变弱，请再拨一次".to_string();
                    self.cents = "上次读数 · 非实时".to_string();
                    self.frequency = self.frequency.replace("检测 ", "上次 ");
                }
                if !was_fading || hide { self.changed(); }
            }
            _ if self.phase != UiPhase::AwaitingSound => {
                let input = self.input_status;
                let level = self.level.clone();
                let pcm = self.last_pcm_at;
                self.show_idle_without_status();
                self.input_status = input;
                self.level = level;
                self.last_pcm_at = pcm;
                self.phase = UiPhase::AwaitingSound;
                self.status = self.listening_prompt();
                self.changed();
            }
            _ => {}
        }
    }

    pub(crate) fn expire_reading(&mut self, now: Instant) {
        if !self.accepts_input() || self.input_status == InputStatus::SystemSilenced { return; }
        if self.last_pcm_at.is_some_and(|last| now.saturating_duration_since(last) >= PCM_TIMEOUT)
            && self.input_status != InputStatus::Stalled {
            self.input_status = InputStatus::Stalled;
            self.level = "暂未收到新音频".to_string();
            self.changed();
        }
        if self.last_reading_at.is_some_and(|last| now.saturating_duration_since(last) >= LIVE_READING_TIMEOUT) {
            // Do not discard fresh candidate evidence merely because the last
            // *accepted* reading is older. A stalled stream has no fresh PCM.
            if self.input_status == InputStatus::Stalled {
                self.pending_target = None;
                self.pending_jump = None;
            }
            self.mark_historical(now);
        }
    }

    pub(crate) fn apply_reading(&mut self, reading: Option<PitchReading>) {
        let now = Instant::now();
        let estimate = reading.and_then(|reading| {
            classify_reading(&reading, self.selected_target, self.last_target)
                .map(|estimate| (reading, estimate))
        });
        let Some((reading, estimate)) = estimate else {
            self.lose_reading(now);
            return;
        };

        // Do not switch the automatic target on one stray harmonic. A manual
        // target is authoritative. On an actual switch, clear the old filter.
        if self.selected_target.is_none() && self.last_target.is_some_and(|old| old != estimate.target_index) {
            let count = match self.pending_target {
                Some((target, count)) if target == estimate.target_index => count.saturating_add(1),
                _ => 1,
            };
            self.pending_target = Some((estimate.target_index, count));
            if count < TARGET_CONFIRMATION_FRAMES {
                self.in_tune = false;
                self.tuned_candidate_frames = 0;
                self.tuned_candidate_target = None;
                self.phase = UiPhase::Fading;
                self.cents = "切换琴弦 · 确认中".to_string();
                self.frequency = self.frequency.replace("检测 ", "上次 ");
                self.status = "正在确认新琴弦".to_string();
                self.changed();
                return;
            }
            self.smoothed_frequency_hz = None;
            self.pending_jump = None;
        }
        self.pending_target = None;

        // Reject a single large excursion, but follow a sustained real turn
        // of the tuning peg without keeping an obsolete tuned indication.
        if self.last_target == Some(estimate.target_index) {
            if let Some(old) = self.smoothed_frequency_hz {
                if cents_between(estimate.frequency_hz, old).abs() > OUTLIER_CENTS
                    && !self.pending_jump.is_some_and(|pending|
                        cents_between(estimate.frequency_hz, pending).abs() <= OUTLIER_CENTS)
                {
                    self.lose_reading(now);
                    self.pending_jump = Some(estimate.frequency_hz);
                    return;
                }
            }
        }
        self.pending_jump = None;
        self.last_reading_at = Some(Instant::now());
        self.phase = UiPhase::Reading;
        let target = STRINGS[estimate.target_index];
        let smoothed = self.smooth_frequency(estimate.frequency_hz, estimate.target_index);
        let cents = cents_between(smoothed, target.frequency_hz);
        let raw_cents = cents_between(estimate.frequency_hz, target.frequency_hz);
        let tolerance = if self.in_tune { TUNED_CENTS_TOLERANCE } else { TUNED_ENTRY_CENTS };
        let candidate_in_tune = cents.abs() <= tolerance
            && raw_cents.abs() <= tolerance
            && reading.confidence >= MIN_TUNING_CONFIDENCE;
        if candidate_in_tune {
            if self.tuned_candidate_target == Some(estimate.target_index) {
                self.tuned_candidate_frames = self.tuned_candidate_frames.saturating_add(1);
            } else {
                self.tuned_candidate_target = Some(estimate.target_index);
                self.tuned_candidate_frames = 1;
            }
        } else {
            self.tuned_candidate_target = None;
            self.tuned_candidate_frames = 0;
        }
        let in_tune = candidate_in_tune && self.tuned_candidate_frames >= TUNED_CONFIRMATION_FRAMES;

        self.note = target.note.to_string();
        self.caption = if self.selected_target.is_some() {
            format!("{}  ·  已锁定", target.description)
        } else {
            format!("{}  ·  自动识别", target.description)
        };
        self.frequency = format!(
            "检测 {:.1} Hz  ·  目标 {:.2} Hz",
            smoothed, target.frequency_hz
        );
        self.cents = format_cents(cents, in_tune, reading.confidence);
        self.status = status_for(cents, in_tune, reading.confidence);
        self.active_target = Some(estimate.target_index);
        self.last_target = Some(estimate.target_index);
        self.needle_cents = Some(cents);
        self.in_tune = in_tune;
        self.keep_screen_on = true;
        self.changed();
    }

    pub(crate) fn show_capture_starting(&mut self) {
        if !self.accepts_input() {
            return;
        }
        self.show_idle_without_status();
        self.phase = UiPhase::AwaitingSound;
        self.input_status = InputStatus::Starting;
        self.level = "正在连接麦克风".to_string();
        self.status = self.listening_prompt();
        self.changed();
    }

    pub(crate) fn show_system_silenced(&mut self) {
        if !self.accepts_input() || self.input_status == InputStatus::SystemSilenced {
            return;
        }
        self.show_idle_without_status();
        self.phase = UiPhase::AwaitingSound;
        self.input_status = InputStatus::SystemSilenced;
        self.level = "麦克风被系统静音".to_string();
        self.status = "检查系统麦克风开关，或结束其他录音".to_string();
        self.changed();
    }

    /// Input delivery is independent of pitch recognition. Noise, an unlocked
    /// note and a weak waveform still prove that PCM is arriving, never tuning.
    pub(crate) fn apply_input_frame(
        &mut self,
        frame: InputFrame,
        reading: Option<PitchReading>,
        system_silenced: bool,
    ) {
        if !self.accepts_input() {
            return;
        }
        if system_silenced {
            self.show_system_silenced();
            return;
        }
        if self.input_status == InputStatus::SystemSilenced {
            self.status = self.listening_prompt();
        }
        self.last_pcm_at = Some(Instant::now());
        self.apply_reading(if frame.digital_silence { None } else { reading });
        let input_status = if frame.digital_silence {
            InputStatus::DigitalSilence
        } else {
            InputStatus::Receiving
        };
        let level = if frame.digital_silence {
            "已连接 · 输入无波形".to_string()
        } else {
            // Coarse buckets keep noise-floor changes from repainting every
            // frame while retaining a real, independently measured level.
            let db = (20.0 * frame.rms.max(f32::MIN_POSITIVE).log10() / 3.0).round() * 3.0;
            if frame.rms < crate::pitch::MIN_RMS {
                format!("输入 {db:.0} dBFS · 声音较弱")
            } else {
                format!("输入 {db:.0} dBFS")
            }
        };
        if self.input_status != input_status || self.level != level {
            self.input_status = input_status;
            self.level = level;
            self.changed();
        }
    }

    fn accepts_input(&self) -> bool {
        matches!(
            self.phase,
            UiPhase::AwaitingSound | UiPhase::Reading | UiPhase::Fading
        )
    }

    pub(crate) fn set_target(&mut self, selected: Option<usize>) {
        let selected = selected.filter(|index| *index < STRINGS.len());
        if self.selected_target == selected {
            return;
        }
        self.selected_target = selected;
        self.target_epoch = self.target_epoch.wrapping_add(1);
        let previous_pcm = self.last_pcm_at;
        let previous_input_status = self.input_status;
        let previous_level = self.level.clone();
        self.show_idle_without_status();
        match self.phase {
            UiPhase::AwaitingSound | UiPhase::Reading | UiPhase::Fading => {
                // A target change invalidates the previous note and its tuned
                // confirmation. Wait for a fresh frame for the new target.
                self.phase = UiPhase::AwaitingSound;
                self.status = self.listening_prompt();
                self.input_status = previous_input_status;
                self.level = previous_level;
                self.last_pcm_at = previous_pcm;
                if previous_input_status == InputStatus::SystemSilenced {
                    self.status = "检查系统麦克风开关，或结束其他录音".to_string();
                }
            }
            UiPhase::Ready => self.status = self.ready_prompt(),
            // A selection is not proof that permission or capture succeeded.
            // Keep the real error/pause/permission message until the adapter
            // completes the existing start/permission flow.
            UiPhase::Stopped | UiPhase::Paused | UiPhase::Permission | UiPhase::MicrophoneError => {
            }
        }
        self.changed();
    }

    fn listening_prompt(&self) -> String {
        self.selected_target
            .map(|index| {
                let target = STRINGS[index];
                format!(
                    "已锁定 {} {}，轻拨这根空弦。",
                    target.description, target.note
                )
            })
            .unwrap_or_else(|| "轻拨一根空弦".to_string())
    }

    fn ready_prompt(&self) -> String {
        if self.selected_target.is_some() {
            "点开始，再拨已锁定的空弦".to_string()
        } else {
            "点开始，再拨一根空弦".to_string()
        }
    }

    fn show_idle_without_status(&mut self) {
        self.input_status = InputStatus::Idle;
        let target = self.selected_target.map(|index| STRINGS[index]);
        self.active_target = self.selected_target;
        self.smoothed_frequency_hz = None;
        self.last_target = None;
        self.last_reading_at = None;
        self.last_pcm_at = None;
        self.pending_target = None;
        self.pending_jump = None;
        self.tuned_candidate_target = None;
        self.tuned_candidate_frames = 0;
        self.note = target
            .map(|target| target.note.to_string())
            .unwrap_or_else(|| "待拨弦".to_string());
        self.caption = target
            .map(|target| format!("{}  ·  已锁定", target.description))
            .unwrap_or_else(|| "自动识别".to_string());
        self.frequency = target
            .map(|target| format!("检测 — Hz  ·  目标 {:.2} Hz", target.frequency_hz))
            .unwrap_or_else(|| "检测 — Hz  ·  目标 — Hz".to_string());
        self.cents = "— 音分".to_string();
        self.level = if matches!(self.phase, UiPhase::Stopped | UiPhase::Paused | UiPhase::Ready | UiPhase::Permission | UiPhase::MicrophoneError) {
            "麦克风未录音".to_string()
        } else { "等待声音".to_string() };
        self.needle_cents = None;
        self.in_tune = false;
    }

    fn smooth_frequency(&mut self, frequency_hz: f32, target_index: usize) -> f32 {
        let next = match (self.smoothed_frequency_hz, self.last_target) {
            (Some(previous), Some(previous_target)) if previous_target == target_index => {
                let delta = cents_between(frequency_hz, previous);
                // Continuous, adaptive gain in logarithmic (cents) space.
                let gain = 0.28 + 0.62 * (delta.abs() / 22.0).clamp(0.0, 1.0);
                previous * 2.0_f32.powf(delta * gain / 1200.0)
            }
            _ => frequency_hz,
        };
        self.smoothed_frequency_hz = Some(next);
        next
    }
}

impl AccessibilityState {
    pub(crate) fn next_announcement(&mut self, snapshot: &UiSnapshot) -> Option<String> {
        let has_reading = snapshot.needle_cents.is_some();
        let message = if has_reading {
            format!("{}，{}。{}", snapshot.note, snapshot.cents, snapshot.status)
        } else {
            snapshot.status.clone()
        };
        let tune_achieved = snapshot.in_tune && !self.last_in_tune;
        let note_changed = has_reading && self.last_had_reading && snapshot.note != self.last_note;
        let reading_explicitly_stopped =
            !has_reading && self.last_had_reading && snapshot.phase != UiPhase::Fading;
        let static_state_changed = !has_reading
            && message != self.last_message
            && snapshot.phase != UiPhase::Fading
            && snapshot.phase != UiPhase::AwaitingSound;
        let now = Instant::now();
        let interval_elapsed = self
            .last_announcement_at
            .map(|last| now.duration_since(last) >= ACCESSIBILITY_ANNOUNCEMENT_INTERVAL)
            .unwrap_or(true);

        self.last_note.clone_from(&snapshot.note);
        self.last_in_tune = snapshot.in_tune;
        self.last_had_reading = has_reading;

        if message == self.last_message
            || (!interval_elapsed
                && !tune_achieved
                && !note_changed
                && !reading_explicitly_stopped
                && !static_state_changed)
        {
            return None;
        }
        self.last_message.clone_from(&message);
        self.last_announcement_at = Some(now);
        Some(message)
    }
}

pub(crate) fn progress_accessibility_description(snapshot: &UiSnapshot) -> String {
    if snapshot.phase == UiPhase::Fading {
        "上次读数，非实时音高".to_string()
    } else if snapshot.needle_cents.is_none() {
        "音高指示，暂无稳定读数".to_string()
    } else {
        format!("音高指示，{}，{}", snapshot.note, snapshot.cents)
    }
}

fn format_cents(cents: f32, in_tune: bool, confidence: f32) -> String {
    if in_tune {
        return "已调准".to_string();
    }
    let amount = cents.abs();
    if amount <= TUNED_CENTS_TOLERANCE {
        return if confidence < MIN_TUNING_CONFIDENCE {
            "接近目标 · 待稳定".to_string()
        } else {
            "接近目标 · 确认中".to_string()
        };
    }
    if amount > 50.0 {
        if cents < 0.0 {
            "偏低超过 50 音分".to_string()
        } else {
            "偏高超过 50 音分".to_string()
        }
    } else if cents < 0.0 {
        format!("偏低 {:.0} 音分", amount)
    } else {
        format!("偏高 {:.0} 音分", amount)
    }
}

fn status_for(cents: f32, in_tune: bool, confidence: f32) -> String {
    if confidence < MIN_TUNING_CONFIDENCE {
        return "单独拨一根弦，等声音稳定".to_string();
    }
    if in_tune {
        return "可以换下一根弦".to_string();
    }
    if cents.abs() <= TUNED_CENTS_TOLERANCE {
        return "保持琴弦，等声音稳定".to_string();
    }
    if cents < 0.0 {
        "轻轻拧紧琴弦".to_string()
    } else {
        "轻轻放松琴弦".to_string()
    }
}

pub(crate) fn permission_request_should_settle(
    in_flight: bool,
    has_window_focus: bool,
    elapsed: Option<Duration>,
) -> bool {
    in_flight
        && has_window_focus
        && elapsed.is_some_and(|elapsed| elapsed >= PERMISSION_RESULT_SETTLE_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stable_reading(frequency_hz: f32) -> PitchReading {
        PitchReading {
            frequency_hz,
            confidence: 0.95,
            level: 0.2,
            harmonic_support: 0.2,
            subharmonic_support: 0.0,
        }
    }

    #[test]
    fn tuning_requires_consecutive_stable_frames() {
        let mut ui = UiState::new();
        for frame in 1..=TUNED_CONFIRMATION_FRAMES {
            ui.apply_reading(Some(stable_reading(STRINGS[0].frequency_hz)));
            assert_eq!(ui.tuned_candidate_frames, frame);
            assert_eq!(ui.in_tune, frame >= TUNED_CONFIRMATION_FRAMES);
        }
        assert_eq!(ui.active_target, Some(0));
        assert!(ui.needle_cents.is_some_and(|cents| cents.abs() < 0.01));
    }

    #[test]
    fn invalid_reading_revokes_tuned_state() {
        let mut ui = UiState::new();
        for _ in 0..TUNED_CONFIRMATION_FRAMES {
            ui.apply_reading(Some(stable_reading(STRINGS[0].frequency_hz)));
        }
        let mut invalid = stable_reading(STRINGS[0].frequency_hz);
        invalid.confidence = f32::NAN;
        ui.apply_reading(Some(invalid));
        assert!(!ui.in_tune);
        assert_eq!(ui.tuned_candidate_frames, 0);
        assert!(ui.needle_cents.is_some()); // historical only
        assert!(ui.phase == UiPhase::Fading);

        ui.apply_reading(Some(stable_reading(STRINGS[0].frequency_hz)));
        assert!(!ui.in_tune);
    }

    #[test]
    fn target_change_and_lifecycle_states_reset_transient_readings() {
        let mut ui = UiState::new();
        ui.apply_reading(Some(stable_reading(STRINGS[1].frequency_hz)));
        ui.set_target(Some(2));
        assert_eq!(ui.selected_target, Some(2));
        assert_eq!(ui.active_target, Some(2));
        assert!(ui.smoothed_frequency_hz.is_none());
        assert!(ui.needle_cents.is_none());

        ui.show_listening();
        assert!(ui.keep_screen_on);
        ui.show_paused();
        assert!(!ui.keep_screen_on);
        assert!(ui.phase == UiPhase::Paused);
        ui.show_microphone_error();
        assert!(ui.phase == UiPhase::MicrophoneError);
        assert!(!ui.in_tune);
        assert!(ui.needle_cents.is_none());
    }

    #[test]
    fn frequency_smoothing_is_continuous_and_resets_on_target_change() {
        let mut ui = UiState::new();
        let target = STRINGS[0].frequency_hz;
        assert_eq!(ui.smooth_frequency(target, 0), target);
        ui.last_target = Some(0);

        let near = target * 2.0_f32.powf(4.0 / 1200.0);
        let smoothed = ui.smooth_frequency(near, 0);
        assert!(smoothed > target && smoothed < near);

        let far = target * 2.0_f32.powf(20.0 / 1200.0);
        let updated = ui.smooth_frequency(far, 0);
        assert!(updated > smoothed && updated < far);
        assert_eq!(ui.smooth_frequency(target, 1), target);
    }

    #[test]
    fn low_confidence_and_out_of_tune_readings_break_confirmation() {
        let mut ui = UiState::new();
        let target = STRINGS[0].frequency_hz;
        for _ in 0..2 {
            ui.apply_reading(Some(stable_reading(target)));
        }
        let mut weak = stable_reading(target);
        weak.confidence = 0.65;
        ui.apply_reading(Some(weak));
        assert!(!ui.in_tune);
        assert_eq!(ui.tuned_candidate_frames, 0);
        ui.apply_reading(Some(stable_reading(target)));
        assert!(!ui.in_tune);
        ui.apply_reading(Some(stable_reading(target * 2.0_f32.powf(12.0 / 1200.0))));
        assert!(!ui.in_tune);
        assert_eq!(ui.tuned_candidate_frames, 0);
    }

    #[test]
    fn expired_signal_clears_target_and_repeated_silence_is_not_dirty() {
        let mut ui = UiState::new();
        ui.show_listening();
        let idle_revision = ui.revision;
        ui.apply_reading(None);
        assert_eq!(ui.revision, idle_revision);
        ui.apply_reading(Some(stable_reading(STRINGS[0].frequency_hz)));
        ui.apply_reading(None);
        let fading_revision = ui.revision;
        ui.apply_reading(None);
        assert_eq!(ui.revision, fading_revision);
        ui.last_reading_at = Some(Instant::now() - READING_HOLD - Duration::from_millis(1));
        ui.apply_reading(None);
        assert!(ui.needle_cents.is_none());
        assert!(ui.active_target.is_none());
        assert!(!ui.in_tune);
    }

    #[test]
    fn permission_request_polling_waits_for_focus_and_settle_delay() {
        assert!(!permission_request_should_settle(
            false,
            true,
            Some(PERMISSION_RESULT_SETTLE_DELAY)
        ));
        assert!(!permission_request_should_settle(
            true,
            false,
            Some(PERMISSION_RESULT_SETTLE_DELAY)
        ));
        assert!(!permission_request_should_settle(
            true,
            true,
            Some(PERMISSION_RESULT_SETTLE_DELAY - Duration::from_millis(1))
        ));
        assert!(permission_request_should_settle(
            true,
            true,
            Some(PERMISSION_RESULT_SETTLE_DELAY)
        ));
    }

    #[test]
    fn pcm_delivery_is_visible_without_a_valid_pitch() {
        let mut ui = UiState::new();
        ui.show_listening();
        assert_eq!(ui.input_status, InputStatus::Starting);
        let before = ui.revision;
        let frame = InputFrame::from_pcm(&[-8, 8, -8, 8]).unwrap();
        ui.apply_input_frame(frame, None, false);
        assert_eq!(ui.input_status, InputStatus::Receiving);
        assert!(ui.revision > before);
        assert!(ui.needle_cents.is_none());
        assert!(!ui.in_tune);
        let received = ui.revision;
        ui.apply_input_frame(frame, None, false);
        assert_eq!(ui.revision, received);
    }

    #[test]
    fn constant_pcm_is_distinct_from_waiting_for_the_first_frame() {
        let mut ui = UiState::new();
        ui.show_listening();
        let frame = InputFrame::from_pcm(&[0; 32]).unwrap();
        ui.apply_input_frame(frame, None, false);
        assert_eq!(ui.input_status, InputStatus::DigitalSilence);
        let revision = ui.revision;
        ui.apply_input_frame(frame, None, false);
        assert_eq!(ui.revision, revision);
        assert!(ui.needle_cents.is_none());
    }

    #[test]
    fn system_silencing_revokes_tuning_and_requires_fresh_confirmation() {
        let mut ui = UiState::new();
        ui.show_listening();
        let frame = InputFrame::from_pcm(&[-1000, 1000]).unwrap();
        let reading = Some(stable_reading(STRINGS[0].frequency_hz));
        for _ in 0..TUNED_CONFIRMATION_FRAMES {
            ui.apply_input_frame(frame, reading, false);
        }
        assert!(ui.in_tune);
        ui.apply_input_frame(frame, reading, true);
        assert_eq!(ui.input_status, InputStatus::SystemSilenced);
        assert!(!ui.in_tune);
        assert!(ui.needle_cents.is_none());
        assert_eq!(ui.tuned_candidate_frames, 0);
        let muted = ui.revision;
        ui.apply_input_frame(frame, reading, true);
        assert_eq!(ui.revision, muted);
        ui.apply_input_frame(frame, reading, false);
        assert_eq!(ui.input_status, InputStatus::Receiving);
        assert!(!ui.in_tune);
        assert_eq!(ui.tuned_candidate_frames, 1);
    }

    #[test]
    fn digital_silence_cannot_confirm_tuning_even_with_a_stale_pitch() {
        let mut ui = UiState::new();
        ui.show_listening();
        let frame = InputFrame::from_pcm(&[11; 32]).unwrap();
        for _ in 0..TUNED_CONFIRMATION_FRAMES {
            ui.apply_input_frame(frame, Some(stable_reading(STRINGS[0].frequency_hz)), false);
        }
        assert!(!ui.in_tune);
        assert!(ui.needle_cents.is_none());
        assert_eq!(ui.tuned_candidate_frames, 0);
        assert_eq!(ui.input_status, InputStatus::DigitalSilence);
    }

    #[test]
    fn cancelled_or_denied_sessions_reject_late_input_and_mute_updates() {
        for phase in [
            UiPhase::Ready,
            UiPhase::Stopped,
            UiPhase::Paused,
            UiPhase::Permission,
            UiPhase::MicrophoneError,
        ] {
            let mut ui = UiState::new();
            ui.phase = phase;
            let before = ui.revision;
            ui.apply_input_frame(
                InputFrame::from_pcm(&[-1000, 1000]).unwrap(),
                Some(stable_reading(STRINGS[0].frequency_hz)),
                false,
            );
            ui.show_system_silenced();
            ui.show_capture_starting();
            assert!(ui.phase == phase);
            assert_eq!(ui.input_status, InputStatus::Idle);
            assert_eq!(ui.revision, before);
            assert!(ui.needle_cents.is_none());
        }
    }

    #[test]
    fn selecting_a_string_preserves_system_silencing_and_input_delivery() {
        let mut ui = UiState::new();
        ui.show_listening();
        ui.show_system_silenced();
        ui.set_target(Some(1));
        assert_eq!(ui.selected_target, Some(1));
        assert_eq!(ui.input_status, InputStatus::SystemSilenced);
        assert!(ui.needle_cents.is_none());
        ui.apply_input_frame(InputFrame::from_pcm(&[-10, 10]).unwrap(), None, false);
        ui.set_target(Some(2));
        assert_eq!(ui.input_status, InputStatus::Receiving);
        assert!(!ui.in_tune);
    }

    #[test]
    fn stalled_pcm_expires_live_tune_without_another_frame() {
        let mut ui = UiState::new();
        ui.show_listening();
        let frame = InputFrame::from_pcm(&[-2000, 2000]).unwrap();
        for _ in 0..TUNED_CONFIRMATION_FRAMES {
            ui.apply_input_frame(frame, Some(stable_reading(82.41)), false);
        }
        assert!(ui.in_tune);
        let last = ui.last_reading_at.unwrap();
        ui.expire_reading(last + LIVE_READING_TIMEOUT);
        assert!(!ui.in_tune);
        assert!(ui.phase == UiPhase::Fading);
        assert!(ui.needle_cents.is_some());
        ui.expire_reading(last + NEEDLE_HOLD);
        assert!(ui.needle_cents.is_none());
        assert!(ui.input_status == InputStatus::Stalled);
        ui.expire_reading(last + READING_HOLD);
        assert!(ui.active_target.is_none());
    }

    #[test]
    fn auto_switch_requires_multiple_consistent_frames() {
        let mut ui = UiState::new();
        ui.show_listening();
        ui.apply_reading(Some(stable_reading(82.41)));
        ui.apply_reading(Some(stable_reading(110.0)));
        assert_eq!(ui.active_target, Some(0));
        assert!(!ui.in_tune);
        ui.apply_reading(Some(stable_reading(110.0)));
        assert_eq!(ui.active_target, Some(0));
        ui.apply_reading(Some(stable_reading(110.0)));
        assert_eq!(ui.active_target, Some(1));
        assert!(!ui.in_tune);
    }

    #[test]
    fn one_large_outlier_cannot_move_live_needle_or_confirm_tune() {
        let mut ui = UiState::new();
        ui.show_listening();
        for _ in 0..TUNED_CONFIRMATION_FRAMES {
            ui.apply_reading(Some(stable_reading(82.41)));
        }
        let needle = ui.needle_cents;
        ui.apply_reading(Some(stable_reading(84.5)));
        assert_eq!(ui.needle_cents, needle);
        assert!(!ui.in_tune);
        ui.apply_reading(Some(stable_reading(84.5)));
        assert!(ui.needle_cents.unwrap() > 20.0);
    }

}
