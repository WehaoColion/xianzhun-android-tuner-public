pub const MIN_FREQ_HZ: f32 = 60.0;
pub const MAX_FREQ_HZ: f32 = 380.0;
pub const MIN_CLARITY: f32 = 0.56;
// Keep quiet periodic input above a floor of 32 PCM16 counts RMS (about
// -60.2 dBFS). This is still far above one-count quantization noise; loudness
// alone never establishes a pitch, which must also pass the period, clarity,
// and frequency checks below. The former 0.006 floor discarded quiet strings.
pub const MIN_RMS: f32 = 32.0 / i16::MAX as f32;

const YIN_THRESHOLD: f32 = 0.12;
const MAX_ALIAS_CHECK_FREQ_HZ: f32 = 1_200.0;
const DOWNSAMPLE_FACTOR: usize = 4;
const LOW_PASS_TAPS: usize = 63;
const LOW_PASS_CUTOFF_HZ: f32 = 1_600.0;
const MIN_INPUT_SAMPLES: usize = 2048;
const MIN_SAMPLE_RATE_HZ: f32 = 16_000.0;
const MAX_SAMPLE_RATE_HZ: f32 = 96_000.0;

#[derive(Clone, Copy, Debug)]
pub struct PitchReading {
    pub frequency_hz: f32,
    pub confidence: f32,
    pub level: f32,
    /// Share of the explained spectral energy carried by harmonics 2 through 4.
    /// This remains diagnostic evidence for the captured tone; lower-string
    /// collision rejection is driven by `subharmonic_support` below.
    pub harmonic_support: f32,
    /// Strongest amplitude ratio on the low-order partial lattice below the
    /// reported pitch (1/2, 1/3, 2/3, 1/4, and 3/4).  This exposes a weak lower
    /// fundamental or its second/third partial when a loud upper partial looks
    /// like a different open string.
    pub subharmonic_support: f32,
}

#[cfg(test)]
pub fn analyze_pcm(samples: &[i16], sample_rate: f32) -> Option<PitchReading> {
    PitchAnalyzer::default().analyze_pcm(samples, sample_rate)
}

#[derive(Default)]
pub struct PitchAnalyzer {
    signal: Vec<f32>,
    difference: Vec<f32>,
    cmnd: Vec<f32>,
    low_pass_sample_rate: f32,
    low_pass_coefficients: Vec<f32>,
    hann_window: Vec<f32>,
    windowed_signal: Vec<f32>,
}

impl PitchAnalyzer {
    pub fn analyze_pcm(&mut self, samples: &[i16], sample_rate: f32) -> Option<PitchReading> {
        if samples.len() < MIN_INPUT_SAMPLES
            || !sample_rate.is_finite()
            || !(MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&sample_rate)
        {
            return None;
        }

        if self.low_pass_sample_rate != sample_rate {
            low_pass_coefficients_into(sample_rate, &mut self.low_pass_coefficients);
            self.low_pass_sample_rate = sample_rate;
        }
        filter_and_downsample_into(
            samples,
            &self.low_pass_coefficients,
            DOWNSAMPLE_FACTOR,
            &mut self.signal,
        );
        remove_dc_offset(&mut self.signal);

        let level = rms_level(&self.signal);
        if level < MIN_RMS {
            return None;
        }

        let analysis_rate = sample_rate / DOWNSAMPLE_FACTOR as f32;
        let min_tau = (analysis_rate / MAX_FREQ_HZ).floor().max(2.0) as usize;
        let alias_check_min_tau =
            (analysis_rate / MAX_ALIAS_CHECK_FREQ_HZ).floor().max(2.0) as usize;
        let max_tau = (analysis_rate / MIN_FREQ_HZ).ceil() as usize;
        let upper_tau = max_tau.min(self.signal.len() / 2);
        if min_tau >= upper_tau || upper_tau - min_tau <= 2 {
            return None;
        }

        // The right-hand neighbor is needed even for the lowest supported
        // pitch. Without it, a falling edge at the search boundary is reported
        // as exactly MIN_FREQ_HZ, including tones below the capture range.
        yin_difference_into(&self.signal, upper_tau + 1, &mut self.difference);
        cumulative_mean_normalized_difference_into(&self.difference, &mut self.cmnd);
        let tau = find_best_period(&self.cmnd, alias_check_min_tau, upper_tau)?;
        if tau == upper_tau && self.cmnd[tau + 1] < self.cmnd[tau] {
            // A short high-rate frame may end before the real minimum. An
            // unfinished trough is not a period and cannot be interpolated
            // into a reliable pitch, even when its confidence appears high.
            return None;
        }
        if tau < min_tau {
            // The strongest valid period belongs to a tone above the tuner's
            // capture range.  Starting the search at `min_tau` would silently
            // choose its second/third period and report a false subharmonic.
            return None;
        }
        let confidence = (1.0 - self.cmnd[tau]).clamp(0.0, 1.0);
        if confidence < MIN_CLARITY {
            return None;
        }

        let refined_tau = parabolic_interpolation(&self.cmnd, tau);
        if refined_tau <= 0.0 || !refined_tau.is_finite() {
            return None;
        }

        let frequency_hz = analysis_rate / refined_tau;
        if !frequency_hz.is_finite() || !(MIN_FREQ_HZ..=MAX_FREQ_HZ).contains(&frequency_hz) {
            return None;
        }
        window_signal_into(
            &self.signal,
            &mut self.hann_window,
            &mut self.windowed_signal,
        );
        let (harmonic_support, subharmonic_support) =
            estimate_spectral_support(&self.windowed_signal, analysis_rate, frequency_hz);

        Some(PitchReading {
            frequency_hz,
            confidence,
            level,
            harmonic_support,
            subharmonic_support,
        })
    }
}

