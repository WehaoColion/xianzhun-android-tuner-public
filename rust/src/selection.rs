//! Selection is a pure target mutation. Recording has its own explicit action.
use crate::tuning::STRINGS;
use crate::ui_state::UiState;

pub(crate) fn select_target(ui: &mut UiState, selected: Option<usize>) {
    if selected.is_some_and(|index| index >= STRINGS.len()) {
        return;
    }
    ui.set_target(selected);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{PitchAnalyzer, PitchReading};
    use crate::ui_state::{UiPhase, TUNED_CONFIRMATION_FRAMES};
    use std::f32::consts::TAU;

    fn reading(frequency_hz: f32) -> PitchReading {
        PitchReading { frequency_hz, confidence: 0.95, level: 0.2,
            harmonic_support: 0.2, subharmonic_support: 0.0 }
    }

    #[test]
    fn selection_never_starts_stops_or_requests_recording() {
        for phase in [UiPhase::Ready, UiPhase::Stopped, UiPhase::Paused,
            UiPhase::Permission, UiPhase::MicrophoneError, UiPhase::AwaitingSound] {
            let mut ui = UiState::new();
            match phase {
                UiPhase::Stopped => ui.show_stopped(),
                UiPhase::Paused => ui.show_paused(),
                UiPhase::Permission => ui.show_permission_needed(false, true),
                UiPhase::MicrophoneError => ui.show_microphone_error(),
                UiPhase::AwaitingSound => ui.show_listening(),
                _ => {}
            }
            let button = ui.button.clone();
            let keep = ui.keep_screen_on;
            for index in 0..STRINGS.len() {
                select_target(&mut ui, Some(index));
                assert!(ui.phase == phase);
                assert_eq!(ui.button, button);
                assert_eq!(ui.keep_screen_on, keep);
                assert_eq!(ui.selected_target, Some(index));
            }
            select_target(&mut ui, None);
            assert!(ui.phase == phase);
            assert_eq!(ui.button, button);
        }
    }

    #[test]
    fn selecting_same_target_is_idempotent_and_switch_invalidates_old_samples() {
        let mut ui = UiState::new();
        ui.show_listening();
        select_target(&mut ui, Some(0));
        for _ in 0..TUNED_CONFIRMATION_FRAMES {
            ui.apply_reading(Some(reading(STRINGS[0].frequency_hz)));
        }
        assert!(ui.in_tune);
        let epoch = ui.target_epoch;
        let revision = ui.revision;
        select_target(&mut ui, Some(0));
        select_target(&mut ui, Some(usize::MAX));
        assert_eq!(ui.target_epoch, epoch);
        assert_eq!(ui.revision, revision);
        assert!(ui.in_tune);
        select_target(&mut ui, Some(1));
        assert_ne!(ui.target_epoch, epoch);
        assert!(ui.phase == UiPhase::AwaitingSound);
        assert!(ui.needle_cents.is_none());
        assert!(ui.last_reading_at.is_none());
        assert!(!ui.in_tune);
        select_target(&mut ui, None);
        assert!(ui.phase == UiPhase::AwaitingSound);
        assert!(ui.selected_target.is_none());
    }

    #[test]
    fn real_pcm_reaches_locked_string_readings_at_both_capture_rates() {
        for rate in [44_100.0, 48_000.0] {
            for (index, target) in STRINGS.iter().enumerate() {
                let mut ui = UiState::new();
                select_target(&mut ui, Some(index));
                // Model the authorized Android adapter's start transition, then
                // run actual production PCM analysis and UI classification.
                ui.show_listening();
                let mut analyzer = PitchAnalyzer::default();
                for frame in 0..TUNED_CONFIRMATION_FRAMES as usize {
                    let samples: Vec<i16> = (0..4096)
                        .map(|sample| {
                            let time = (frame * 2048 + sample) as f32 / rate;
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
