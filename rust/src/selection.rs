//! User-driven target changes and their recording intent.
//! Kept independent of JNI so locking a string cannot silently become a stop.
use crate::tuning::STRINGS;
use crate::ui_state::UiState;

#[derive(Clone, Copy)]
pub(crate) struct SelectionContext {
    pub(crate) resumed: bool,
    pub(crate) listening_requested: bool,
    pub(crate) permission_in_flight: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionEffect {
    None,
    StartListening,
}

/// Update the target, then return an intent for the Android adapter to handle
/// after releasing the UI mutex. Starting must use the existing permission and
/// recorder lifecycle path, never create a second microphone owner here.
pub(crate) fn select_target(
    ui: &mut UiState,
    selected: Option<usize>,
    context: SelectionContext,
) -> SelectionEffect {
    if selected.is_some_and(|index| index >= STRINGS.len()) || selected == ui.selected_target {
        return SelectionEffect::None;
    }
    ui.set_target(selected);

    if selected.is_some()
        && context.resumed
        && !context.listening_requested
        && !context.permission_in_flight
    {
        SelectionEffect::StartListening
    } else {
        // Unlocking returns to automatic selection without starting or stopping
        // recording. Switching strings during capture must not toggle capture.
        SelectionEffect::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{PitchAnalyzer, PitchReading};
    use crate::ui_state::UiPhase;
    use std::f32::consts::TAU;

    fn context(listening_requested: bool) -> SelectionContext {
        SelectionContext {
            resumed: true,
            listening_requested,
            permission_in_flight: false,
        }
    }

    fn reading(frequency_hz: f32) -> PitchReading {
        PitchReading {
            frequency_hz,
            confidence: 0.95,
            level: 0.2,
            harmonic_support: 0.2,
            subharmonic_support: 0.0,
        }
    }

    #[test]
    fn locking_each_string_from_idle_requests_capture() {
        for index in 0..STRINGS.len() {
            let mut ui = UiState::new();
            assert_eq!(
                select_target(&mut ui, Some(index), context(false)),
                SelectionEffect::StartListening
            );
            assert_eq!(ui.selected_target, Some(index));
            // Producing an intent is not proof of recording. Until the adapter
            // starts capture, idle text must not claim the microphone is live.
            assert!(ui.phase == UiPhase::Ready);
            assert!(!ui.keep_screen_on);
        }
    }

    #[test]
    fn locking_after_stop_or_microphone_error_requests_a_retry() {
        for microphone_error in [false, true] {
            let mut ui = UiState::new();
            if microphone_error {
                ui.show_microphone_error();
            } else {
                ui.show_stopped();
            }
            assert_eq!(
                select_target(&mut ui, Some(1), context(false)),
                SelectionEffect::StartListening
            );
            assert!(!ui.keep_screen_on);
            assert!(ui.needle_cents.is_none());
        }
    }

    #[test]
    fn permission_needed_produces_an_intent_without_claiming_recording() {
        let mut ui = UiState::new();
        ui.show_permission_needed(true, false);
        let status = ui.status.clone();
        let button = ui.button.clone();
        assert_eq!(
            select_target(&mut ui, Some(4), context(false)),
            SelectionEffect::StartListening
        );
        assert!(ui.phase == UiPhase::Permission);
        assert_eq!(ui.status, status);
        assert_eq!(ui.button, button);
        assert!(!ui.keep_screen_on);
        // The adapter reuses handle_action, which requests permission/settings.
        // Only its successful permission result may call show_listening.
        ui.show_listening();
        assert_eq!(ui.selected_target, Some(4));
        assert!(ui.phase == UiPhase::AwaitingSound);
    }

    #[test]
    fn pending_permission_and_background_do_not_request_capture() {
        for resumed in [false, true] {
            for listening_requested in [false, true] {
                for permission_in_flight in [false, true] {
                    let mut ui = UiState::new();
                    let effect = select_target(
                        &mut ui,
                        Some(0),
                        SelectionContext {
                            resumed,
                            listening_requested,
                            permission_in_flight,
                        },
                    );
                    assert_eq!(
                        effect == SelectionEffect::StartListening,
                        resumed && !listening_requested && !permission_in_flight
                    );
                    assert_eq!(ui.selected_target, Some(0));
                }
            }
        }
    }

    #[test]
    fn switching_during_capture_never_emits_a_start_or_stop_toggle() {
        let mut ui = UiState::new();
        ui.show_listening();
        let button = ui.button.clone();
        for index in 0..STRINGS.len() {
            assert_eq!(
                select_target(&mut ui, Some(index), context(true)),
                SelectionEffect::None
            );
            assert_eq!(ui.button, button);
            assert!(ui.keep_screen_on);
            assert!(ui.phase == UiPhase::AwaitingSound);
            assert_eq!(ui.active_target, Some(index));
        }
    }

    #[test]
    fn switching_resets_old_frequency_needle_and_tuned_confirmation() {
        let mut ui = UiState::new();
        ui.show_listening();
        select_target(&mut ui, Some(0), context(true));
        for _ in 0..3 {
            ui.apply_reading(Some(reading(STRINGS[0].frequency_hz)));
        }
        assert!(ui.in_tune);
        select_target(&mut ui, Some(1), context(true));
        assert!(ui.phase == UiPhase::AwaitingSound);
        assert!(ui.smoothed_frequency_hz.is_none());
        assert!(ui.needle_cents.is_none());
        assert!(ui.last_reading_at.is_none());
        assert!(ui.last_target.is_none());
        assert_eq!(ui.tuned_candidate_frames, 0);
        assert!(!ui.in_tune);
        ui.apply_reading(Some(reading(STRINGS[1].frequency_hz)));
        assert_eq!(ui.tuned_candidate_frames, 1);
        assert!(!ui.in_tune);
        assert_eq!(ui.active_target, Some(1));
    }

    #[test]
    fn duplicate_selection_does_not_reset_a_reading_or_emit_an_intent() {
        let mut ui = UiState::new();
        select_target(&mut ui, Some(2), context(false));
        ui.show_listening();
        ui.apply_reading(Some(reading(STRINGS[2].frequency_hz)));
        let revision = ui.revision;
        let frequency = ui.smoothed_frequency_hz;
        assert_eq!(
            select_target(&mut ui, Some(2), context(false)),
            SelectionEffect::None
        );
        assert_eq!(ui.revision, revision);
        assert_eq!(ui.smoothed_frequency_hz, frequency);
    }

    #[test]
    fn unlocking_does_not_start_stopped_capture_or_stop_active_capture() {
        for listening in [false, true] {
            let mut ui = UiState::new();
            ui.set_target(Some(3));
            if listening {
                ui.show_listening();
            } else {
                ui.show_stopped();
            }
            let status = ui.status.clone();
            assert_eq!(
                select_target(&mut ui, None, context(listening)),
                SelectionEffect::None
            );
            assert_eq!(ui.selected_target, None);
            assert_eq!(ui.keep_screen_on, listening);
            if listening {
                assert!(ui.phase == UiPhase::AwaitingSound);
                ui.apply_reading(Some(reading(STRINGS[0].frequency_hz)));
                assert_eq!(ui.active_target, Some(0));
            } else {
                assert!(ui.phase == UiPhase::Stopped);
                assert_eq!(ui.status, status);
            }
        }
    }

    #[test]
    fn invalid_indices_do_not_clear_the_target_or_start_capture() {
        let mut ui = UiState::new();
        ui.set_target(Some(2));
        let revision = ui.revision;
        for invalid in [STRINGS.len(), usize::MAX] {
            assert_eq!(
                select_target(&mut ui, Some(invalid), context(false)),
                SelectionEffect::None
            );
            assert_eq!(ui.selected_target, Some(2));
            assert_eq!(ui.revision, revision);
        }
    }

    #[test]
    fn inactive_phase_messages_survive_target_changes() {
        let mut ui = UiState::new();
        ui.show_paused();
        let status = ui.status.clone();
        select_target(
            &mut ui,
            Some(0),
            SelectionContext {
                resumed: false,
                ..context(false)
            },
        );
        assert!(ui.phase == UiPhase::Paused);
        assert_eq!(ui.status, status);
        assert!(!ui.keep_screen_on);

        ui.show_permission_needed(false, true);
        let status = ui.status.clone();
        select_target(
            &mut ui,
            Some(1),
            SelectionContext {
                permission_in_flight: true,
                ..context(true)
            },
        );
        assert!(ui.phase == UiPhase::Permission);
        assert_eq!(ui.status, status);
        assert!(!ui.keep_screen_on);
    }

    #[test]
    fn real_pcm_reaches_locked_string_readings_at_both_capture_rates() {
        for rate in [44_100.0, 48_000.0] {
            for (index, target) in STRINGS.iter().enumerate() {
                let mut ui = UiState::new();
                assert_eq!(
                    select_target(&mut ui, Some(index), context(false)),
                    SelectionEffect::StartListening
                );
                // Model the authorized Android adapter's start transition, then
                // run actual production PCM analysis and UI classification.
                ui.show_listening();
                let mut analyzer = PitchAnalyzer::default();
                for frame in 0..3 {
                    let samples: Vec<i16> = (0..4096)
                        .map(|sample| {
                            let time = (frame * 4096 + sample) as f32 / rate;
                            ((TAU * target.frequency_hz * time).sin() * 0.42 * i16::MAX as f32)
                                as i16
                        })
                        .collect();
                    let reading = analyzer.analyze_pcm(&samples, rate);
                    assert!(
                        reading.is_some(),
                        "no PCM pitch for {} at {rate}",
                        target.note
                    );
                    ui.apply_reading(reading);
                    assert!(ui.phase == UiPhase::Reading);
                    assert_eq!(ui.selected_target, Some(index));
                    assert_eq!(ui.active_target, Some(index));
                    assert!(ui.needle_cents.is_some());
                }
                assert!(ui.in_tune, "{} at {rate} did not settle", target.note);
            }
        }
    }

    #[test]
    fn locked_targets_still_show_flat_and_sharp_without_octave_folding() {
        for (index, target) in STRINGS.iter().enumerate() {
            for cents in [-25.0_f32, 25.0] {
                let mut ui = UiState::new();
                ui.set_target(Some(index));
                ui.show_listening();
                let frequency = target.frequency_hz * 2.0_f32.powf(cents / 1200.0);
                ui.apply_reading(Some(reading(frequency)));
                let actual = ui
                    .needle_cents
                    .expect("locked target did not produce a needle");
                assert!((actual - cents).abs() < 0.1);
                assert_eq!(ui.selected_target, Some(index));
                assert!(!ui.in_tune);
            }
        }
    }

    #[test]
    fn a_different_string_is_not_marked_tuned_for_the_locked_target() {
        let mut ui = UiState::new();
        ui.set_target(Some(0));
        ui.show_listening();
        for _ in 0..3 {
            ui.apply_reading(Some(reading(STRINGS[5].frequency_hz)));
        }
        assert_eq!(ui.selected_target, Some(0));
        assert!(ui.needle_cents.is_none());
        assert!(!ui.in_tune);
        assert!(ui.phase == UiPhase::AwaitingSound);
        assert_eq!(ui.active_target, Some(0));
    }
}