fn low_pass_coefficients_into(sample_rate: f32, coefficients: &mut Vec<f32>) {
    coefficients.clear();
    coefficients.resize(LOW_PASS_TAPS, 0.0);
    let half = (LOW_PASS_TAPS / 2) as isize;
    let normalized_cutoff = LOW_PASS_CUTOFF_HZ / sample_rate;
    let mut coefficient_sum = 0.0;

    for (index, coefficient) in coefficients.iter_mut().enumerate() {
        let offset = index as isize - half;
        let ideal = if offset == 0 {
            2.0 * normalized_cutoff
        } else {
            let offset = offset as f32;
            (std::f32::consts::TAU * normalized_cutoff * offset).sin()
                / (std::f32::consts::PI * offset)
        };
        let phase = std::f32::consts::TAU * index as f32 / (LOW_PASS_TAPS - 1) as f32;
        let blackman = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
        *coefficient = ideal * blackman;
        coefficient_sum += *coefficient;
    }

    for coefficient in coefficients {
        *coefficient /= coefficient_sum;
    }
}

fn filter_and_downsample_into(
    samples: &[i16],
    coefficients: &[f32],
    factor: usize,
    signal: &mut Vec<f32>,
) {
    signal.clear();
    if factor == 0 || samples.len() < LOW_PASS_TAPS {
        return;
    }

    let half = LOW_PASS_TAPS / 2;
    let end = samples.len() - half;
    signal.reserve((end - half) / factor + 1);
    for center in (half..end).step_by(factor) {
        let start = center - half;
        let filtered = coefficients
            .iter()
            .enumerate()
            .map(|(tap, coefficient)| samples[start + tap] as f32 * coefficient)
            .sum::<f32>();
        signal.push(filtered / i16::MAX as f32);
    }
}

#[cfg(test)]
fn downsample_into(samples: &[i16], sample_rate: f32, factor: usize, signal: &mut Vec<f32>) {
    let mut coefficients = Vec::new();
    low_pass_coefficients_into(sample_rate, &mut coefficients);
    filter_and_downsample_into(samples, &coefficients, factor, signal);
}

fn remove_dc_offset(signal: &mut [f32]) {
    let mean = signal.iter().copied().sum::<f32>() / signal.len() as f32;
    for sample in signal.iter_mut() {
        *sample -= mean;
    }
}

fn rms_level(signal: &[f32]) -> f32 {
    let energy = signal.iter().map(|sample| sample * sample).sum::<f32>() / signal.len() as f32;
    energy.sqrt()
}

