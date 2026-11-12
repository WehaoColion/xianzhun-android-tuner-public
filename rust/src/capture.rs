//! Platform-independent recovery policy used by the Android capture worker.
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptureConfig {
    pub(crate) sample_rate: i32,
    pub(crate) source: i32,
    digital_silence_retry: bool,
}

// MIC, VOICE_RECOGNITION, UNPROCESSED. A successfully constructed AudioRecord
// is not proof that the source can start recording or deliver complete frames.
const CONFIGURATIONS: [CaptureConfig; 6] = [
    CaptureConfig {
        sample_rate: 48_000,
        source: 1,
        digital_silence_retry: true,
    },
    CaptureConfig {
        sample_rate: 48_000,
        source: 6,
        digital_silence_retry: true,
    },
    CaptureConfig {
        sample_rate: 48_000,
        source: 9,
        digital_silence_retry: true,
    },
    CaptureConfig {
        sample_rate: 44_100,
        source: 1,
        digital_silence_retry: true,
    },
    CaptureConfig {
        sample_rate: 44_100,
        source: 6,
        digital_silence_retry: true,
    },
    CaptureConfig {
        sample_rate: 44_100,
        source: 9,
        digital_silence_retry: false,
    },
];

impl CaptureConfig {
    pub(crate) fn can_retry_digital_silence(self) -> bool {
        // Digital silence alone cannot distinguish a quiet/noise-gated room
        // from an unusable source. Preserve the final configured recorder so
        // it can still respond when the user plucks a string later.
        self.digital_silence_retry
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureAttempt {
    Retry,
    NoWaveform,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureOutcome {
    Unavailable,
    Stopped,
}

/// Each attempt owns its recorder through release before returning. Try each
/// configuration once, remembering recorders that delivered constant complete
/// frames. If later configurations fail, restore the latest such recorder once
/// with digital-silence retry disabled, so quiet rooms can still be tuned later.
pub(crate) fn run_capture(
    mut active: impl FnMut() -> bool,
    mut attempt: impl FnMut(CaptureConfig) -> CaptureAttempt,
) -> CaptureOutcome {
    let mut last_constant_configuration = None;
    for configuration in CONFIGURATIONS {
        if !active() {
            return CaptureOutcome::Stopped;
        }
        match attempt(configuration) {
            CaptureAttempt::Stopped => return CaptureOutcome::Stopped,
            CaptureAttempt::NoWaveform => last_constant_configuration = Some(configuration),
            CaptureAttempt::Retry => {}
        }
    }
    if !active() {
        return CaptureOutcome::Stopped;
    }
    if let Some(configuration) = last_constant_configuration {
        let restored = CaptureConfig {
            digital_silence_retry: false,
            ..configuration
        };
        // With retries disabled the adapter keeps reading through digital
        // silence. Only a real startup/read/stall failure returns Retry.
        // NoWaveform must not become a microphone-unavailable diagnosis.
        return match attempt(restored) {
            CaptureAttempt::Retry if active() => CaptureOutcome::Unavailable,
            _ => CaptureOutcome::Stopped,
        };
    }
    CaptureOutcome::Unavailable
}

/// Measure complete-frame progress, not the number of successful read calls.
/// A trickle of partial reads must not keep an unusable stream alive forever.
/// A full frame containing silence is still progress; this is not a sound gate.
pub(crate) struct FrameWatchdog {
    last_frame_at: Duration,
}

impl FrameWatchdog {
    pub(crate) const TIMEOUT: Duration = Duration::from_secs(2);

    pub(crate) fn new(now: Duration) -> Self {
        Self { last_frame_at: now }
    }

    pub(crate) fn frame_received(&mut self, now: Duration) {
        self.last_frame_at = now;
    }

    pub(crate) fn timed_out(&self, now: Duration) -> bool {
        now.saturating_sub(self.last_frame_at) >= Self::TIMEOUT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureSignalStatus {
    Receiving,
    SystemSilenced,
    ZeroSignalTimeout,
    NoWaveform,
}

/// A recorder can deliver complete PCM frames while providing no waveform.
/// Detect digital silence independently of the complete-frame watchdog and
/// pitch recognition: even one-bit waveform changes prove PCM is not constant.
/// Known system silencing must never cause attempts to bypass capture policy.
#[derive(Default)]
pub(crate) struct DigitalSilenceWatchdog {
    constant_since: Option<Duration>,
}

impl DigitalSilenceWatchdog {
    pub(crate) const TIMEOUT: Duration = Duration::from_secs(2);

    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn observe_frame(
        &mut self,
        now: Duration,
        samples: &[i16],
        system_silenced: bool,
        allow_retry: bool,
    ) -> CaptureSignalStatus {
        if system_silenced {
            // Give an unmuted stream its own observation period. Time spent
            // intentionally muted is not evidence of a broken source.
            self.constant_since = None;
            return CaptureSignalStatus::SystemSilenced;
        }
        let Some(first) = samples.first() else {
            // Missing PCM belongs to FrameWatchdog. It must not reset an
            // already observed run of constant complete frames.
            return CaptureSignalStatus::Receiving;
        };
        if samples.iter().any(|sample| sample != first) {
            self.constant_since = None;
            return CaptureSignalStatus::Receiving;
        }
        let started = *self.constant_since.get_or_insert(now);
        if now.saturating_sub(started) >= Self::TIMEOUT {
            if allow_retry {
                CaptureSignalStatus::ZeroSignalTimeout
            } else {
                CaptureSignalStatus::NoWaveform
            }
        } else {
            CaptureSignalStatus::Receiving
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn start_failure_falls_back_to_the_next_source() {
        let mut tried = Vec::new();
        let outcome = run_capture(
            || true,
            |configuration| {
                tried.push(configuration);
                if configuration.source == 1 {
                    CaptureAttempt::Retry
                } else {
                    CaptureAttempt::Stopped
                }
            },
        );
        assert_eq!(tried, CONFIGURATIONS[..2]);
        assert_eq!(outcome, CaptureOutcome::Stopped);
    }

    #[test]
    fn read_failure_or_stall_also_falls_back() {
        let mut tried = Vec::new();
        let outcome = run_capture(
            || true,
            |configuration| {
                tried.push(configuration);
                // The Android adapter returns Retry after releasing a recorder
                // whose start, read, or complete-frame watchdog failed.
                if tried.len() < 3 {
                    CaptureAttempt::Retry
                } else {
                    CaptureAttempt::Stopped
                }
            },
        );
        assert_eq!(tried, CONFIGURATIONS[..3]);
        assert_eq!(outcome, CaptureOutcome::Stopped);
    }

    #[test]
    fn unsupported_rate_does_not_prevent_trying_the_other_rate() {
        let mut tried = Vec::new();
        run_capture(
            || true,
            |configuration| {
                tried.push(configuration);
                if configuration.sample_rate == 48_000 {
                    CaptureAttempt::Retry
                } else {
                    CaptureAttempt::Stopped
                }
            },
        );
        assert_eq!(tried, CONFIGURATIONS[..4]);
    }

    #[test]
    fn all_failures_are_bounded_and_report_unavailable() {
        let mut tried = Vec::new();
        let outcome = run_capture(
            || true,
            |configuration| {
                tried.push(configuration);
                CaptureAttempt::Retry
            },
        );
        assert_eq!(tried, CONFIGURATIONS);
        assert_eq!(outcome, CaptureOutcome::Unavailable);
    }

    #[test]
    fn stopping_before_capture_does_not_open_a_microphone() {
        let outcome = run_capture(|| false, |_| panic!("must not start after stop"));
        assert_eq!(outcome, CaptureOutcome::Stopped);
    }

    #[test]
    fn stopping_during_a_failed_attempt_does_not_start_another() {
        let active = Cell::new(true);
        let mut attempts = 0;
        let outcome = run_capture(
            || active.get(),
            |_| {
                attempts += 1;
                active.set(false);
                CaptureAttempt::Retry
            },
        );
        assert_eq!(attempts, 1);
        assert_eq!(outcome, CaptureOutcome::Stopped);
    }

    #[test]
    fn stopping_during_the_last_failure_does_not_report_an_error() {
        let active = Cell::new(true);
        let mut attempts = 0;
        let outcome = run_capture(
            || active.get(),
            |_| {
                attempts += 1;
                if attempts == CONFIGURATIONS.len() {
                    active.set(false);
                }
                CaptureAttempt::Retry
            },
        );
        assert_eq!(attempts, CONFIGURATIONS.len());
        assert_eq!(outcome, CaptureOutcome::Stopped);
    }

    #[test]
    fn zero_reads_time_out_at_the_deadline() {
        let watchdog = FrameWatchdog::new(Duration::ZERO);
        assert!(!watchdog.timed_out(Duration::from_millis(1_999)));
        assert!(watchdog.timed_out(Duration::from_millis(2_000)));
    }

    #[test]
    fn incomplete_frames_cannot_postpone_the_deadline() {
        let watchdog = FrameWatchdog::new(Duration::ZERO);
        // Partial reads never call frame_received. Repeated polling is not
        // progress, even when each poll returns one or more PCM samples.
        for millis in (0..2_000).step_by(20) {
            assert!(!watchdog.timed_out(Duration::from_millis(millis)));
        }
        assert!(watchdog.timed_out(Duration::from_secs(2)));
    }

    #[test]
    fn full_frames_including_silence_keep_capture_alive() {
        let mut watchdog = FrameWatchdog::new(Duration::ZERO);
        for frame in 1..=100 {
            let now = Duration::from_millis(frame * 90);
            assert!(!watchdog.timed_out(now));
            // The watchdog deliberately knows nothing about sample amplitude
            // or pitch recognition: quiet rooms must not cause source retries.
            watchdog.frame_received(now);
        }
        assert!(!watchdog.timed_out(Duration::from_millis(10_999)));
        assert!(watchdog.timed_out(Duration::from_millis(11_000)));
    }

    #[test]
    fn digital_zero_frames_time_out_at_their_own_deadline() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        assert_eq!(
            watchdog.observe_frame(Duration::ZERO, &[0; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_millis(1_999), &[0; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(2), &[0; 4096], false, true),
            CaptureSignalStatus::ZeroSignalTimeout
        );
    }

    #[test]
    fn waiting_for_the_first_frame_is_not_digital_silence() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(10), &[0; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(12), &[0; 4096], false, true),
            CaptureSignalStatus::ZeroSignalTimeout
        );
    }

    #[test]
    fn constant_dc_frames_also_time_out_without_becoming_sound() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        assert_eq!(
            watchdog.observe_frame(Duration::ZERO, &[34; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        // Changing the DC offset between frames is not a waveform within one.
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(2), &[35; 4096], false, true),
            CaptureSignalStatus::ZeroSignalTimeout
        );
    }

    #[test]
    fn a_one_bit_waveform_restores_the_digital_silence_deadline() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        watchdog.observe_frame(Duration::ZERO, &[0; 4096], false, true);
        assert_eq!(
            watchdog.observe_frame(Duration::from_millis(1_999), &[0, 1, 0, -1], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(2), &[0; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_millis(3_999), &[0; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(4), &[0; 4096], false, true),
            CaptureSignalStatus::ZeroSignalTimeout
        );
    }

    #[test]
    fn quiet_nonzero_waveforms_do_not_trigger_source_recovery() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        for second in 0..=120 {
            assert_eq!(
                watchdog.observe_frame(Duration::from_secs(second), &[1, 0, -1, 0], false, true),
                CaptureSignalStatus::Receiving
            );
        }
    }

    #[test]
    fn reported_system_silencing_never_requests_a_source_retry() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        watchdog.observe_frame(Duration::ZERO, &[0; 4096], false, true);
        for second in [2, 3, 6, 20] {
            assert_eq!(
                watchdog.observe_frame(Duration::from_secs(second), &[0; 4096], true, true),
                CaptureSignalStatus::SystemSilenced
            );
        }
        // The stream gets two seconds of observations after policy unmutes it.
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(21), &[0; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_millis(22_999), &[0; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(23), &[0; 4096], false, true),
            CaptureSignalStatus::ZeroSignalTimeout
        );
    }

    #[test]
    fn reported_system_silencing_has_priority_over_pcm_content() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        assert_eq!(
            watchdog.observe_frame(Duration::ZERO, &[0, 1, 0, -1], true, true),
            CaptureSignalStatus::SystemSilenced
        );
    }

    #[test]
    fn missing_frames_do_not_postpone_digital_silence() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        watchdog.observe_frame(Duration::ZERO, &[0; 4096], false, true);
        watchdog.observe_frame(Duration::from_millis(1_999), &[], false, true);
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(2), &[0; 4096], false, true),
            CaptureSignalStatus::ZeroSignalTimeout
        );
    }

    #[test]
    fn reversed_observation_time_does_not_prematurely_time_out() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        watchdog.observe_frame(Duration::from_secs(10), &[0; 4096], false, true);
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(9), &[0; 4096], false, true),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(12), &[0; 4096], false, true),
            CaptureSignalStatus::ZeroSignalTimeout
        );
    }

    #[test]
    fn only_the_final_configuration_preserves_digitally_quiet_capture() {
        for (index, configuration) in CONFIGURATIONS.iter().enumerate() {
            assert_eq!(
                configuration.can_retry_digital_silence(),
                index + 1 < CONFIGURATIONS.len()
            );
        }
    }

    #[test]
    fn the_final_quiet_recorder_waits_and_recovers_when_a_waveform_arrives() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        let final_configuration = CONFIGURATIONS[CONFIGURATIONS.len() - 1];
        let allow_retry = final_configuration.can_retry_digital_silence();
        assert_eq!(
            watchdog.observe_frame(Duration::ZERO, &[0; 4096], false, allow_retry),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_millis(1_999), &[0; 4096], false, allow_retry),
            CaptureSignalStatus::Receiving
        );
        for second in [2, 12, 120] {
            assert_eq!(
                watchdog.observe_frame(Duration::from_secs(second), &[0; 4096], false, allow_retry),
                CaptureSignalStatus::NoWaveform
            );
        }
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(121), &[0, 1, 0, -1], false, allow_retry),
            CaptureSignalStatus::Receiving
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(122), &[0; 4096], false, allow_retry),
            CaptureSignalStatus::Receiving
        );
    }

    #[test]
    fn system_silencing_has_priority_even_on_the_final_quiet_recorder() {
        let mut watchdog = DigitalSilenceWatchdog::new();
        watchdog.observe_frame(Duration::ZERO, &[0; 4096], false, false);
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(12), &[0; 4096], true, false),
            CaptureSignalStatus::SystemSilenced
        );
        assert_eq!(
            watchdog.observe_frame(Duration::from_secs(13), &[0; 4096], false, false),
            CaptureSignalStatus::Receiving
        );
    }

    #[test]
    fn unsupported_final_sources_restore_a_recorder_that_delivered_pcm() {
        let mut tried = Vec::new();
        let outcome = run_capture(
            || true,
            |configuration| {
                tried.push(configuration);
                if !configuration.can_retry_digital_silence()
                    && configuration.source == CONFIGURATIONS[0].source
                {
                    let mut watchdog = DigitalSilenceWatchdog::new();
                    watchdog.observe_frame(Duration::ZERO, &[0; 4096], false, false);
                    assert_eq!(
                        watchdog.observe_frame(Duration::from_secs(120), &[0; 4096], false, false),
                        CaptureSignalStatus::NoWaveform
                    );
                    assert_eq!(
                        watchdog.observe_frame(
                            Duration::from_secs(121),
                            &[0, 1, 0, -1],
                            false,
                            false
                        ),
                        CaptureSignalStatus::Receiving
                    );
                    CaptureAttempt::Stopped
                } else if configuration == CONFIGURATIONS[0] {
                    CaptureAttempt::NoWaveform
                } else {
                    // Other sources/rates, including the final UNPROCESSED
                    // configuration, can be unsupported on the same device.
                    CaptureAttempt::Retry
                }
            },
        );
        assert_eq!(outcome, CaptureOutcome::Stopped);
        assert_eq!(&tried[..CONFIGURATIONS.len()], &CONFIGURATIONS);
        assert_eq!(tried.len(), CONFIGURATIONS.len() + 1);
        let restored = tried.last().unwrap();
        assert_eq!(restored.source, CONFIGURATIONS[0].source);
        assert_eq!(restored.sample_rate, CONFIGURATIONS[0].sample_rate);
        assert!(!restored.can_retry_digital_silence());
    }

    #[test]
    fn restores_the_most_recent_recorder_that_delivered_constant_pcm() {
        let mut tried = Vec::new();
        let outcome = run_capture(
            || true,
            |configuration| {
                tried.push(configuration);
                if tried.len() > CONFIGURATIONS.len() {
                    return CaptureAttempt::Stopped;
                }
                if configuration == CONFIGURATIONS[0] || configuration == CONFIGURATIONS[4] {
                    CaptureAttempt::NoWaveform
                } else {
                    CaptureAttempt::Retry
                }
            },
        );
        assert_eq!(outcome, CaptureOutcome::Stopped);
        assert_eq!(tried.len(), CONFIGURATIONS.len() + 1);
        let restored = tried.last().unwrap();
        assert_eq!(restored.source, CONFIGURATIONS[4].source);
        assert_eq!(restored.sample_rate, CONFIGURATIONS[4].sample_rate);
        assert!(!restored.can_retry_digital_silence());
    }

    #[test]
    fn a_restored_recorder_failure_reports_unavailable_after_one_restore() {
        let mut tried = Vec::new();
        let outcome = run_capture(
            || true,
            |configuration| {
                tried.push(configuration);
                if configuration == CONFIGURATIONS[0] {
                    CaptureAttempt::NoWaveform
                } else {
                    CaptureAttempt::Retry
                }
            },
        );
        assert_eq!(outcome, CaptureOutcome::Unavailable);
        assert_eq!(tried.len(), CONFIGURATIONS.len() + 1);
        assert!(!tried.last().unwrap().can_retry_digital_silence());
    }

    #[test]
    fn stopping_after_final_source_failure_does_not_restore_a_recorder() {
        let active = Cell::new(true);
        let mut attempts = 0;
        let outcome = run_capture(
            || active.get(),
            |_| {
                attempts += 1;
                if attempts == 1 {
                    CaptureAttempt::NoWaveform
                } else {
                    if attempts == CONFIGURATIONS.len() {
                        active.set(false);
                    }
                    CaptureAttempt::Retry
                }
            },
        );
        assert_eq!(outcome, CaptureOutcome::Stopped);
        assert_eq!(attempts, CONFIGURATIONS.len());
    }

    #[test]
    fn stopping_during_restore_failure_does_not_report_unavailable() {
        let active = Cell::new(true);
        let mut attempts = 0;
        let outcome = run_capture(
            || active.get(),
            |_| {
                attempts += 1;
                if attempts == 1 {
                    CaptureAttempt::NoWaveform
                } else {
                    if attempts > CONFIGURATIONS.len() {
                        active.set(false);
                    }
                    CaptureAttempt::Retry
                }
            },
        );
        assert_eq!(outcome, CaptureOutcome::Stopped);
        assert_eq!(attempts, CONFIGURATIONS.len() + 1);
    }
}
