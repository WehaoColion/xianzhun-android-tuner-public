use crate::pitch::PitchReading;

pub const MAX_AUTO_DISTANCE_CENTS: f32 = 210.0;
const MAX_TRACKED_DISTANCE_CENTS: f32 = 260.0;
const MAX_SELECTED_DISTANCE_CENTS: f32 = 220.0;
const TARGET_HYSTERESIS_CENTS: f32 = 35.0;
const HARMONIC_COLLISION_CENTS: f32 = 35.0;
const MAX_COLLISION_SUBHARMONIC_SUPPORT: f32 = 0.03;
// Capture samples are normalized before RMS is calculated.  The small allowance
// above 1.0 covers bounded FIR overshoot without accepting impossible metadata.
const MAX_READING_LEVEL: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetString {
    pub note: &'static str,
    pub description: &'static str,
    pub frequency_hz: f32,
}

pub const STRINGS: [TargetString; 6] = [
    TargetString {
        note: "E2",
        description: "6弦 低音",
        frequency_hz: 82.41,
    },
    TargetString {
        note: "A2",
        description: "5弦",
        frequency_hz: 110.00,
    },
    TargetString {
        note: "D3",
        description: "4弦",
        frequency_hz: 146.83,
    },
    TargetString {
        note: "G3",
        description: "3弦",
        frequency_hz: 196.00,
    },
    TargetString {
        note: "B3",
        description: "2弦",
        frequency_hz: 246.94,
    },
    TargetString {
        note: "E4",
        description: "1弦 高音",
        frequency_hz: 329.63,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TuningEstimate {
    pub target_index: usize,
    pub frequency_hz: f32,
    pub cents: f32,
}

pub fn cents_between(frequency_hz: f32, target_hz: f32) -> f32 {
    1200.0 * (frequency_hz / target_hz).log2()
}

#[cfg(test)]
pub fn classify_frequency(
    frequency_hz: f32,
    selected_target: Option<usize>,
    previous_target: Option<usize>,
) -> Option<TuningEstimate> {
    classify_with_evidence(frequency_hz, selected_target, previous_target, None)
}

pub fn classify_reading(
    reading: &PitchReading,
    selected_target: Option<usize>,
    previous_target: Option<usize>,
) -> Option<TuningEstimate> {
    if !reading_is_valid(reading) {
        return None;
    }
    classify_with_evidence(
        reading.frequency_hz,
        selected_target,
        previous_target,
        Some(reading.subharmonic_support),
    )
}

fn classify_with_evidence(
    frequency_hz: f32,
    selected_target: Option<usize>,
    previous_target: Option<usize>,
    subharmonic_support: Option<f32>,
) -> Option<TuningEstimate> {
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 {
        return None;
    }

    let estimate = if let Some(index) = selected_target {
        if index >= STRINGS.len() {
            return None;
        }
        classify_selected(frequency_hz, index)?
    } else {
        classify_automatic(frequency_hz, previous_target)?
    };

    if lower_string_harmonic_collision(frequency_hz, estimate.target_index) {
        // B3/E4 can coincide with loud lower-string partials.  This gate must
        // apply equally to automatic and manually selected targets.
        let has_lower_string_evidence = subharmonic_support
            .map(|support| support > MAX_COLLISION_SUBHARMONIC_SUPPORT)
            .unwrap_or(true);
        if has_lower_string_evidence {
            return None;
        }
    }

    Some(estimate)
}

fn reading_is_valid(reading: &PitchReading) -> bool {
    reading.frequency_hz.is_finite()
        && (crate::pitch::MIN_FREQ_HZ..=crate::pitch::MAX_FREQ_HZ).contains(&reading.frequency_hz)
        && reading.confidence.is_finite()
        && (crate::pitch::MIN_CLARITY..=1.0).contains(&reading.confidence)
        && reading.level.is_finite()
        && (crate::pitch::MIN_RMS..=MAX_READING_LEVEL).contains(&reading.level)
        && reading.harmonic_support.is_finite()
        && (0.0..=1.0).contains(&reading.harmonic_support)
        && reading.subharmonic_support.is_finite()
        && (0.0..=1.0).contains(&reading.subharmonic_support)
}

fn classify_automatic(frequency_hz: f32, previous_target: Option<usize>) -> Option<TuningEstimate> {
    let (nearest_index, nearest_cents) = STRINGS
        .iter()
        .enumerate()
        .map(|(index, target)| (index, cents_between(frequency_hz, target.frequency_hz)))
        .min_by(|left, right| left.1.abs().total_cmp(&right.1.abs()))?;

    let retained_target = previous_target
        .filter(|index| *index < STRINGS.len())
        .filter(|index| {
            let previous_cents = cents_between(frequency_hz, STRINGS[*index].frequency_hz);
            previous_cents.abs() <= MAX_TRACKED_DISTANCE_CENTS
                && previous_cents.abs() <= nearest_cents.abs() + TARGET_HYSTERESIS_CENTS
        });
    let target_index = match retained_target {
        Some(index) => index,
        None if nearest_cents.abs() <= MAX_AUTO_DISTANCE_CENTS => nearest_index,
        None => return None,
    };
    let cents = cents_between(frequency_hz, STRINGS[target_index].frequency_hz);

    Some(TuningEstimate {
        target_index,
        frequency_hz,
        cents,
    })
}

fn classify_selected(frequency_hz: f32, target_index: usize) -> Option<TuningEstimate> {
    let target_hz = STRINGS[target_index].frequency_hz;
    let cents = cents_between(frequency_hz, target_hz);
    if cents.abs() > MAX_SELECTED_DISTANCE_CENTS {
        return None;
    }

    Some(TuningEstimate {
        target_index,
        frequency_hz,
        cents,
    })
}

fn lower_string_harmonic_collision(frequency_hz: f32, target_index: usize) -> bool {
    STRINGS[..target_index].iter().any(|lower_string| {
        (2_u8..=4).any(|order| {
            let harmonic_hz = lower_string.frequency_hz * order as f32;
            cents_between(frequency_hz, harmonic_hz).abs() <= HARMONIC_COLLISION_CENTS
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::analyze_pcm;
    use std::f32::consts::TAU;

    const SAMPLE_RATES: [f32; 2] = [44_100.0, 48_000.0];
    const PRODUCTION_FRAME_SIZE: usize = 4096;

    #[test]
    fn standard_open_strings_map_without_octave_folding() {
        for (index, target) in STRINGS.iter().enumerate() {
            let reading = reading_with_support(target.frequency_hz, 0.25);
            let result = classify_reading(&reading, None, None).unwrap();
            assert_eq!(result.target_index, index);
            assert!(result.cents.abs() < 0.01);
        }
    }

    #[test]
    fn production_analyzer_and_classifier_recognize_all_open_strings() {
        for sample_rate in SAMPLE_RATES {
            for phase_offset in [0.0, 0.37] {
                for (expected_index, target) in STRINGS.iter().enumerate() {
                    let samples = sine_wave(
                        target.frequency_hz,
                        0.42,
                        phase_offset,
                        PRODUCTION_FRAME_SIZE,
                        sample_rate,
                    );
                    let reading = analyze_pcm(&samples, sample_rate).unwrap_or_else(|| {
                        panic!(
                            "analyzer missed {} at {sample_rate} Hz and phase {phase_offset}",
                            target.note
                        )
                    });
                    let estimate = classify_reading(&reading, None, None).unwrap_or_else(|| {
                        panic!(
                            "classifier rejected {} at {sample_rate} Hz: {reading:?}",
                            target.note
                        )
                    });
                    assert_eq!(estimate.target_index, expected_index);
                    assert!((estimate.frequency_hz - target.frequency_hz).abs() < 0.8);

                    let selected = classify_reading(&reading, Some(expected_index), None)
                        .unwrap_or_else(|| {
                            panic!(
                                "selected classifier rejected {} at {sample_rate} Hz: {reading:?}",
                                target.note
                            )
                        });
                    assert_eq!(selected.target_index, expected_index);
                }
            }
        }
    }

    #[test]
    fn production_lower_string_partials_are_rejected_in_auto_and_selected_modes() {
        for sample_rate in SAMPLE_RATES {
            for phase_offset in [0.0, 0.37] {
                for (fundamental, candidate_order, selected_target) in
                    [(82.41, 3, 4), (110.0, 3, 5), (82.41, 4, 5)]
                {
                    let candidate_hz = fundamental * candidate_order as f32;
                    let samples = lower_string_partial_wave(
                        fundamental,
                        candidate_order,
                        phase_offset,
                        PRODUCTION_FRAME_SIZE,
                        sample_rate,
                    );
                    let reading = analyze_pcm(&samples, sample_rate).unwrap_or_else(|| {
                        panic!(
                            "analyzer missed lower-string candidate {candidate_hz} Hz at \
                             {sample_rate} Hz and phase {phase_offset}"
                        )
                    });
                    assert!(
                        (reading.frequency_hz - candidate_hz).abs() < 0.8,
                        "unexpected candidate reading: {reading:?}"
                    );
                    assert!(
                        reading.subharmonic_support > MAX_COLLISION_SUBHARMONIC_SUPPORT,
                        "test signal lacks lower-string evidence: {reading:?}"
                    );
                    assert!(classify_reading(&reading, None, None).is_none());
                    assert!(
                        classify_reading(&reading, Some(selected_target), None).is_none(),
                        "selected {} accepted lower-string evidence: {reading:?}",
                        STRINGS[selected_target].note
                    );
                }
            }
        }
    }

    #[test]
    fn ordinary_non_guitar_frequency_is_not_doubled() {
        let result = classify_frequency(100.0, None, None).unwrap();
        assert_eq!(result.target_index, 1, "100 Hz should remain near A2");
        assert!((result.frequency_hz - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn automatic_mode_rejects_frequency_below_useful_capture_range() {
        assert!(classify_frequency(60.0, None, None).is_none());
    }

    #[test]
    fn harmonic_collisions_require_positive_lower_string_evidence() {
        for (frequency, expected_target) in [
            (164.82, 2),
            (220.0, 3),
            (247.23, 4),
            (329.64, 5),
            (330.0, 5),
        ] {
            assert!(
                classify_frequency(frequency, None, None).is_none(),
                "{frequency} Hz is ambiguous without overtone evidence"
            );

            let reading = reading_with_support(frequency, 0.25);
            let result = classify_reading(&reading, None, None).unwrap();
            assert_eq!(result.target_index, expected_target);
        }

        for (target_index, real_frequency, impostor_frequency) in [
            (2, 146.83, 164.82),
            (3, 196.0, 220.0),
            (4, 246.94, 247.23),
            (5, 329.63, 329.64),
            (5, 329.63, 330.0),
        ] {
            let real_string = reading_with_support(real_frequency, 0.25);
            let locked = classify_reading(&real_string, None, None).unwrap();
            assert_eq!(locked.target_index, target_index);

            let mut lower_string_partial = reading_with_support(impostor_frequency, 0.25);
            lower_string_partial.subharmonic_support = 0.08;
            assert!(
                classify_reading(&lower_string_partial, None, Some(locked.target_index)).is_none()
            );

            let isolated_reference_tone = reading_with_support(impostor_frequency, 0.0);
            assert!(
                classify_reading(&isolated_reference_tone, None, Some(locked.target_index))
                    .is_some()
            );
        }
    }

    #[test]
    fn hysteresis_prevents_target_flicker_at_an_adjacent_string_boundary() {
        let boundary = (STRINGS[1].frequency_hz * STRINGS[2].frequency_hz).sqrt();
        let keep_a = classify_frequency(boundary * 1.004, None, Some(1)).unwrap();
        let keep_d = classify_frequency(boundary / 1.004, None, Some(2)).unwrap();
        assert_eq!(keep_a.target_index, 1);
        assert_eq!(keep_d.target_index, 2);
    }

    #[test]
    fn selected_string_mode_rejects_other_strings_and_integer_multiples() {
        for (selected_index, _) in STRINGS.iter().enumerate() {
            for (input_index, input) in STRINGS.iter().enumerate() {
                if input_index != selected_index {
                    assert!(
                        classify_frequency(input.frequency_hz, Some(selected_index), None)
                            .is_none(),
                        "{} must not be accepted while {} is selected",
                        input.note,
                        STRINGS[selected_index].note
                    );
                }
            }
        }

        for harmonic_hz in [164.82, 247.23, 329.64] {
            assert!(classify_frequency(harmonic_hz, Some(0), None).is_none());
        }
    }

    #[test]
    fn selected_string_mode_accepts_only_a_direct_useful_tuning_window() {
        let target = STRINGS[1].frequency_hz;
        for cents in [-200.0_f32, 0.0, 200.0] {
            let frequency = target * 2.0_f32.powf(cents / 1200.0);
            let result = classify_frequency(frequency, Some(1), None).unwrap();
            assert_eq!(result.target_index, 1);
            assert!((result.cents - cents).abs() < 0.01);
        }

        for cents in [-230.0_f32, 230.0] {
            let frequency = target * 2.0_f32.powf(cents / 1200.0);
            assert!(classify_frequency(frequency, Some(1), None).is_none());
        }
    }

    #[test]
    fn automatic_mode_rejects_adjacent_string_dead_zones_instead_of_guessing() {
        for adjacent in STRINGS.windows(2) {
            let boundary = (adjacent[0].frequency_hz * adjacent[1].frequency_hz).sqrt();
            assert!(classify_frequency(boundary, None, None).is_none());
        }
        assert!(classify_frequency(128.0, None, None).is_none());
    }

    #[test]
    fn invalid_selected_index_fails_closed() {
        assert!(classify_frequency(110.0, Some(STRINGS.len()), None).is_none());
        assert!(classify_frequency(110.0, Some(usize::MAX), None).is_none());
    }

    #[test]
    fn non_finite_or_out_of_range_reading_metadata_fails_closed() {
        let invalid_values = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        for value in invalid_values {
            let mut reading = reading_with_support(110.0, 0.25);
            reading.frequency_hz = value;
            assert!(classify_reading(&reading, None, None).is_none());

            let mut reading = reading_with_support(110.0, 0.25);
            reading.confidence = value;
            assert!(classify_reading(&reading, None, None).is_none());

            let mut reading = reading_with_support(110.0, 0.25);
            reading.level = value;
            assert!(classify_reading(&reading, None, None).is_none());

            let mut reading = reading_with_support(246.94, 0.25);
            reading.harmonic_support = value;
            assert!(classify_reading(&reading, None, None).is_none());

            let mut reading = reading_with_support(246.94, 0.25);
            reading.subharmonic_support = value;
            assert!(classify_reading(&reading, None, None).is_none());
        }

        for invalid_confidence in [-0.01, 1.01] {
            let mut reading = reading_with_support(110.0, 0.25);
            reading.confidence = invalid_confidence;
            assert!(classify_reading(&reading, None, None).is_none());
        }
        for invalid_support in [-0.01, 1.01] {
            let reading = reading_with_support(246.94, invalid_support);
            assert!(classify_reading(&reading, None, None).is_none());

            let mut reading = reading_with_support(246.94, 0.25);
            reading.subharmonic_support = invalid_support;
            assert!(classify_reading(&reading, None, None).is_none());
        }

        for invalid_level in [-0.01, MAX_READING_LEVEL + 0.01, f32::MAX] {
            let mut reading = reading_with_support(110.0, 0.25);
            reading.level = invalid_level;
            assert!(classify_reading(&reading, None, None).is_none());
        }
    }

    #[test]
    fn silent_or_unclear_readings_cannot_change_the_target_in_auto_or_selected_mode() {
        for selected in [None, Some(1)] {
            for previous in [None, Some(1)] {
                for confidence in [0.0, crate::pitch::MIN_CLARITY - 0.001] {
                    let mut reading = reading_with_support(110.0, 0.25);
                    reading.confidence = confidence;
                    assert!(classify_reading(&reading, selected, previous).is_none());
                }
                for level in [0.0, crate::pitch::MIN_RMS - 0.001] {
                    let mut reading = reading_with_support(110.0, 0.25);
                    reading.level = level;
                    assert!(classify_reading(&reading, selected, previous).is_none());
                }
                let mut reading = reading_with_support(110.0, 0.25);
                reading.level = crate::pitch::MIN_RMS;
                reading.confidence = crate::pitch::MIN_CLARITY;
                assert!(classify_reading(&reading, selected, previous).is_some());
            }
        }
    }

    fn reading_with_support(frequency_hz: f32, harmonic_support: f32) -> PitchReading {
        PitchReading {
            frequency_hz,
            confidence: 0.95,
            level: 0.2,
            harmonic_support,
            subharmonic_support: 0.0,
        }
    }

    fn sine_wave(
        frequency: f32,
        amplitude: f32,
        phase_offset: f32,
        len: usize,
        sample_rate: f32,
    ) -> Vec<i16> {
        (0..len)
            .map(|index| {
                let phase = TAU * frequency * index as f32 / sample_rate + phase_offset;
                (phase.sin() * amplitude * i16::MAX as f32) as i16
            })
            .collect()
    }

    fn lower_string_partial_wave(
        fundamental: f32,
        candidate_order: usize,
        phase_offset: f32,
        len: usize,
        sample_rate: f32,
    ) -> Vec<i16> {
        (0..len)
            .map(|index| {
                let phase = TAU * fundamental * index as f32 / sample_rate + phase_offset;
                let candidate_phase = candidate_order as f32 * phase;
                let sample = 0.001 * phase.sin()
                    + 0.03 * (2.0 * phase).sin()
                    + 0.45 * candidate_phase.sin()
                    + 0.18 * (2.0 * candidate_phase).sin();
                (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            })
            .collect()
    }
}