fn estimate_spectral_support(
    windowed_signal: &[f32],
    sample_rate: f32,
    fundamental_hz: f32,
) -> (f32, f32) {
    let fundamental_power = tone_power(windowed_signal, sample_rate, fundamental_hz);
    let harmonic_power = (2..=4)
        .map(|order| tone_power(windowed_signal, sample_rate, fundamental_hz * order as f32))
        .sum::<f32>();
    let explained_power = fundamental_power + harmonic_power;
    let harmonic_support = if explained_power <= f32::EPSILON {
        0.0
    } else {
        (harmonic_power / explained_power).clamp(0.0, 1.0)
    };
    let subharmonic_support = if fundamental_power <= f32::EPSILON {
        0.0
    } else {
        // 2/4 duplicates 1/2; analyze each lower partial only once.
        let strongest_lower_power = [0.5_f32, 1.0 / 3.0, 2.0 / 3.0, 0.25, 0.75]
            .into_iter()
            .map(|ratio| tone_power(windowed_signal, sample_rate, fundamental_hz * ratio))
            .fold(0.0_f32, f32::max);
        (strongest_lower_power / fundamental_power)
            .sqrt()
            .clamp(0.0, 1.0)
    };
    (harmonic_support, subharmonic_support)
}

fn window_signal_into(signal: &[f32], window: &mut Vec<f32>, windowed: &mut Vec<f32>) {
    if window.len() != signal.len() {
        window.clear();
        let last_index = (signal.len() - 1) as f32;
        window
            .extend((0..signal.len()).map(|index| {
                0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / last_index).cos()
            }));
    }
    windowed.clear();
    windowed.extend(
        signal
            .iter()
            .zip(window.iter())
            .map(|(sample, weight)| sample * weight),
    );
}

fn tone_power(windowed_signal: &[f32], sample_rate: f32, frequency_hz: f32) -> f32 {
    if windowed_signal.len() < 2
        || !frequency_hz.is_finite()
        || frequency_hz <= 0.0
        || frequency_hz >= sample_rate * 0.5
    {
        return 0.0;
    }

    // Goertzel computes the same single-frequency DFT power with one cosine
    // per bin. f64 state prevents cancellation at low frequencies, where a
    // false lower partial would reject a correctly tuned upper string.
    let angular_step = std::f64::consts::TAU * frequency_hz as f64 / sample_rate as f64;
    let coefficient = 2.0 * angular_step.cos();
    let mut previous = 0.0_f64;
    let mut previous_previous = 0.0_f64;
    for &sample in windowed_signal {
        let current = sample as f64 + coefficient * previous - previous_previous;
        previous_previous = previous;
        previous = current;
    }
    (previous * previous + previous_previous * previous_previous
        - coefficient * previous * previous_previous)
        .max(0.0) as f32
}

fn yin_difference_into(signal: &[f32], upper_tau: usize, difference: &mut Vec<f32>) {
    difference.clear();
    difference.resize(upper_tau + 1, 0.0);
    for tau in 1..=upper_tau {
        let overlap = signal.len() - tau;
        let aligned = overlap - overlap % 4;
        let mut sums = [0.0_f32; 4];
        for index in (0..aligned).step_by(4) {
            for lane in 0..4 {
                let delta = signal[index + lane] - signal[index + lane + tau];
                sums[lane] += delta * delta;
            }
        }
        let mut sum = (sums[0] + sums[1]) + (sums[2] + sums[3]);
        for index in aligned..overlap {
            let delta = signal[index] - signal[index + tau];
            sum += delta * delta;
        }
        difference[tau] = sum;
    }
}

fn cumulative_mean_normalized_difference_into(difference: &[f32], cmnd: &mut Vec<f32>) {
    cmnd.clear();
    cmnd.resize(difference.len(), 1.0);
    let mut running_sum = 0.0;

    for tau in 1..difference.len() {
        running_sum += difference[tau];
        cmnd[tau] = if running_sum > 0.0 {
            difference[tau] * tau as f32 / running_sum
        } else {
            1.0
        };
    }
}

fn find_best_period(cmnd: &[f32], min_tau: usize, upper_tau: usize) -> Option<usize> {
    for tau in min_tau..=upper_tau {
        if cmnd[tau] < YIN_THRESHOLD {
            let mut best_tau = tau;
            while best_tau < upper_tau && cmnd[best_tau + 1] < cmnd[best_tau] {
                best_tau += 1;
            }
            return Some(best_tau);
        }
    }

    let (best_offset, best_value) = cmnd[min_tau..=upper_tau]
        .iter()
        .enumerate()
        .min_by(|left, right| left.1.total_cmp(right.1))?;
    if *best_value > 0.30 {
        return None;
    }

    Some(min_tau + best_offset)
}

fn parabolic_interpolation(values: &[f32], tau: usize) -> f32 {
    if tau == 0 || tau + 1 >= values.len() {
        return tau as f32;
    }

    let left = values[tau - 1];
    let center = values[tau];
    let right = values[tau + 1];
    let denominator = left - (2.0 * center) + right;
    if denominator.abs() < f32::EPSILON {
        return tau as f32;
    }

    let refined = tau as f32 + 0.5 * (left - right) / denominator;
    refined.clamp((tau - 1) as f32, (tau + 1) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SAMPLE_RATE: f32 = 48_000.0;

    #[test]
    fn detects_classical_guitar_open_strings() {
        for expected in [82.41, 110.0, 146.83, 196.0, 246.94, 329.63] {
            let samples = sine_wave(expected, 0.42, 4096);
            let reading = analyze_pcm(&samples, SAMPLE_RATE).unwrap();

            assert!(
                (reading.frequency_hz - expected).abs() < 0.7,
                "expected {expected} Hz, got {} Hz",
                reading.frequency_hz,
            );
            assert!(
                reading.confidence > 0.85,
                "confidence was {}",
                reading.confidence
            );
            assert!(reading.level > MIN_RMS, "level was {}", reading.level);
        }
    }

    #[test]
    fn detects_quiet_open_strings_above_the_pcm_floor() {
        for sample_rate in [44_100.0, 48_000.0] {
            for (target_index, target) in crate::tuning::STRINGS.into_iter().enumerate() {
                let samples = sine_wave_at(target.frequency_hz, 0.0016, 4096, sample_rate);
                let reading = analyze_pcm(&samples, sample_rate).unwrap_or_else(|| {
                    panic!("quiet {} must be detected at {sample_rate} Hz", target.note)
                });
                assert!(
                    (reading.frequency_hz - target.frequency_hz).abs() < 0.7,
                    "quiet {} at {sample_rate} Hz: {reading:?}",
                    target.note
                );
                assert!(reading.confidence > 0.85, "{reading:?}");
                assert!(reading.level >= MIN_RMS && reading.level < 0.006);
                let estimate = crate::tuning::classify_reading(&reading, None, None)
                    .unwrap_or_else(|| panic!("quiet reading must classify: {reading:?}"));
                assert_eq!(estimate.target_index, target_index);
            }
        }
    }

    #[test]
    fn quiet_periodic_input_must_still_cross_the_pcm_rms_floor() {
        for sample_rate in [44_100.0, 48_000.0] {
            for target in crate::tuning::STRINGS {
                for (rms_scale, should_detect) in [(0.8, false), (1.2, true)] {
                    let amplitude = MIN_RMS * std::f32::consts::SQRT_2 * rms_scale;
                    let samples = sine_wave_at(target.frequency_hz, amplitude, 4096, sample_rate);
                    assert_eq!(
                        analyze_pcm(&samples, sample_rate).is_some(),
                        should_detect,
                        "{} at {sample_rate} Hz / {rms_scale} times the RMS floor",
                        target.note
                    );
                }
            }
        }
        assert!(analyze_pcm(&vec![i16::MAX; 4096], SAMPLE_RATE).is_none());
        let quantization_only: Vec<i16> = (0..4096).map(|index| (index % 3) as i16 - 1).collect();
        assert!(analyze_pcm(&quantization_only, SAMPLE_RATE).is_none());
    }

    #[test]
    fn quiet_unpitched_noise_cannot_pass_the_periodicity_gate() {
        for sample_rate in [44_100.0, 48_000.0] {
            for noise_amplitude in [0.02, 0.2] {
                for mut seed in [0x1234_5678_u32, 0x9e37_79b9, 0xa341_316c, 0x51f1_5e5d] {
                    let samples: Vec<i16> = (0..4096)
                        .map(|_| {
                            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            let noise = (seed >> 8) as f32 / 16_777_215.0 - 0.5;
                            (noise * noise_amplitude * i16::MAX as f32) as i16
                        })
                        .collect();
                    let mut filtered = Vec::new();
                    downsample_into(&samples, sample_rate, DOWNSAMPLE_FACTOR, &mut filtered);
                    remove_dc_offset(&mut filtered);
                    assert!(
                        rms_level(&filtered) > MIN_RMS,
                        "noise must exercise periodicity rejection above the volume floor"
                    );
                    assert!(analyze_pcm(&samples, sample_rate).is_none());
                }
            }
        }
    }

    #[test]
    fn rejects_silence() {
        assert!(analyze_pcm(&vec![0; 4096], SAMPLE_RATE).is_none());
    }

    #[test]
    fn rejects_invalid_or_short_input() {
        assert!(analyze_pcm(&[], SAMPLE_RATE).is_none());
        assert!(analyze_pcm(&vec![0; 512], SAMPLE_RATE).is_none());
        let non_silent = sine_wave(110.0, 0.42, 4096);
        for invalid_rate in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MIN,
            -1.0,
            0.0,
            MIN_SAMPLE_RATE_HZ - 1.0,
            MAX_SAMPLE_RATE_HZ + 1.0,
            f32::MAX,
        ] {
            assert!(
                analyze_pcm(&non_silent, invalid_rate).is_none(),
                "sample rate {invalid_rate:?} must fail closed"
            );
        }
    }

    #[test]
    fn rejects_tones_below_the_capture_boundary_instead_of_pinning_them_to_sixty_hz() {
        for sample_rate in [16_000.0, 44_100.0, 48_000.0, 96_000.0] {
            for frequency in [59.0, 59.5, 59.9] {
                let samples = sine_wave_at(frequency, 0.42, 8192, sample_rate);
                assert!(
                    analyze_pcm(&samples, sample_rate).is_none(),
                    "out-of-range {frequency} Hz at {sample_rate} Hz must not become 60 Hz"
                );
            }
            let samples = sine_wave_at(60.2, 0.42, 8192, sample_rate);
            let reading = analyze_pcm(&samples, sample_rate).unwrap();
            assert!((reading.frequency_hz - 60.2).abs() < 0.03, "{reading:?}");
        }
    }

    #[test]
    fn reusable_analyzer_keeps_detuned_strings_accurate_when_rate_and_frame_size_change() {
        let mut analyzer = PitchAnalyzer::default();
        for (sample_rate, len) in [(48_000.0, 4096), (44_100.0, 8192), (48_000.0, 4096)] {
            for target in crate::tuning::STRINGS {
                for cents in [-100.0_f32, -15.0, 0.0, 15.0, 100.0] {
                    let expected = target.frequency_hz * 2.0_f32.powf(cents / 1200.0);
                    let samples = harmonic_rich_pluck(expected, sample_rate, len);
                    let reading = analyzer.analyze_pcm(&samples, sample_rate).unwrap();
                    let error_cents = crate::tuning::cents_between(reading.frequency_hz, expected);
                    assert!(
                        error_cents.abs() < 4.0,
                        "{} at {cents} cents / {sample_rate} Hz / {len} samples: {error_cents} cents",
                        target.note
                    );
                }
            }
        }
    }

    #[test]
    fn short_high_rate_frames_cannot_report_an_unfinished_low_string_period() {
        let expected = crate::tuning::STRINGS[0].frequency_hz;
        for sample_rate in [88_200.0, 96_000.0] {
            let samples: Vec<i16> = (0..4096)
                .map(|index| {
                    let time = index as f32 / sample_rate;
                    let phase = TAU * expected * time + 0.37;
                    let sample = (-2.4 * time).exp()
                        * (0.22 * phase.sin()
                            + 0.11 * (2.0 * phase).sin()
                            + 0.055 * (3.0 * phase).sin());
                    (sample * i16::MAX as f32) as i16
                })
                .collect();
            assert!(
                analyze_pcm(&samples[..2048], sample_rate).is_none(),
                "short {sample_rate} Hz frame must not shift E2 upward"
            );
            let reading = analyze_pcm(&samples, sample_rate).unwrap();
            assert!(crate::tuning::cents_between(reading.frequency_hz, expected).abs() < 1.0);
        }
    }

    #[test]
    fn detects_nylon_string_plucks_with_weak_fundamentals() {
        for expected in [82.41, 110.0, 146.83, 196.0, 246.94, 329.63] {
            let samples = nylon_pluck(expected, 4096);
            let reading = analyze_pcm(&samples, SAMPLE_RATE).unwrap();
            assert!(
                (reading.frequency_hz - expected).abs() < 1.3,
                "expected {expected} Hz, got {} Hz",
                reading.frequency_hz,
            );
        }
    }

    #[test]
    fn rejects_unpitched_noise() {
        let mut seed = 0x1234_5678_u32;
        let samples: Vec<i16> = (0..8192)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (((seed >> 8) as f32 / 16_777_215.0 - 0.5) * 0.35 * i16::MAX as f32) as i16
            })
            .collect();
        assert!(analyze_pcm(&samples, SAMPLE_RATE).is_none());
    }

    #[test]
    fn detects_open_strings_with_offset_and_noise() {
        for expected in [82.41, 110.0, 146.83, 196.0, 246.94, 329.63] {
            let samples = noisy_sine_wave(expected, 0.38, 4096);
            let reading = analyze_pcm(&samples, SAMPLE_RATE).unwrap();

            assert!(
                (reading.frequency_hz - expected).abs() < 1.1,
                "expected {expected} Hz, got {} Hz",
                reading.frequency_hz,
            );
            assert!(
                reading.confidence > 0.74,
                "confidence was {}",
                reading.confidence
            );
        }
    }

    #[test]
    fn detects_weak_fundamentals_with_strong_harmonics_at_common_sample_rates() {
        for sample_rate in [44_100.0, 48_000.0] {
            for (expected_index, expected) in [82.41, 110.0, 146.83, 196.0, 246.94, 329.63]
                .into_iter()
                .enumerate()
            {
                let samples = harmonic_rich_pluck(expected, sample_rate, 4096);
                let reading = analyze_pcm(&samples, sample_rate).unwrap_or_else(|| {
                    panic!("failed to detect {expected} Hz at {sample_rate} Hz")
                });
                assert!(
                    (reading.frequency_hz - expected).abs() < 1.3,
                    "expected {expected} Hz at {sample_rate} Hz, got {} Hz",
                    reading.frequency_hz,
                );
                assert!(
                    reading.harmonic_support >= 0.04,
                    "harmonic support for {expected} Hz at {sample_rate} Hz was {}",
                    reading.harmonic_support
                );
                let estimate = crate::tuning::classify_reading(&reading, None, None)
                    .unwrap_or_else(|| panic!("classifier rejected {reading:?}"));
                assert_eq!(estimate.target_index, expected_index);
                if expected_index >= 4 {
                    assert!(
                        reading.subharmonic_support <= 0.03,
                        "subharmonic support for {expected} Hz at {sample_rate} Hz was {}",
                        reading.subharmonic_support
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_out_of_range_tones_before_they_can_fold_to_a_guitar_string() {
        for sample_rate in [44_100.0, 48_000.0] {
            for frequency in [381.0, 493.88, 659.26, 987.76] {
                let samples = sine_wave_at(frequency, 0.42, 8192, sample_rate);
                assert!(
                    analyze_pcm(&samples, sample_rate).is_none(),
                    "{frequency} Hz at {sample_rate} Hz must not be subharmonically folded"
                );
            }

            for frequency in [1201.0, 1318.52, 1320.0, 1468.3, 1600.0] {
                let samples = sine_wave_at(frequency, 0.75, 8192, sample_rate);
                assert!(
                    analyze_pcm(&samples, sample_rate).is_none(),
                    "high-frequency boundary {frequency} Hz at {sample_rate} Hz must be rejected"
                );
            }
        }
    }

    #[test]
    fn anti_alias_filter_rejects_high_tones_that_would_decimate_into_range() {
        for sample_rate in [44_100.0, 48_000.0] {
            let would_alias_to = 220.0;
            let high_frequency = sample_rate / DOWNSAMPLE_FACTOR as f32 - would_alias_to;
            let samples = sine_wave_at(high_frequency, 0.75, 8192, sample_rate);
            assert!(
                analyze_pcm(&samples, sample_rate).is_none(),
                "{high_frequency} Hz at {sample_rate} Hz would alias to {would_alias_to} Hz"
            );
        }
    }

    #[test]
    fn isolated_sines_do_not_claim_plucked_string_harmonic_support() {
        for frequency in [246.94, 329.63] {
            let samples = sine_wave(frequency, 0.42, 8192);
            let reading = analyze_pcm(&samples, SAMPLE_RATE).unwrap();
            assert!(
                reading.harmonic_support < 0.04,
                "pure {frequency} Hz sine reported support {}",
                reading.harmonic_support
            );
            assert!(
                reading.subharmonic_support < 0.03,
                "pure {frequency} Hz sine reported subharmonic support {}",
                reading.subharmonic_support
            );
        }
    }

    #[test]
    fn subharmonic_evidence_covers_lower_string_partial_lattices() {
        for (lower_fundamental, candidate_order) in [(82.41, 3), (110.0, 3), (82.41, 4)] {
            let reported_frequency = lower_fundamental * candidate_order as f32;
            let samples =
                lower_string_partial_wave(lower_fundamental, candidate_order, SAMPLE_RATE, 8192);
            let mut filtered = Vec::new();
            downsample_into(&samples, SAMPLE_RATE, DOWNSAMPLE_FACTOR, &mut filtered);
            remove_dc_offset(&mut filtered);
            let analysis_rate = SAMPLE_RATE / DOWNSAMPLE_FACTOR as f32;
            let mut window = Vec::new();
            let mut windowed = Vec::new();
            window_signal_into(&filtered, &mut window, &mut windowed);
            let (harmonic_support, subharmonic_support) =
                estimate_spectral_support(&windowed, analysis_rate, reported_frequency);

            assert!(
                harmonic_support >= 0.04,
                "test signal must resemble a harmonic-rich {reported_frequency} Hz candidate"
            );
            assert!(
                subharmonic_support > 0.03,
                "lower partials beneath {reported_frequency} Hz were not exposed"
            );
        }
    }

    fn sine_wave(frequency: f32, amplitude: f32, len: usize) -> Vec<i16> {
        sine_wave_at(frequency, amplitude, len, SAMPLE_RATE)
    }

    fn sine_wave_at(frequency: f32, amplitude: f32, len: usize, sample_rate: f32) -> Vec<i16> {
        (0..len)
            .map(|index| {
                let phase = TAU * frequency * index as f32 / sample_rate;
                (phase.sin() * amplitude * i16::MAX as f32) as i16
            })
            .collect()
    }

    fn noisy_sine_wave(frequency: f32, amplitude: f32, len: usize) -> Vec<i16> {
        let mut seed = 0x9e37_79b9_u32;
        (0..len)
            .map(|index| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = ((seed >> 8) as f32 / 16_777_215.0) - 0.5;
                let phase = TAU * frequency * index as f32 / SAMPLE_RATE;
                let sample = (phase.sin() * amplitude) + 0.018 + (noise * 0.018);
                (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            })
            .collect()
    }

    fn nylon_pluck(frequency: f32, len: usize) -> Vec<i16> {
        let mut seed = 0xa341_316c_u32;
        (0..len)
            .map(|index| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let time = index as f32 / SAMPLE_RATE;
                let phase = TAU * frequency * time;
                let envelope = (-time * 2.8).exp();
                let noise = ((seed >> 8) as f32 / 16_777_215.0) - 0.5;
                let sample = envelope
                    * (0.13 * phase.sin()
                        + 0.34 * (2.0 * phase).sin()
                        + 0.20 * (3.0 * phase).sin()
                        + 0.08 * (4.0 * phase).sin())
                    + noise * 0.008;
                (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            })
            .collect()
    }

    fn harmonic_rich_pluck(frequency: f32, sample_rate: f32, len: usize) -> Vec<i16> {
        let mut seed = 0x51f1_5e5d_u32;
        (0..len)
            .map(|index| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let time = index as f32 / sample_rate;
                let phase = TAU * frequency * time;
                let envelope = (-time * 2.4).exp();
                let noise = ((seed >> 8) as f32 / 16_777_215.0) - 0.5;
                let sample = envelope
                    * (0.035 * phase.sin()
                        + 0.46 * (2.0 * phase).sin()
                        + 0.29 * (3.0 * phase).sin()
                        + 0.13 * (4.0 * phase).sin())
                    + noise * 0.004;
                (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            })
            .collect()
    }

    fn lower_string_partial_wave(
        fundamental: f32,
        candidate_order: usize,
        sample_rate: f32,
        len: usize,
    ) -> Vec<i16> {
        (0..len)
            .map(|index| {
                let phase = TAU * fundamental * index as f32 / sample_rate;
                let candidate_phase = candidate_order as f32 * phase;
                let sample = 0.04 * phase.sin()
                    + 0.22 * (2.0 * phase).sin()
                    + 0.45 * candidate_phase.sin()
                    + 0.18 * (2.0 * candidate_phase).sin();
                (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            })
            .collect()
    }
}
